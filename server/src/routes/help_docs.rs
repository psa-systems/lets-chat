//! LC-712: AI help desk, Phase 1. A support assistant (`/support`) that answers
//! questions from a multi-product documentation knowledge base (Mokosh, Bunyip,
//! Let's Chat, ...) via retrieval-augmented generation.
//!
//! The pipeline reuses the existing AI plumbing wholesale: the operator
//! embeddings client (`crate::embeddings`) vectorises both the docs and the
//! question, ranking is the same in-Rust cosine scan as "find related"
//! (`db::doc_chunks`), the answer is produced by `LlmClient::complete_guarded`
//! (so the global [`crate::llm`] guardrail always applies), and it is posted as
//! the shared `assistant` bot with the same placeholder-then-edit UX as `/ask`
//! (`routes::assistant`). The whole surface is gated by [`super::ai_gate`].
//!
//! What is new here is a documentation corpus: docs are fetched from their
//! published docs sites (configured by an admin), split on headings, embedded,
//! and stored in the `doc_chunks` table. Citations are appended by code from the
//! chunks actually retrieved, so the source links are always real (never
//! model-invented). A retrieval that finds nothing above the relevance floor is
//! the Phase 1 confidence signal: the assistant says it could not find an answer
//! rather than guessing (Phase 2, LC-713, hooks the human handoff onto exactly
//! this branch).

use scraper::{Html, Selector};
use sha2::{Digest, Sha256};

use crate::db;
use crate::embeddings;
use crate::error::AppError;
use crate::models::{Room, User};
use crate::rate_limit::{Outcome, RateLimitKind};
use crate::state::AppState;

/// Settings key holding the configured documentation sources, one `product|url`
/// per line (blank lines and `#` comments ignored). Edited on `/admin/settings`.
pub const HELP_DOCS_SOURCES_KEY: &str = "help_docs_sources";

/// Settings key holding a short human-readable status line from the last index
/// run (shown read-only on `/admin/settings`).
pub const HELP_DOCS_STATUS_KEY: &str = "help_docs_last_status";

/// Max characters accepted in a question (bounds the prompt + abuse).
const MAX_QUESTION_CHARS: usize = 1000;
/// Per-user support asks per minute.
const SUPPORT_RATE_PER_MIN: u32 = 10;
/// Top-N chunks pulled into the retrieval context.
const TOP_K: usize = 6;
/// Minimum cosine similarity to treat a chunk as relevant. A best top score
/// below this floor is the "cannot answer from the docs" (low-confidence)
/// signal that Phase 2 will escalate on.
const MIN_SIMILARITY: f32 = 0.15;
/// Overall cap on the assembled context fed to the model.
const CONTEXT_CHARS: usize = 8000;
/// Max characters per stored chunk (keeps one long section from dominating a
/// page's vectors; longer sections are split on whitespace).
const CHUNK_MAX_CHARS: usize = 1200;
/// Safety cap on sub-pages fetched per source (a runaway docs index cannot make
/// the indexer fetch unboundedly).
const MAX_PAGES_PER_SOURCE: usize = 100;
/// Cap on a fetched page's bytes (a misbehaving docs host cannot exhaust memory).
const MAX_PAGE_BYTES: usize = 512 * 1024;
/// Per-fetch timeout for the docs HTTP GETs.
const FETCH_TIMEOUT_SECS: u64 = 15;
/// LC-713: per-user support escalations (`/human`) per minute. Tighter than the
/// ask cap because each escalation fans a DM + push + email to every admin.
const ESCALATE_RATE_PER_MIN: u32 = 3;
/// LC-713: an admin who interacted within this many minutes counts as available
/// "right now". Admins are notified regardless; this only picks which
/// confirmation the user sees.
const ADMIN_AVAILABLE_MINUTES: i64 = 5;
/// LC-713: max characters kept from the user's escalation note.
const MAX_ESCALATE_CHARS: usize = 1000;

/// LC-712: the INVARIANT half of the support system prompt - always applied
/// (the global [`crate::llm`] guardrail is prepended by `complete_guarded`). It
/// keeps the RAG contract (answer only from the provided docs), makes a
/// no-answer outcome honest + helpful rather than a guess, and keeps safety
/// refusals distinct from a documentation miss.
const SUPPORT_RULES: &str = "You are the support assistant for the a8n product \
suite (Mokosh, Bunyip, Let's Chat, and related products). Answer the user's question using ONLY the \
provided documentation excerpts. Attribute what you state to the documentation it came from. If the \
excerpts do not contain the answer, say so plainly and helpfully - tell the user you could not find \
it in the documentation and that they can ask an admin for a human - and do NOT guess or invent an \
answer. If the question asks for harmful, dangerous, illegal, or otherwise unsafe information, refuse \
briefly on safety grounds, and never phrase such a refusal as a documentation miss. Be concise and \
use Markdown. Do not invent facts, links, or quotes.";

/// Placeholder shown the instant a `/support` is sent, then replaced in place by
/// the answer. Italic so it reads as a transient status.
const THINKING_BODY: &str = "_The support assistant is checking the docs\u{2026}_";

/// One configured documentation source: a product name and its docs index URL.
#[derive(Debug, Clone, PartialEq)]
pub struct DocSource {
    pub product: String,
    pub index_url: String,
}

/// Parse the `help_docs_sources` setting: one `product|url` per line. Blank lines
/// and `#` comments are ignored; a line without a `|`, or with an empty half, or
/// a non-http(s) URL, is skipped (best-effort, so one bad line never breaks the
/// rest).
pub fn parse_sources(raw: &str) -> Vec<DocSource> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((product, url)) = line.split_once('|') else {
            continue;
        };
        let product = product.trim();
        let url = url.trim().trim_end_matches('/');
        if product.is_empty() || !(url.starts_with("https://") || url.starts_with("http://")) {
            continue;
        }
        out.push(DocSource {
            product: product.to_string(),
            index_url: url.to_string(),
        });
    }
    out
}

/// Split a URL into its origin (`scheme://host[:port]`) and path (leading slash,
/// no trailing slash, no query/fragment). A URL with no path returns an empty
/// path. Avoids pulling in a URL-parsing dependency for this narrow need.
fn split_origin_path(url: &str) -> Option<(String, String)> {
    let after_scheme = url.find("://")? + 3;
    let rest = &url[after_scheme..];
    match rest.find('/') {
        Some(slash) => {
            let origin = url[..after_scheme + slash].to_string();
            let path = &rest[slash..];
            let path = path.split(['?', '#']).next().unwrap_or(path);
            Some((origin, path.trim_end_matches('/').to_string()))
        }
        None => Some((url.to_string(), String::new())),
    }
}

/// Extract the docs sub-page URLs linked from an index page. Keeps only links
/// whose resolved path sits directly under the index path (so nav/footer links
/// to other areas are ignored), resolves site-absolute (`/...`) hrefs against
/// the index origin, dedups, and caps at [`MAX_PAGES_PER_SOURCE`].
fn parse_index_links(html: &str, index_url: &str) -> Vec<String> {
    let Some((origin, index_path)) = split_origin_path(index_url) else {
        return Vec::new();
    };
    let prefix = format!("{index_path}/");
    let doc = Html::parse_document(html);
    let Ok(sel) = Selector::parse("a[href]") else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for el in doc.select(&sel) {
        let Some(href) = el.value().attr("href") else {
            continue;
        };
        // Resolve to an absolute URL (only site-absolute and absolute hrefs; the
        // docs site uses site-absolute `/apps/.../docs/...` links).
        let abs = if href.starts_with("http://") || href.starts_with("https://") {
            href.to_string()
        } else if let Some(path) = href.strip_prefix('/') {
            format!("{origin}/{path}")
        } else {
            continue;
        };
        let Some((_, path)) = split_origin_path(&abs) else {
            continue;
        };
        // Directly under the index path, and not the index itself.
        if !path.starts_with(&prefix) || path == index_path {
            continue;
        }
        let clean = abs.split(['?', '#']).next().unwrap_or(&abs).to_string();
        let clean = clean.trim_end_matches('/').to_string();
        if seen.insert(clean.clone()) {
            out.push(clean);
            if out.len() >= MAX_PAGES_PER_SOURCE {
                break;
            }
        }
    }
    out
}

/// A parsed docs page: its title and its heading-delimited sections.
struct ParsedPage {
    title: String,
    /// `(heading, body)` sections in document order.
    sections: Vec<(String, String)>,
}

/// Collapse runs of whitespace to single spaces and trim.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a docs page's HTML into a title and heading-delimited sections. Content
/// is read from `<article class="docs-article">` (the docs site's content
/// container), falling back to `<main>` then the whole document, so a markup
/// change degrades to coarser extraction rather than empty results.
fn parse_page(html: &str) -> ParsedPage {
    let doc = Html::parse_document(html);

    // Title: the <title> tag's first segment (before " · site · Bunyip"),
    // falling back to the first <h1>.
    let title = Selector::parse("title")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|el| el.text().collect::<String>())
        .map(|t| t.split('\u{00b7}').next().unwrap_or(&t).trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            Selector::parse("h1")
                .ok()
                .and_then(|s| doc.select(&s).next())
                .map(|el| normalize_ws(&el.text().collect::<String>()))
        })
        .unwrap_or_default();

    // Content container: prefer the docs article, then main, then body.
    let container_html = ["article.docs-article", "main", "body"]
        .iter()
        .find_map(|sel| {
            Selector::parse(sel)
                .ok()
                .and_then(|s| doc.select(&s).next())
                .map(|el| el.inner_html())
        })
        .unwrap_or_else(|| html.to_string());

    let fragment = Html::parse_fragment(&container_html);
    let block = Selector::parse("h1, h2, h3, p, li, pre, blockquote, td")
        .unwrap_or_else(|_| Selector::parse("p").unwrap());

    let mut sections: Vec<(String, String)> = Vec::new();
    let mut cur_heading = title.clone();
    let mut cur_body = String::new();
    let flush = |sections: &mut Vec<(String, String)>, heading: &str, body: &str| {
        let body = body.trim();
        if !body.is_empty() {
            sections.push((heading.to_string(), body.to_string()));
        }
    };
    for el in fragment.select(&block) {
        let name = el.value().name();
        let text = normalize_ws(&el.text().collect::<String>());
        if text.is_empty() {
            continue;
        }
        if matches!(name, "h1" | "h2" | "h3") {
            flush(&mut sections, &cur_heading, &cur_body);
            cur_body.clear();
            cur_heading = text;
        } else {
            cur_body.push_str(&text);
            cur_body.push('\n');
        }
    }
    flush(&mut sections, &cur_heading, &cur_body);

    ParsedPage { title, sections }
}

/// Split a section body into chunks no longer than [`CHUNK_MAX_CHARS`], breaking
/// on whitespace so a chunk never splits mid-word. A short body yields one chunk.
fn chunk_body(body: &str) -> Vec<String> {
    let body = body.trim();
    if body.chars().count() <= CHUNK_MAX_CHARS {
        return if body.is_empty() {
            Vec::new()
        } else {
            vec![body.to_string()]
        };
    }
    let mut chunks = Vec::new();
    let mut cur = String::new();
    for word in body.split_whitespace() {
        if cur.chars().count() + word.chars().count() + 1 > CHUNK_MAX_CHARS && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

/// SHA-256 hex of a page's extracted text (all section bodies joined), used to
/// skip re-embedding an unchanged page on refresh.
fn page_hash(sections: &[(String, String)]) -> String {
    let mut hasher = Sha256::new();
    for (heading, body) in sections {
        hasher.update(heading.as_bytes());
        hasher.update(b"\n");
        hasher.update(body.as_bytes());
        hasher.update(b"\n");
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Fetch a URL's body as text via the SSRF-checked outbound GET (the docs sites
/// are public hosts), capped and timed. Returns the body or a short error label.
async fn fetch_text(url: &str) -> Result<String, String> {
    let req = crate::http_client::outbound_get(url)
        .await
        .map_err(|e| e.to_string())?
        .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS));
    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status().as_u16()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let capped = &bytes[..bytes.len().min(MAX_PAGE_BYTES)];
    Ok(String::from_utf8_lossy(capped).into_owned())
}

/// Outcome counters from an index run, rendered into the status line.
#[derive(Default)]
pub struct IndexReport {
    pub products: usize,
    pub pages: usize,
    pub chunks: usize,
    pub unchanged: usize,
    pub errors: usize,
    pub removed: u64,
}

/// Re-index every configured documentation source into `doc_chunks`. When
/// `force` is false a page whose extracted text is unchanged (same content hash)
/// is skipped without re-embedding. Products no longer present in the sources are
/// pruned. Best-effort per page (a fetch/embed error is counted and skipped, not
/// fatal). A no-op that records a status when embeddings are unconfigured or no
/// sources are set. Writes a human-readable status line to
/// [`HELP_DOCS_STATUS_KEY`] and returns the report.
pub async fn reindex_all(state: &AppState, force: bool) -> Result<IndexReport, AppError> {
    let mut report = IndexReport::default();
    let Some(client) = state.embedding_client.clone() else {
        set_status(state, "Not indexed: no embeddings endpoint is configured.").await;
        return Ok(report);
    };
    let raw = db::settings::get_setting(&state.settings, HELP_DOCS_SOURCES_KEY)
        .await?
        .unwrap_or_default();
    let sources = parse_sources(&raw);
    if sources.is_empty() {
        set_status(
            state,
            "Not indexed: no documentation sources are configured.",
        )
        .await;
        return Ok(report);
    }

    for source in &sources {
        report.products += 1;
        let index_html = match fetch_text(&source.index_url).await {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(url = %source.index_url, error = %e, "help docs index fetch failed");
                report.errors += 1;
                continue;
            }
        };
        let links = parse_index_links(&index_html, &source.index_url);
        for page_url in links {
            match index_page(state, &*client, &source.product, &page_url, force).await {
                Ok(PageOutcome::Indexed(n)) => {
                    report.pages += 1;
                    report.chunks += n;
                }
                Ok(PageOutcome::Unchanged) => report.unchanged += 1,
                Err(e) => {
                    tracing::warn!(url = %page_url, error = %e, "help docs page index failed");
                    report.errors += 1;
                }
            }
        }
    }

    // Prune products that are no longer configured.
    let configured: std::collections::HashSet<&str> =
        sources.iter().map(|s| s.product.as_str()).collect();
    if let Ok(existing) = db::doc_chunks::distinct_products(&state.chat).await {
        for product in existing {
            if !configured.contains(product.as_str()) {
                report.removed += db::doc_chunks::delete_by_product(&state.chat, &product)
                    .await
                    .unwrap_or(0);
            }
        }
    }

    set_status(state, &report.summary()).await;
    Ok(report)
}

enum PageOutcome {
    Indexed(usize),
    Unchanged,
}

/// Fetch, parse, chunk, embed, and upsert one docs page. Skips (returns
/// `Unchanged`) when the page's content hash already matches the stored one and
/// `force` is false.
async fn index_page(
    state: &AppState,
    client: &dyn embeddings::EmbeddingClient,
    product: &str,
    page_url: &str,
    force: bool,
) -> Result<PageOutcome, AppError> {
    let html = fetch_text(page_url)
        .await
        .map_err(|e| AppError::Internal(format!("fetch {page_url}: {e}")))?;
    let page = parse_page(&html);
    if page.sections.is_empty() {
        return Ok(PageOutcome::Indexed(0));
    }
    let hash = page_hash(&page.sections);
    if !force {
        if let Ok(Some(existing)) = db::doc_chunks::source_content_hash(&state.chat, page_url).await
        {
            if existing == hash {
                return Ok(PageOutcome::Unchanged);
            }
        }
    }

    // Build the flat chunk list first so a mid-page embed error does not leave a
    // page half-updated (chunks are only written after all embeds succeed).
    let mut prepared: Vec<(String, String, Vec<u8>, i64)> = Vec::new(); // heading, body, vec bytes, dim
    for (heading, body) in &page.sections {
        for chunk in chunk_body(body) {
            let embed_input = format!("{}\n{}", heading, chunk);
            let vec = client
                .embed(&embed_input)
                .await
                .map_err(|e| AppError::Internal(format!("embed: {e}")))?;
            let dim = vec.len() as i64;
            prepared.push((heading.clone(), chunk, embeddings::vec_to_bytes(&vec), dim));
        }
    }

    db::doc_chunks::delete_by_source(&state.chat, page_url).await?;
    let n = prepared.len();
    for (i, (heading, body, vec, dim)) in prepared.into_iter().enumerate() {
        db::doc_chunks::upsert(
            &state.chat,
            product,
            page_url,
            &page.title,
            &heading,
            i as i64,
            &body,
            &hash,
            dim,
            &vec,
        )
        .await?;
    }
    Ok(PageOutcome::Indexed(n))
}

impl IndexReport {
    fn summary(&self) -> String {
        format!(
            "Indexed {} product(s), {} page(s), {} chunk(s); {} unchanged, {} error(s), {} pruned.",
            self.products, self.pages, self.chunks, self.unchanged, self.errors, self.removed
        )
    }
}

async fn set_status(state: &AppState, status: &str) {
    if let Err(e) = db::settings::set_setting(&state.settings, HELP_DOCS_STATUS_KEY, status).await {
        tracing::warn!(error = %e, "failed to store help-docs status");
    }
}

/// Scheduled refresh tick (see `main::spawn_help_docs_refresh_dispatcher`).
/// A no-op unless the AI flag is on, an embeddings endpoint is configured, and
/// sources are set; otherwise re-indexes (content-hash skips unchanged pages).
pub async fn run_refresh_tick(state: &AppState) -> Result<(), AppError> {
    if !super::ai_gate::flag_on(state).await || state.embedding_client.is_none() {
        return Ok(());
    }
    let raw = db::settings::get_setting(&state.settings, HELP_DOCS_SOURCES_KEY)
        .await?
        .unwrap_or_default();
    if parse_sources(&raw).is_empty() {
        return Ok(());
    }
    reindex_all(state, false).await.map(|_| ())
}

/// Handle `/support <question>` for `asker` in `room`. Retrieves from the docs
/// knowledge base, answers with the LLM, and posts the reply as the assistant
/// bot (placeholder-then-edit, like `/ask`). Errors surface to the asker's
/// composer; nothing is posted on a gate failure.
pub(crate) async fn handle_support(
    state: &AppState,
    room: &Room,
    asker: &User,
    question: &str,
) -> Result<(), AppError> {
    let Some(llm) = state.llm_client.clone() else {
        return Err(AppError::BadRequest(
            "The AI assistant is not configured on this server.".into(),
        ));
    };
    if state.embedding_client.is_none() {
        return Err(AppError::BadRequest(
            "The help desk documentation search is not configured on this server.".into(),
        ));
    }
    // Same runtime flag + audience gate as /ask; refused server-side, not merely
    // hidden. Unlike /ask this does NOT require the room's assistant opt-in - the
    // docs helper is a global surface, not a per-room feature.
    super::ai_gate::require_llm_in_room(state, room.id, asker).await?;

    let question = question.trim();
    if question.is_empty() {
        return Err(AppError::BadRequest("usage: /support <question>".into()));
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return Err(AppError::BadRequest(
            "That question is too long for the support assistant.".into(),
        ));
    }
    if let Outcome::Deny { .. } =
        state
            .rate_limits
            .check(RateLimitKind::HelpDocsAsk, &asker.id, SUPPORT_RATE_PER_MIN)
    {
        return Err(AppError::BadRequest(
            "You're asking the support assistant too quickly. Try again in a minute.".into(),
        ));
    }

    let asker_label = match asker.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n,
        _ => &asker.username,
    };
    let header = format!("> {asker_label}: {question}");

    let bot = super::assistant::assistant_bot(state).await?;
    let placeholder = format!("{header}\n\n{THINKING_BODY}");
    let msg_id = db::chat::insert_message(&state.chat, room.id, &bot.id, &placeholder).await?;
    super::room::finalize_message_send(state, room, &bot, msg_id, &placeholder, None).await?;

    // Run retrieval + the (multi-second) LLM call off the request path; replace
    // the placeholder in place when the answer is ready (see `routes::assistant`).
    let st = state.clone();
    let room_id = room.id;
    let question = question.to_string();
    let is_admin = asker.role == "admin";
    // LC-719: when this /support runs inside a DM (the support bubble backs the
    // panel with the assistant-bot DM), push the refreshed panel thread to the
    // asker so the answer lands live without polling.
    let notify_panel = room.room_type == "dm";
    let asker_id = asker.id.clone();
    tokio::spawn(async move {
        let final_body = build_support_answer(&st, &question, &header, is_admin, &*llm).await;
        if let Err(e) = db::chat::replace_message_body(&st.chat, msg_id, &final_body).await {
            tracing::warn!(error = %e, message_id = msg_id, "failed to store support answer");
            return;
        }
        st.hub.broadcast_to_room(
            room_id,
            &crate::ws::events::ChatEvent::MessageEdited {
                message_id: msg_id,
                room_id,
                new_body: final_body,
                edited_at: String::new(),
            },
        );
        if notify_panel {
            st.hub.broadcast_to_user(
                &asker_id,
                &crate::ws::events::ChatEvent::SupportThreadChanged {
                    user_id: asker_id.clone(),
                },
            );
        }
    });
    Ok(())
}

/// Retrieve docs context, ask the LLM, and format the final answer body with a
/// code-appended, deduplicated `Sources` list (so the citation links are always
/// real). Returns a friendly, italic body on any failure so the placeholder
/// always resolves. Public for integration tests (which call it directly to
/// avoid the background-task timing of [`handle_support`]).
pub async fn build_support_answer(
    state: &AppState,
    question: &str,
    header: &str,
    is_admin: bool,
    llm: &dyn crate::llm::LlmClient,
) -> String {
    let Some(client) = state.embedding_client.clone() else {
        return format!("{header}\n\n_The help desk documentation search is not configured._");
    };
    let query_vec = match client.embed(question).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "support question embed failed");
            return format!(
                "{header}\n\n_The support assistant could not answer right now. Try again later._"
            );
        }
    };
    let chunks = match db::doc_chunks::list_for_scan(&state.chat, None).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "support docs load failed");
            return format!(
                "{header}\n\n_The support assistant could not answer right now. Try again later._"
            );
        }
    };

    // LC-715: empty knowledge base is a distinct state from a real no-match. If
    // no documentation has been indexed yet, say so explicitly instead of the
    // generic "couldn't find anything" reply (which reads as if the docs were
    // searched and came up empty). Admins get a direct link to the settings
    // panel where sources are added; regular users only get the `/human` hint,
    // so the setup path is not surfaced to non-admins.
    if chunks.is_empty() {
        return if is_admin {
            format!(
                "{header}\n\n_The help desk has no documentation configured yet, so I can't answer \
                 product questions. Add one or more sources under [Admin settings](/admin#st-helpdocs), \
                 then reindex. In the meantime, type `/human` to reach an admin._"
            )
        } else {
            format!(
                "{header}\n\n_The help desk isn't set up yet, so I can't answer product questions \
                 right now. Type `/human` to reach an admin._"
            )
        };
    }

    let mut scored: Vec<(f32, db::doc_chunks::DocChunk)> = chunks
        .into_iter()
        .map(|c| (embeddings::cosine_similarity(&query_vec, &c.vec), c))
        .filter(|(s, _)| *s >= MIN_SIMILARITY)
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(TOP_K);

    // Low-confidence: nothing above the relevance floor. Answer honestly rather
    // than guessing. (Phase 2 will offer a human handoff on this branch.)
    if scored.is_empty() {
        return format!(
            "{header}\n\n_I couldn't find anything about that in the product documentation. \
             Try rephrasing, or type `/human` to bring in an admin._"
        );
    }

    let mut context = String::new();
    for (_, c) in &scored {
        let loc = if c.heading.is_empty() || c.heading == c.title {
            format!("{} - {}", c.product, c.title)
        } else {
            format!("{} - {} - {}", c.product, c.title, c.heading)
        };
        context.push_str(&format!("[{loc}]\n{}\n\n", c.body));
    }
    let context: String = context.chars().take(CONTEXT_CHARS).collect();
    let user_content = format!("Documentation excerpts:\n{context}\n\nQuestion: {question}");

    let answer = match llm.complete_guarded(SUPPORT_RULES, &user_content).await {
        Ok(a) if !a.trim().is_empty() => a.trim().to_string(),
        Ok(_) => return format!("{header}\n\n_The support assistant returned an empty answer._"),
        Err(e) => {
            tracing::warn!(error = %e, "support LLM call failed");
            return format!(
                "{header}\n\n_The support assistant could not answer right now. Try again later._"
            );
        }
    };

    // Append real citations, deduplicated by source URL in rank order.
    let mut seen = std::collections::HashSet::new();
    let mut sources = String::new();
    for (_, c) in &scored {
        if seen.insert(c.source_url.clone()) {
            sources.push_str(&format!(
                "\n- {}: {} ({})",
                c.product, c.title, c.source_url
            ));
        }
    }
    format!("{header}\n\n{answer}\n\n**Sources:**{sources}")
}

// LC-713: human escalation ("talk to a human") ----------------------------

/// Result of a `/human` escalation: how many admins were notified, and whether
/// at least one was active recently (the "available right now" signal that only
/// changes the user-facing confirmation).
pub struct EscalationOutcome {
    pub notified: usize,
    pub available: bool,
}

/// Handle `/human [message]` for `asker` in `room`. Notifies the admins (via a
/// DM from the assistant bot that rides the existing DM push/email stack) that
/// the user wants a person, then posts a confirmation in the room stating
/// whether an admin is available now. Gated + rate-limited like the rest of the
/// help desk surface; errors surface to the asker's composer.
pub(crate) async fn handle_human(
    state: &AppState,
    room: &Room,
    asker: &User,
    message: &str,
) -> Result<(), AppError> {
    // Same runtime flag + audience gate as /support (the escalation is part of
    // the one help desk surface, so it toggles with it).
    super::ai_gate::require_llm_in_room(state, room.id, asker).await?;
    if let Outcome::Deny { .. } = state.rate_limits.check(
        RateLimitKind::SupportEscalate,
        &asker.id,
        ESCALATE_RATE_PER_MIN,
    ) {
        return Err(AppError::BadRequest(
            "You're requesting a human too quickly. Try again in a minute.".into(),
        ));
    }
    let message: String = message.trim().chars().take(MAX_ESCALATE_CHARS).collect();

    let outcome = escalate_to_admins(state, asker, room, &message).await?;

    let asker_label = match asker.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n,
        _ => &asker.username,
    };
    // LC-714: when a human is available now, the live notification suffices. When
    // none is available (idle admins, or none at all), file a durable support
    // ticket so the request is not lost, and surface it in the /admin/support
    // queue. The admins were still DMed above (they get it when they return).
    let confirmation = if outcome.available {
        format!("_{asker_label}, an admin has been notified and should respond here shortly._")
    } else {
        let body = if message.is_empty() {
            "(the user asked for a human via /human)".to_string()
        } else {
            message.clone()
        };
        let ticket_id =
            db::support_tickets::create(&state.chat, &asker.id, Some(room.id), &room.name, &body)
                .await?;
        super::support::broadcast_support_changed(state);
        format!(
            "_{asker_label}, no admins are available right now. I've filed support ticket #{ticket_id}; an admin will follow up._"
        )
    };
    let bot = super::assistant::assistant_bot(state).await?;
    let msg_id = db::chat::insert_message(&state.chat, room.id, &bot.id, &confirmation).await?;
    super::room::finalize_message_send(state, room, &bot, msg_id, &confirmation, None).await?;
    // LC-719: push the refreshed support panel when driven from the bubble (DM).
    if room.room_type == "dm" {
        state.hub.broadcast_to_user(
            &asker.id,
            &crate::ws::events::ChatEvent::SupportThreadChanged {
                user_id: asker.id.clone(),
            },
        );
    }
    Ok(())
}

/// LC-719: the ticket number a `/human` fallback confirmation carries, if any.
/// The support-bubble panel uses this to render an explicit "we've filed this
/// (ref #N)" stage. Kept next to the message it parses (the `handle_human`
/// confirmation above) so the two stay in sync; the confirmations are English
/// constants, not localized, so the marker is stable.
pub(crate) fn ticket_ref(body: &str) -> Option<i64> {
    let marker = "support ticket #";
    let start = body.find(marker)? + marker.len();
    let digits: String = body[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// LC-719: true when a bot reply is a low-confidence / cannot-answer outcome, so
/// the panel can surface a prominent "Still stuck? Talk to a human" affordance.
/// Matches the decline copy from [`build_support_answer`] (empty knowledge base
/// and no-match) - all of which point the user at a human.
pub(crate) fn is_low_confidence(body: &str) -> bool {
    body.contains("couldn't find anything about that in the product documentation")
        || body.contains("no documentation configured yet")
        || body.contains("isn't set up yet")
}

/// Notify every active admin (except the requester, if they are one) that
/// `requester` wants a human, by DMing them from the assistant bot with a link
/// back to `origin_room`. All admins are notified so an offline one still gets
/// the push/email/inbox when they return; the returned `available` flag reports
/// whether one was active recently. Public for integration tests.
pub async fn escalate_to_admins(
    state: &AppState,
    requester: &User,
    origin_room: &Room,
    message: &str,
) -> Result<EscalationOutcome, AppError> {
    let admins = db::auth::list_admins(&state.auth).await?;
    let targets: Vec<(String, String)> = admins
        .into_iter()
        .filter(|(id, _)| id != &requester.id)
        .collect();
    if targets.is_empty() {
        return Ok(EscalationOutcome {
            notified: 0,
            available: false,
        });
    }
    let available = db::auth::any_admin_active_within(&state.auth, ADMIN_AVAILABLE_MINUTES).await?;
    let bot = super::assistant::assistant_bot(state).await?;

    let requester_label = match requester.display_name.as_deref() {
        Some(n) if !n.trim().is_empty() => n.to_string(),
        _ => requester.username.clone(),
    };
    let quoted = if message.is_empty() {
        String::new()
    } else {
        format!("\n\n> {message}")
    };
    let dm_body = format!(
        "\u{1f198} **Help requested** by {requester_label} in [#{}](/room/{}).{quoted}\n\nReply in that channel to help them.",
        origin_room.name, origin_room.id
    );

    let mut notified = 0;
    for (admin_id, admin_username) in &targets {
        match dm_from_bot(state, &bot, admin_id, admin_username, &dm_body).await {
            Ok(()) => notified += 1,
            Err(e) => {
                tracing::warn!(admin_id = %admin_id, error = %e, "help desk escalation DM failed")
            }
        }
    }
    Ok(EscalationOutcome {
        notified,
        available,
    })
}

/// LC-714: send `body` as a DM from the assistant bot to `recipient_id`,
/// resolving the bot and recipient username. Used to tell a requester their
/// support ticket was resolved (from `routes::admin`). No-op if the recipient no
/// longer exists.
// Only the standalone admin support queue calls this; without the allow, the
// `saas` build (which never compiles `routes::admin`) fails `-D dead-code`.
#[cfg_attr(not(feature = "standalone"), allow(dead_code))]
pub(crate) async fn notify_user_from_bot(
    state: &AppState,
    recipient_id: &str,
    body: &str,
) -> Result<(), AppError> {
    let bot = super::assistant::assistant_bot(state).await?;
    let recipient_username = match db::auth::find_user_by_id(&state.auth, recipient_id).await? {
        Some(rec) => rec.username,
        None => return Ok(()),
    };
    dm_from_bot(state, &bot, recipient_id, &recipient_username, body).await
}

/// LC-718: find-or-create the assistant-bot <-> `user` DM room that backs the
/// support chat bubble. This is the same DM the escalation notices use, so the
/// bubble thread and any `/support` answers persist across page loads and rooms.
/// A freshly created room nudges the user's sidebar (mirrors [`dm_from_bot`]).
pub(crate) async fn support_dm_room(state: &AppState, user: &User) -> Result<Room, AppError> {
    let bot = super::assistant::assistant_bot(state).await?;
    if let Some(r) = db::chat::find_dm_room(&state.chat, &bot.id, &user.id).await? {
        return Ok(r);
    }
    let name = format!("@{}", bot.username);
    let room = db::chat::create_dm_room(&state.chat, &name, &bot.id, &user.id).await?;
    state.hub.broadcast_to_user(
        &user.id,
        &crate::ws::events::ChatEvent::RoomMemberAdded {
            room_id: room.id,
            user_id: user.id.clone(),
        },
    );
    Ok(room)
}

/// Send `body` as a DM from the assistant bot to `recipient_id`. Finds or creates
/// the bot-to-recipient DM room, then posts through `finalize_message_send`,
/// which for a DM room already fans out the peer notification (WS + push +
/// email) to the recipient, so the message rides the exact same delivery path as
/// any DM (no bespoke notification code, no double-send).
async fn dm_from_bot(
    state: &AppState,
    bot: &User,
    recipient_id: &str,
    recipient_username: &str,
    body: &str,
) -> Result<(), AppError> {
    let (room, created) = match db::chat::find_dm_room(&state.chat, &bot.id, recipient_id).await? {
        Some(r) => (r, false),
        None => {
            let name = format!("@{recipient_username}");
            let r = db::chat::create_dm_room(&state.chat, &name, &bot.id, recipient_id).await?;
            (r, true)
        }
    };
    if created {
        // New DM: nudge the recipient's sidebar to pick it up live (mirrors routes::dm).
        state.hub.broadcast_to_user(
            recipient_id,
            &crate::ws::events::ChatEvent::RoomMemberAdded {
                room_id: room.id,
                user_id: recipient_id.to_string(),
            },
        );
    }
    let msg_id = db::chat::insert_message(&state.chat, room.id, &bot.id, body).await?;
    super::room::finalize_message_send(state, &room, bot, msg_id, body, None).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_ref_extracts_the_filed_ticket_number() {
        // The exact /human fallback confirmation.
        let filed = "_alice, no admins are available right now. I've filed support ticket #42; an admin will follow up._";
        assert_eq!(ticket_ref(filed), Some(42));
        // The "available now" confirmation carries no ticket.
        assert_eq!(
            ticket_ref("_alice, an admin has been notified and should respond shortly._"),
            None
        );
        assert_eq!(ticket_ref("a normal answer from the docs"), None);
    }

    #[test]
    fn is_low_confidence_matches_the_decline_copy() {
        assert!(is_low_confidence(
            "> q\n\n_I couldn't find anything about that in the product documentation. Try rephrasing, or type `/human`._"
        ));
        assert!(is_low_confidence(
            "_The help desk has no documentation configured yet, so I can't answer._"
        ));
        assert!(is_low_confidence(
            "_The help desk isn't set up yet, so I can't answer product questions right now._"
        ));
        assert!(!is_low_confidence(
            "> q\n\nTo reset your password, open Settings.\n\n**Sources:** ..."
        ));
    }

    #[test]
    fn parse_sources_keeps_valid_and_drops_junk() {
        let raw = "# a comment\n\nmokosh-server | https://a8n.systems/apps/mokosh-server/docs/\n\
                   mokosh-www|https://a8n.systems/apps/mokosh-www/docs\n\
                   nodelimiter here\n\
                   |https://empty-product\n\
                   bad | ftp://nope\n";
        let s = parse_sources(raw);
        assert_eq!(
            s,
            vec![
                DocSource {
                    product: "mokosh-server".into(),
                    index_url: "https://a8n.systems/apps/mokosh-server/docs".into(),
                },
                DocSource {
                    product: "mokosh-www".into(),
                    index_url: "https://a8n.systems/apps/mokosh-www/docs".into(),
                },
            ]
        );
    }

    #[test]
    fn split_origin_path_basics() {
        assert_eq!(
            split_origin_path("https://a8n.systems/apps/mokosh-server/docs"),
            Some((
                "https://a8n.systems".into(),
                "/apps/mokosh-server/docs".into()
            ))
        );
        assert_eq!(
            split_origin_path("https://a8n.systems/apps/x/docs/page?y=1#z"),
            Some(("https://a8n.systems".into(), "/apps/x/docs/page".into()))
        );
        assert_eq!(
            split_origin_path("https://a8n.systems"),
            Some(("https://a8n.systems".into(), String::new()))
        );
    }

    #[test]
    fn index_links_keeps_only_sub_pages() {
        let html = r#"
            <a href="/apps/mokosh-server/docs/getting-started">GS</a>
            <a href="/apps/mokosh-server/docs/configuration">Cfg</a>
            <a href="/apps/mokosh-server/docs">Index self</a>
            <a href="/apps/other/docs/thing">Other product</a>
            <a href="/account">Nav</a>
            <a href="https://a8n.systems/apps/mokosh-server/docs/quick-start">Abs</a>
        "#;
        let mut links = parse_index_links(html, "https://a8n.systems/apps/mokosh-server/docs");
        links.sort();
        assert_eq!(
            links,
            vec![
                "https://a8n.systems/apps/mokosh-server/docs/configuration".to_string(),
                "https://a8n.systems/apps/mokosh-server/docs/getting-started".to_string(),
                "https://a8n.systems/apps/mokosh-server/docs/quick-start".to_string(),
            ]
        );
    }

    #[test]
    fn parse_page_extracts_title_and_sections() {
        let html = r#"<html><head><title>Getting Started · mokosh-server docs · Bunyip</title></head>
            <body><nav><a href="/x">skip</a></nav>
            <main><article class="docs-article">
              <h1>Getting started with Mokosh Server</h1>
              <p>Mokosh Server is the self-hosted PSA API backend.</p>
              <h2>Get it</h2>
              <p>Mokosh Server ships as a container image.</p>
              <h2>Run it</h2>
              <p>Run the image with your configuration as environment variables.</p>
            </article></main></body></html>"#;
        let page = parse_page(html);
        assert!(page.title.starts_with("Getting Started"));
        // Intro (under the h1/title), then the two h2 sections.
        assert_eq!(page.sections.len(), 3);
        assert_eq!(page.sections[1].0, "Get it");
        assert!(page.sections[1].1.contains("container image"));
        assert_eq!(page.sections[2].0, "Run it");
    }

    #[test]
    fn chunk_body_splits_long_text_on_whitespace() {
        let short = "one two three";
        assert_eq!(chunk_body(short), vec!["one two three".to_string()]);
        let long = "word ".repeat(600); // ~3000 chars
        let chunks = chunk_body(long.trim());
        assert!(chunks.len() > 1, "long body should split");
        assert!(chunks.iter().all(|c| c.chars().count() <= CHUNK_MAX_CHARS));
        // No word was cut in half.
        assert!(chunks
            .iter()
            .all(|c| c.split_whitespace().all(|w| w == "word")));
    }

    #[test]
    fn page_hash_is_stable_and_content_sensitive() {
        let a = vec![("H".to_string(), "body text".to_string())];
        let b = vec![("H".to_string(), "body text".to_string())];
        let c = vec![("H".to_string(), "different".to_string())];
        assert_eq!(page_hash(&a), page_hash(&b));
        assert_ne!(page_hash(&a), page_hash(&c));
    }
}
