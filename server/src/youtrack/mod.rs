//! LC-1135: file call-transcript action items into an org-wide YouTrack.
//!
//! An admin configures one YouTrack in `/admin/settings` (base URL, an API
//! token, a default project). The token is sealed at rest with the instance
//! secret key, exactly like the IMAP password, and is never rendered or logged.
//! Generation turns a transcript into proposed issues via the LLM
//! ([`parse_proposals`]); filing creates the issues over the blessed
//! operator-trusted HTTP path ([`create_issue`]).
//!
//! This module splits the IO from the pure logic: [`build_issue_payload`] and
//! [`parse_created_key`] are unit-tested, [`create_issue`] is the thin send that
//! wires them to `http_client::outbound_trusted_post` (the same trusted path the
//! LLM / STT / embeddings clients use for their operator-configured endpoints).

use base64::Engine;

use crate::db;
use crate::db::transcript_proposals::NewProposal;
use crate::state::AppState;

/// Settings KV keys for the YouTrack connection.
pub const BASE_URL_KEY: &str = "youtrack_base_url";
pub const TOKEN_KEY: &str = "youtrack_token";
pub const PROJECT_KEY: &str = "youtrack_default_project";

/// Bounds on a generated proposal set, mirroring `followups::parse_action_items`
/// so a runaway model response cannot create an unbounded list.
const MAX_PROPOSALS: usize = 50;
const MAX_SUMMARY_LEN: usize = 500;
const MAX_SOURCE_LEN: usize = 2000;

/// The resolved, usable YouTrack connection. Present only when all three
/// settings exist and the token could be unsealed.
#[derive(Debug, Clone)]
pub struct YoutrackConfig {
    pub base_url: String,
    pub token: String,
    pub default_project: String,
}

// --- configuration --------------------------------------------------------

/// Seal `token` with the instance secret key and store it as the `youtrack_token`
/// settings row, base64 `v1:{nonce}:{ciphertext}`. An empty token clears the row.
pub async fn store_token(state: &AppState, token: &str) -> Result<(), crate::error::AppError> {
    put_token(&state.settings, state.secret_key.as_deref(), token).await
}

/// [`store_token`] against a raw settings pool + key (so tests can seed a token
/// without an `AppState`). An empty token clears the row; a non-empty one
/// requires a secret key.
pub async fn put_token(
    settings: &sqlx::SqlitePool,
    secret_key: Option<&[u8; 32]>,
    token: &str,
) -> Result<(), crate::error::AppError> {
    if token.is_empty() {
        db::settings::set_setting(settings, TOKEN_KEY, "").await?;
        return Ok(());
    }
    let key = secret_key
        .ok_or_else(|| crate::error::AppError::BadRequest("secret key not configured".into()))?;
    let (ciphertext, nonce) = crate::crypto::seal(key, token.as_bytes())
        .map_err(|_| crate::error::AppError::Internal("token seal failed".into()))?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let sealed = format!("v1:{}:{}", b64.encode(nonce), b64.encode(ciphertext));
    db::settings::set_setting(settings, TOKEN_KEY, &sealed).await?;
    Ok(())
}

/// Unseal a stored `v1:{nonce}:{ciphertext}` token blob. Returns None for an
/// absent/blank row, a malformed blob, a missing secret key, or a decrypt
/// failure (e.g. the secret key was rotated after the token was written).
fn unseal_token(sealed: Option<&str>, secret_key: Option<&[u8; 32]>) -> Option<String> {
    let sealed = sealed?.trim();
    if sealed.is_empty() {
        return None;
    }
    let rest = sealed.strip_prefix("v1:")?;
    let (nonce_b64, ct_b64) = rest.split_once(':')?;
    let b64 = base64::engine::general_purpose::STANDARD;
    let nonce = b64.decode(nonce_b64).ok()?;
    let ciphertext = b64.decode(ct_b64).ok()?;
    let key = secret_key?;
    let plain = crate::crypto::open(key, &nonce, &ciphertext).ok()?;
    String::from_utf8(plain).ok()
}

/// Narrow three raw settings reads into a usable config. Pure, so it is unit
/// tested without a live settings store or tracing subscriber (LC-782). A read
/// `Err` is treated as absent (None), matching the LC-919 fail-safe posture.
fn narrow_config(
    base_url: Result<Option<String>, sqlx::Error>,
    token_row: Result<Option<String>, sqlx::Error>,
    project: Result<Option<String>, sqlx::Error>,
    secret_key: Option<&[u8; 32]>,
) -> Option<YoutrackConfig> {
    let non_empty = |r: Result<Option<String>, sqlx::Error>| -> Option<String> {
        match r {
            Ok(Some(v)) if !v.trim().is_empty() => Some(v.trim().to_string()),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(error = %e, "youtrack config setting read failed; treating as unset");
                None
            }
        }
    };
    let base_url = non_empty(base_url)?;
    let default_project = non_empty(project)?;
    let token_raw = match token_row {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "youtrack token read failed; treating as unset");
            None
        }
    };
    let token = unseal_token(token_raw.as_deref(), secret_key)?;
    Some(YoutrackConfig {
        base_url: base_url.trim_end_matches('/').to_string(),
        token,
        default_project,
    })
}

/// Load the live YouTrack connection, or None when it is not fully configured.
pub async fn load_config(state: &AppState) -> Option<YoutrackConfig> {
    let base_url = db::settings::get_setting(&state.settings, BASE_URL_KEY).await;
    let token = db::settings::get_setting(&state.settings, TOKEN_KEY).await;
    let project = db::settings::get_setting(&state.settings, PROJECT_KEY).await;
    narrow_config(base_url, token, project, state.secret_key.as_deref())
}

/// True when a token row is present (for the write-only admin field: the UI
/// shows "configured" without ever rendering the value).
pub async fn token_configured(state: &AppState) -> bool {
    matches!(
        db::settings::get_setting(&state.settings, TOKEN_KEY).await,
        Ok(Some(v)) if !v.trim().is_empty()
    )
}

// --- gates ----------------------------------------------------------------

/// True when the YouTrack-issues feature flag is not Off (the kill switch).
pub async fn flag_on(state: &AppState) -> bool {
    crate::routes::feature_flags::rollout(state, &crate::routes::feature_flags::YOUTRACK_ISSUES)
        .await
        .is_on()
}

/// May `user` review proposed issues for a transcript in `room_id`? The surface
/// uses the LLM, so review is scoped by the AI gate (room AI toggle + audience);
/// the flag must also be on. (LC-1135 ruling: one audience concept, the AI gate.)
pub async fn can_review(
    state: &AppState,
    room_id: i64,
    user: &crate::models::User,
) -> Result<bool, crate::error::AppError> {
    Ok(flag_on(state).await
        && crate::routes::ai_gate::allowed_in_room(state, room_id, user).await?)
}

/// May `user` FILE issues to YouTrack from a transcript in `room_id`? Filing is
/// a trusted action, so it needs the flag on AND the LC-679 staff set (site
/// admin / enclave Owner|Admin / room moderator).
pub async fn can_file(
    state: &AppState,
    room_id: i64,
    user: &crate::models::User,
) -> Result<bool, crate::error::AppError> {
    Ok(flag_on(state).await
        && crate::routes::ai_gate::privileged_in_room(state, room_id, user).await?)
}

/// 403 unless [`can_file`].
pub async fn require_file(
    state: &AppState,
    room_id: i64,
    user: &crate::models::User,
) -> Result<(), crate::error::AppError> {
    if can_file(state, room_id, user).await? {
        Ok(())
    } else {
        Err(crate::error::AppError::Forbidden)
    }
}

/// Build the public issue URL for a filed proposal's readable key.
pub fn issue_url(base_url: &str, key: &str) -> String {
    format!("{}/issue/{}", base_url.trim_end_matches('/'), key)
}

// --- issue creation -------------------------------------------------------

#[derive(Debug)]
pub struct NewIssue<'a> {
    pub summary: &'a str,
    pub description: &'a str,
    /// A YouTrack login to assign to, if the reviewer mapped one. Best-effort.
    pub assignee_login: Option<&'a str>,
}

#[derive(Debug)]
pub enum YoutrackError {
    /// Transport / connection failure. Never carries the token.
    Transport(String),
    /// A non-2xx response, with the status code only.
    Status(u16),
    /// The response could not be decoded / had no issue key.
    Decode(String),
    /// The configured project could not be resolved to an id.
    UnknownProject(String),
}

impl std::fmt::Display for YoutrackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            YoutrackError::Transport(e) => write!(f, "youtrack transport error: {e}"),
            YoutrackError::Status(s) => write!(f, "youtrack returned status {s}"),
            YoutrackError::Decode(e) => write!(f, "youtrack response decode error: {e}"),
            YoutrackError::UnknownProject(p) => write!(f, "youtrack project not found: {p}"),
        }
    }
}

impl std::error::Error for YoutrackError {}

/// Does the configured project string look like an internal YouTrack id
/// (e.g. `0-28`)? If so it is used directly; otherwise it is treated as a short
/// name and resolved via the projects endpoint.
fn is_internal_project_id(s: &str) -> bool {
    let mut parts = s.splitn(2, '-');
    matches!((parts.next(), parts.next()), (Some(a), Some(b))
        if !a.is_empty() && !b.is_empty()
        && a.bytes().all(|c| c.is_ascii_digit())
        && b.bytes().all(|c| c.is_ascii_digit()))
}

/// Build the `POST /api/issues` JSON body for an issue in `project_id`. Pure.
fn build_issue_payload(project_id: &str, issue: &NewIssue<'_>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "project": { "id": project_id },
        "summary": issue.summary,
        "description": issue.description,
    });
    if let Some(login) = issue.assignee_login {
        // Best-effort Assignee single-user custom field. A YouTrack instance
        // without this field, or an unknown login, leaves the issue unassigned
        // rather than failing the create (handled at the call site).
        body["customFields"] = serde_json::json!([{
            "name": "Assignee",
            "$type": "SingleUserIssueCustomField",
            "value": { "login": login },
        }]);
    }
    body
}

/// Pull the created issue's readable key (e.g. `LC-1234`) out of a create
/// response body. Pure.
fn parse_created_key(body: &serde_json::Value) -> Option<String> {
    body.get("idReadable")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

async fn resolve_project_id(cfg: &YoutrackConfig) -> Result<String, YoutrackError> {
    if is_internal_project_id(&cfg.default_project) {
        return Ok(cfg.default_project.clone());
    }
    let encoded_query = percent_encoding::utf8_percent_encode(
        &cfg.default_project,
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    let url = format!(
        "{}/api/admin/projects?fields=id,shortName&query={}",
        cfg.base_url, encoded_query
    );
    // Trusted GET: the base URL is operator config and may be an internal/
    // loopback host, the same posture as the create POST below. The public-IP
    // SSRF filter would reject a self-hosted YouTrack (LC-1135 review #2).
    let resp = crate::http_client::outbound_trusted_get(&url)
        .await
        .map_err(|e| YoutrackError::Transport(e.to_string()))?
        .bearer_auth(&cfg.token)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| YoutrackError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(YoutrackError::Status(resp.status().as_u16()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| YoutrackError::Decode(e.to_string()))?;
    body.as_array()
        .and_then(|arr| {
            arr.iter().find(|p| {
                p.get("shortName").and_then(|s| s.as_str()) == Some(cfg.default_project.as_str())
            })
        })
        .and_then(|p| p.get("id").and_then(|i| i.as_str()))
        .map(|s| s.to_string())
        .ok_or_else(|| YoutrackError::UnknownProject(cfg.default_project.clone()))
}

/// Create a YouTrack issue and return its readable key (e.g. `LC-1234`).
pub async fn create_issue(
    cfg: &YoutrackConfig,
    issue: &NewIssue<'_>,
) -> Result<String, YoutrackError> {
    let project_id = resolve_project_id(cfg).await?;
    let payload = build_issue_payload(&project_id, issue);
    let url = format!("{}/api/issues?fields=idReadable", cfg.base_url);
    let resp = crate::http_client::outbound_trusted_post(&url)
        .await
        .map_err(|e| YoutrackError::Transport(e.to_string()))?
        .bearer_auth(&cfg.token)
        .header("Accept", "application/json")
        .json(&payload)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| YoutrackError::Transport(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(YoutrackError::Status(resp.status().as_u16()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| YoutrackError::Decode(e.to_string()))?;
    parse_created_key(&body)
        .ok_or_else(|| YoutrackError::Decode("no idReadable in response".into()))
}

// --- proposal generation (LLM) --------------------------------------------

/// The system prompt for turning a transcript into proposed issues. The
/// transcript is DATA (guarded by the LLM client's guardrail wrapper), so a line
/// inside it can never act as an instruction.
pub const PROPOSE_SYSTEM: &str =
    "You read a call transcript (each line is 'Name: what they said', \
led by a 'Participants:' line) and extract the concrete tasks, promises and follow-ups worth \
tracking as issues. Reply with ONLY a JSON array, no prose and no code fence. Each element is an \
object with exactly these keys: \"summary\" (a short imperative issue title), \"assignee\" (the \
name of the participant who owns it, copied verbatim from the roster, or null if unclear), and \
\"source_lines\" (the transcript line or lines the task came from, joined by newlines). Include an \
item only when the transcript states a real, actionable task; never invent people, names, tasks, \
or details. If there are no actionable tasks, reply with an empty array [].";

/// Parse the model's output into proposals. Tolerant of a ```json code fence,
/// drops malformed entries, and bounds the count and field lengths.
pub fn parse_proposals(model_output: &str) -> Vec<NewProposal> {
    let trimmed = strip_code_fence(model_output.trim());
    let value: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(arr) = value.as_array() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let Some(summary) = item.get("summary").and_then(|v| v.as_str()) else {
            continue;
        };
        let summary = summary.trim();
        if summary.is_empty() {
            continue;
        }
        let assignee = item
            .get("assignee")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let source_lines = item
            .get("source_lines")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .chars()
            .take(MAX_SOURCE_LEN)
            .collect::<String>();
        out.push(NewProposal {
            summary: summary.chars().take(MAX_SUMMARY_LEN).collect(),
            assignee_name: assignee,
            source_lines,
        });
        if out.len() >= MAX_PROPOSALS {
            break;
        }
    }
    out
}

/// Strip a leading/trailing ```json ... ``` fence if the model wrapped its JSON.
fn strip_code_fence(s: &str) -> &str {
    let s = s.trim();
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    // Drop the fence's info string (e.g. "json") up to the first newline.
    let rest = match rest.split_once('\n') {
        Some((_, body)) => body,
        None => rest,
    };
    rest.trim().strip_suffix("```").unwrap_or(rest).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- config narrowing / token sealing ---

    fn ok(v: &str) -> Result<Option<String>, sqlx::Error> {
        Ok(Some(v.to_string()))
    }

    #[test]
    fn narrow_config_is_none_when_any_field_missing() {
        let key = [7u8; 32];
        // token sealed so it would unseal, but base_url missing -> None
        let (ct, nonce) = crate::crypto::seal(&key, b"tok").unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let sealed = format!("v1:{}:{}", b64.encode(&nonce), b64.encode(&ct));
        assert!(narrow_config(Ok(None), ok(&sealed), ok("LC"), Some(&key)).is_none());
        assert!(narrow_config(ok("https://yt"), Ok(None), ok("LC"), Some(&key)).is_none());
        assert!(narrow_config(ok("https://yt"), ok(&sealed), Ok(None), Some(&key)).is_none());
    }

    #[test]
    fn narrow_config_round_trips_a_sealed_token_and_trims_base_url() {
        let key = [9u8; 32];
        let (ct, nonce) = crate::crypto::seal(&key, b"s3cr3t").unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let sealed = format!("v1:{}:{}", b64.encode(&nonce), b64.encode(&ct));
        let cfg =
            narrow_config(ok("https://yt.example/"), ok(&sealed), ok("LC"), Some(&key)).unwrap();
        assert_eq!(cfg.base_url, "https://yt.example"); // trailing slash trimmed
        assert_eq!(cfg.token, "s3cr3t");
        assert_eq!(cfg.default_project, "LC");
    }

    #[test]
    fn narrow_config_is_none_on_read_err_or_bad_key() {
        let key = [1u8; 32];
        let (ct, nonce) = crate::crypto::seal(&key, b"tok").unwrap();
        let b64 = base64::engine::general_purpose::STANDARD;
        let sealed = format!("v1:{}:{}", b64.encode(&nonce), b64.encode(&ct));
        // read error on base_url -> None
        assert!(narrow_config(
            Err(sqlx::Error::RowNotFound),
            ok(&sealed),
            ok("LC"),
            Some(&key)
        )
        .is_none());
        // wrong secret key cannot unseal -> None
        let wrong = [2u8; 32];
        assert!(narrow_config(ok("https://yt"), ok(&sealed), ok("LC"), Some(&wrong)).is_none());
        // no secret key -> None
        assert!(narrow_config(ok("https://yt"), ok(&sealed), ok("LC"), None).is_none());
    }

    // --- issue payload / response parse ---

    #[test]
    fn build_issue_payload_sets_project_summary_description() {
        let p = build_issue_payload(
            "0-7",
            &NewIssue {
                summary: "Do X",
                description: "from the call",
                assignee_login: None,
            },
        );
        assert_eq!(p["project"]["id"], "0-7");
        assert_eq!(p["summary"], "Do X");
        assert_eq!(p["description"], "from the call");
        assert!(p.get("customFields").is_none());
    }

    #[test]
    fn build_issue_payload_sets_assignee_custom_field_when_mapped() {
        let p = build_issue_payload(
            "0-7",
            &NewIssue {
                summary: "Do X",
                description: "d",
                assignee_login: Some("vas"),
            },
        );
        assert_eq!(p["customFields"][0]["name"], "Assignee");
        assert_eq!(p["customFields"][0]["value"]["login"], "vas");
    }

    #[test]
    fn parse_created_key_reads_id_readable() {
        assert_eq!(
            parse_created_key(&serde_json::json!({"idReadable": "LC-1234"})),
            Some("LC-1234".to_string())
        );
        assert_eq!(
            parse_created_key(&serde_json::json!({"idReadable": ""})),
            None
        );
        assert_eq!(parse_created_key(&serde_json::json!({})), None);
    }

    #[test]
    fn is_internal_project_id_recognises_the_numeric_pair() {
        assert!(is_internal_project_id("0-28"));
        assert!(is_internal_project_id("12-3"));
        assert!(!is_internal_project_id("LC"));
        assert!(!is_internal_project_id("0-"));
        assert!(!is_internal_project_id("-28"));
        assert!(!is_internal_project_id("LC-28"));
    }

    #[test]
    fn error_display_does_not_leak_a_token() {
        // Status/Transport/Decode never embed the token; prove the formatted
        // string is the status/message only.
        assert_eq!(
            YoutrackError::Status(401).to_string(),
            "youtrack returned status 401"
        );
        assert!(!YoutrackError::Transport("connect refused".into())
            .to_string()
            .contains("Bearer"));
    }

    // --- proposal parsing ---

    #[test]
    fn parse_proposals_reads_a_clean_json_array() {
        let out = parse_proposals(
            r#"[{"summary":"Ship it","assignee":"Vas","source_lines":"Vas: I'll ship it"},
                {"summary":"Write docs","assignee":null,"source_lines":"Ada: document it"}]"#,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].summary, "Ship it");
        assert_eq!(out[0].assignee_name.as_deref(), Some("Vas"));
        assert_eq!(out[1].assignee_name, None);
    }

    #[test]
    fn parse_proposals_tolerates_a_code_fence() {
        let out = parse_proposals("```json\n[{\"summary\":\"A\",\"source_lines\":\"x\"}]\n```");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].summary, "A");
    }

    #[test]
    fn parse_proposals_drops_malformed_and_caps_count_and_length() {
        // one good, one missing summary (dropped), one blank summary (dropped)
        let out = parse_proposals(r#"[{"summary":"Keep"},{"assignee":"x"},{"summary":"  "}]"#);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].summary, "Keep");

        // count cap
        let many: String = format!(
            "[{}]",
            (0..80)
                .map(|i| format!("{{\"summary\":\"t{i}\"}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(parse_proposals(&many).len(), MAX_PROPOSALS);

        // length cap
        let long = format!("[{{\"summary\":\"{}\"}}]", "a".repeat(1000));
        assert_eq!(
            parse_proposals(&long)[0].summary.chars().count(),
            MAX_SUMMARY_LEN
        );
    }

    #[test]
    fn parse_proposals_empty_on_garbage_or_empty_array() {
        assert!(parse_proposals("not json at all").is_empty());
        assert!(parse_proposals("[]").is_empty());
        assert!(parse_proposals("{\"not\":\"an array\"}").is_empty());
    }
}
