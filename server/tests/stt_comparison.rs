//! LC-861: same-audio comparison harness for transcription services.
//!
//! Engine selection (LC-843) is only useful with a way to judge services
//! against each other. Comparing them across separate live meetings is not a
//! controlled experiment - the audio differs every run, so results are not
//! comparable and regressions are invisible. This harness replays the SAME
//! recorded fixtures through every candidate `SttClient` adapter (a plain
//! `transcribe()` call, no live call and no second machine), scores each
//! output against a hand-corrected reference by word error rate, times it, and
//! prints one comparison table.
//!
//! ## Metric note (a deliberate revision of the ticket)
//!
//! The ticket proposed "word error rate plus speaker attribution accuracy,
//! revise if validation shows otherwise". Validation shows otherwise: the
//! `SttClient` adapters return speaker-LESS text (`SttResult { text, segments
//! }`) - no diarization is requested or parsed from any provider. Speaker
//! attribution in lets-chat happens OUTSIDE the STT layer (one call/track per
//! participant, attributed by the call plumbing), so there is no per-word
//! speaker label in the adapter output to score. The metric is therefore WER +
//! latency. The concurrent-speaker fixture is exactly where a single mixed
//! stream jumbles, and WER captures that as a spike of substitutions/deletions.
//!
//! ## What runs where
//!
//! - The `#[test]`/`#[tokio::test]` functions below are DETERMINISTIC: they
//!   score known reference/hypothesis pairs and drive the runner with
//!   `MockSttClient`, so they run in CI with no audio, no network, no services.
//!   They are the coverage for the harness itself (a known-good fixture and an
//!   expected score).
//! - `live_comparison_across_configured_services` is `#[ignore]`d: it dials out
//!   to real endpoints and prints the table. Configure candidates in
//!   `LETS_CHAT_STT_BENCH` and run:
//!
//!   ```text
//!   LETS_CHAT_STT_BENCH='whisper=openai,http://127.0.0.1:8090/v1/audio/transcriptions,whisper-1;\
//!                        deepgram=deepgram,https://api.deepgram.com/v1/listen,nova-2,DG_KEY' \
//!     ./dev/cargo test -p lets-chat-server --test stt_comparison -- --ignored --nocapture
//!   ```
//!
//!   Each `;`-separated entry is `label=provider,url,model[,api_key]`
//!   (`provider` is `openai` or `deepgram`). Or use `just stt-bench`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use lets_chat::stt::{
    MockSttClient, ReqwestSttClient, SttClient, SttConfig, SttProvider, SttRequest,
};

/// Latency budget per clip (LC-841's five-second target). Cells above it are
/// flagged in the report so a slow service is visible at a glance.
const LATENCY_BUDGET_MS: u128 = 5000;

// ---- word error rate ----------------------------------------------------

/// Split into comparable word tokens: lowercase, and break on anything that is
/// not a letter or digit, so punctuation and casing never count as errors.
fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect()
}

/// A word-error-rate breakdown of one hypothesis against one reference.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wer {
    ref_words: usize,
    subs: usize,
    dels: usize,
    ins: usize,
}

impl Wer {
    fn errors(&self) -> usize {
        self.subs + self.dels + self.ins
    }

    /// WER = (substitutions + deletions + insertions) / reference length. With
    /// an empty reference there is nothing to be right about: 0.0 when the
    /// hypothesis is also empty, else 1.0 (every hypothesis word is spurious).
    fn rate(&self) -> f64 {
        if self.ref_words == 0 {
            return if self.ins == 0 { 0.0 } else { 1.0 };
        }
        self.errors() as f64 / self.ref_words as f64
    }
}

/// Levenshtein alignment over word tokens, counting the three edit kinds. This
/// is the standard WER computation; the DP matrix is fine for clip-length
/// transcripts (a few hundred words at most).
fn wer(reference: &str, hypothesis: &str) -> Wer {
    let r = tokenize(reference);
    let h = tokenize(hypothesis);
    let (n, m) = (r.len(), h.len());

    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, v) in dp[0].iter_mut().enumerate() {
        *v = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            dp[i][j] = if r[i - 1] == h[j - 1] {
                dp[i - 1][j - 1]
            } else {
                let sub = dp[i - 1][j - 1] + 1;
                let del = dp[i - 1][j] + 1;
                let ins = dp[i][j - 1] + 1;
                sub.min(del).min(ins)
            };
        }
    }

    // Backtrace one optimal path to attribute each error to sub / del / ins.
    let (mut i, mut j) = (n, m);
    let (mut subs, mut dels, mut ins) = (0, 0, 0);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && r[i - 1] == h[j - 1] && dp[i][j] == dp[i - 1][j - 1] {
            i -= 1;
            j -= 1;
        } else if i > 0 && j > 0 && dp[i][j] == dp[i - 1][j - 1] + 1 {
            subs += 1;
            i -= 1;
            j -= 1;
        } else if i > 0 && dp[i][j] == dp[i - 1][j] + 1 {
            dels += 1;
            i -= 1;
        } else {
            ins += 1;
            j -= 1;
        }
    }

    Wer {
        ref_words: n,
        subs,
        dels,
        ins,
    }
}

// ---- fixtures + candidates ----------------------------------------------

/// One recorded clip plus its hand-corrected reference transcript.
struct Fixture {
    name: String,
    content_type: String,
    audio: Vec<u8>,
    reference: String,
}

/// One transcription service under test.
struct Candidate {
    label: String,
    client: Arc<dyn SttClient>,
}

/// One (candidate, fixture) result.
struct Cell {
    candidate: String,
    fixture: String,
    wer: Wer,
    latency_ms: u128,
    error: Option<String>,
}

/// Replay every fixture through every candidate and score the output. Identical
/// bytes go to each service, so the only variable is the service.
async fn run_comparison(candidates: &[Candidate], fixtures: &[Fixture]) -> Vec<Cell> {
    let mut cells = Vec::new();
    for c in candidates {
        for f in fixtures {
            let req =
                SttRequest::new(f.audio.clone(), f.content_type.clone()).with_language(Some("en"));
            let start = Instant::now();
            let res = c.client.transcribe(req).await;
            let latency_ms = start.elapsed().as_millis();
            let (score, error) = match res {
                Ok(r) => (wer(&f.reference, &r.text), None),
                // A failure is a 100%-miss row rather than a hole in the table.
                Err(e) => (wer(&f.reference, ""), Some(format!("{e:?}"))),
            };
            cells.push(Cell {
                candidate: c.label.clone(),
                fixture: f.name.clone(),
                wer: score,
                latency_ms,
                error,
            });
        }
    }
    cells
}

fn cell<'a>(cells: &'a [Cell], candidate: &str, fixture: &str) -> Option<&'a Cell> {
    cells
        .iter()
        .find(|c| c.candidate == candidate && c.fixture == fixture)
}

/// A markdown comparison table: one row per fixture, one column per service,
/// each cell `WER% / latency`, plus per-service means and an over-budget count.
fn render_table(candidates: &[Candidate], fixtures: &[Fixture], cells: &[Cell]) -> String {
    let mut out = String::new();
    let head: Vec<String> = candidates.iter().map(|c| c.label.clone()).collect();
    out.push_str("| fixture | ");
    out.push_str(&head.join(" | "));
    out.push_str(" |\n|---|");
    out.push_str(&"---|".repeat(candidates.len()));
    out.push('\n');

    for f in fixtures {
        out.push_str(&format!("| {} | ", f.name));
        let row: Vec<String> = candidates
            .iter()
            .map(|c| match cell(cells, &c.label, &f.name) {
                Some(x) => {
                    let flag = if x.latency_ms > LATENCY_BUDGET_MS {
                        " OVER"
                    } else {
                        ""
                    };
                    let err = if x.error.is_some() { " ERR" } else { "" };
                    format!(
                        "{:.1}% / {}ms{}{}",
                        x.wer.rate() * 100.0,
                        x.latency_ms,
                        flag,
                        err
                    )
                }
                None => "-".to_string(),
            })
            .collect();
        out.push_str(&row.join(" | "));
        out.push_str(" |\n");
    }

    // Per-service mean WER and mean latency.
    out.push_str("| **mean** | ");
    let means: Vec<String> = candidates
        .iter()
        .map(|c| {
            let mine: Vec<&Cell> = cells.iter().filter(|x| x.candidate == c.label).collect();
            if mine.is_empty() {
                return "-".to_string();
            }
            let n = mine.len() as f64;
            let wer_mean = mine.iter().map(|x| x.wer.rate()).sum::<f64>() / n * 100.0;
            let lat_mean = mine.iter().map(|x| x.latency_ms).sum::<u128>() / mine.len() as u128;
            let over = mine
                .iter()
                .filter(|x| x.latency_ms > LATENCY_BUDGET_MS)
                .count();
            format!("{wer_mean:.1}% / {lat_mean}ms ({over} over budget)")
        })
        .collect();
    out.push_str(&means.join(" | "));
    out.push_str(" |\n");
    out
}

// ---- live-run wiring (opt-in) -------------------------------------------

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stt")
}

fn content_type_for(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "wav" => "audio/wav",
        "webm" => "video/webm",
        "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "audio/mp4",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}

/// Load every fixture directory (one clip + one reference each). Skips a
/// directory missing either half rather than failing the whole run.
fn load_fixtures(dir: &Path) -> Vec<Fixture> {
    let mut fixtures = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return fixtures;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for d in dirs {
        let reference = match std::fs::read_to_string(d.join("reference.txt")) {
            Ok(s) => s.trim().to_string(),
            Err(_) => continue,
        };
        // The single audio file in the directory, whatever its extension.
        let audio_path = std::fs::read_dir(&d).ok().and_then(|es| {
            es.flatten()
                .map(|e| e.path())
                .find(|p| p.file_stem().is_some_and(|s| s == "audio"))
        });
        let Some(audio_path) = audio_path else {
            continue;
        };
        let Ok(audio) = std::fs::read(&audio_path) else {
            continue;
        };
        let ext = audio_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        fixtures.push(Fixture {
            name: d.file_name().unwrap().to_string_lossy().to_string(),
            content_type: content_type_for(ext).to_string(),
            audio,
            reference,
        });
    }
    fixtures
}

/// Build a candidate from one `label=provider,url,model[,api_key]` entry.
fn parse_candidate(entry: &str) -> Option<Candidate> {
    let (label, rest) = entry.split_once('=')?;
    let parts: Vec<&str> = rest.split(',').collect();
    if parts.len() < 3 {
        return None;
    }
    let provider = match parts[0].trim().to_ascii_lowercase().as_str() {
        "deepgram" => SttProvider::Deepgram,
        _ => SttProvider::OpenAi,
    };
    let cfg = SttConfig {
        provider,
        url: parts[1].trim().to_string(),
        model: parts[2].trim().to_string(),
        api_key: parts
            .get(3)
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()),
        prompt: None,
        timeout_secs: 120,
        vad_filter: false,
        // Do not drop low-confidence segments here: the point is to see the raw
        // output, not the production filtering.
        min_logprob: -100.0,
        max_no_speech: 1.0,
        default_language: Some("en".to_string()),
    };
    Some(Candidate {
        label: label.trim().to_string(),
        client: Arc::new(ReqwestSttClient::new(cfg)),
    })
}

fn candidates_from_env() -> Vec<Candidate> {
    std::env::var("LETS_CHAT_STT_BENCH")
        .ok()
        .into_iter()
        .flat_map(|s| s.split(';').map(str::to_string).collect::<Vec<_>>())
        .filter(|e| !e.trim().is_empty())
        .filter_map(|e| parse_candidate(&e))
        .collect()
}

#[tokio::test]
#[ignore]
async fn live_comparison_across_configured_services() {
    let candidates = candidates_from_env();
    if candidates.is_empty() {
        eprintln!(
            "LC-861: set LETS_CHAT_STT_BENCH='label=provider,url,model[,api_key];...' to run. \
             Skipping."
        );
        return;
    }
    let fixtures = load_fixtures(&fixtures_dir());
    assert!(
        !fixtures.is_empty(),
        "no fixtures found in {:?}",
        fixtures_dir()
    );
    let cells = run_comparison(&candidates, &fixtures).await;
    println!("\nLC-861 transcription comparison\n");
    println!("{}", render_table(&candidates, &fixtures, &cells));
}

// ---- deterministic coverage (CI: no audio, no network) ------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_transcript_scores_zero() {
        let w = wer("hello world foo bar", "hello world foo bar");
        assert_eq!(w.errors(), 0);
        assert_eq!(w.rate(), 0.0);
    }

    #[test]
    fn one_substitution_in_four_words_is_25_percent() {
        // The crisp "known-good fixture, expected score" case.
        let w = wer("hello world foo bar", "hello world foo baz");
        assert_eq!(
            w,
            Wer {
                ref_words: 4,
                subs: 1,
                dels: 0,
                ins: 0
            }
        );
        assert_eq!(w.rate(), 0.25);
    }

    #[test]
    fn deletion_and_insertion_are_counted() {
        assert_eq!(
            wer("a b c", "a c"),
            Wer {
                ref_words: 3,
                subs: 0,
                dels: 1,
                ins: 0
            }
        );
        assert_eq!(
            wer("a c", "a b c"),
            Wer {
                ref_words: 2,
                subs: 0,
                dels: 0,
                ins: 1
            }
        );
    }

    #[test]
    fn casing_and_punctuation_are_normalized_away() {
        let w = wer("Hello, world!", "hello world");
        assert_eq!(w.errors(), 0);
    }

    #[test]
    fn empty_reference_edges() {
        assert_eq!(wer("", "").rate(), 0.0);
        assert_eq!(wer("", "spurious words").rate(), 1.0);
    }

    #[tokio::test]
    async fn runner_scores_each_candidate_against_the_same_fixture() {
        let reference = "the meeting starts at nine";
        let fixtures = vec![Fixture {
            name: "single-speaker".into(),
            content_type: "audio/wav".into(),
            audio: vec![1, 2, 3], // ignored by the mock; identical bytes to both
            reference: reference.into(),
        }];
        // One perfect service, one that mishears exactly one word (WER 1/5).
        let candidates = vec![
            Candidate {
                label: "perfect".into(),
                client: Arc::new(MockSttClient::text(reference)),
            },
            Candidate {
                label: "one-off".into(),
                client: Arc::new(MockSttClient::text("the meeting starts at ten")),
            },
        ];

        let cells = run_comparison(&candidates, &fixtures).await;
        assert_eq!(cells.len(), 2);
        assert_eq!(
            cell(&cells, "perfect", "single-speaker")
                .unwrap()
                .wer
                .rate(),
            0.0
        );
        assert_eq!(
            cell(&cells, "one-off", "single-speaker")
                .unwrap()
                .wer
                .rate(),
            0.2
        );
    }

    #[tokio::test]
    async fn runner_measures_latency_and_records_errors() {
        let fixtures = vec![Fixture {
            name: "f".into(),
            content_type: "audio/wav".into(),
            audio: vec![0],
            reference: "anything".into(),
        }];
        let candidates = vec![
            Candidate {
                label: "slow".into(),
                client: Arc::new(MockSttClient::slow("anything", 40)),
            },
            Candidate {
                label: "broken".into(),
                client: Arc::new(MockSttClient::failing_permanently()),
            },
        ];

        let cells = run_comparison(&candidates, &fixtures).await;
        let slow = cell(&cells, "slow", "f").unwrap();
        assert!(
            slow.latency_ms >= 40,
            "measured latency should reflect the 40ms mock delay"
        );
        assert!(slow.error.is_none());
        let broken = cell(&cells, "broken", "f").unwrap();
        assert!(
            broken.error.is_some(),
            "a transcribe failure is recorded, not dropped"
        );
        assert_eq!(
            broken.wer.rate(),
            1.0,
            "a failed clip scores as a total miss"
        );
    }

    #[tokio::test]
    async fn table_is_comparable_and_names_every_service() {
        let fixtures = vec![Fixture {
            name: "single-speaker".into(),
            content_type: "audio/wav".into(),
            audio: vec![1],
            reference: "one two three four".into(),
        }];
        let candidates = vec![
            Candidate {
                label: "svcA".into(),
                client: Arc::new(MockSttClient::text("one two three four")),
            },
            Candidate {
                label: "svcB".into(),
                client: Arc::new(MockSttClient::text("one two three FIVE")),
            },
        ];
        let cells = run_comparison(&candidates, &fixtures).await;
        let table = render_table(&candidates, &fixtures, &cells);
        assert!(table.contains("svcA"), "table names service A");
        assert!(table.contains("svcB"), "table names service B");
        assert!(table.contains("single-speaker"), "table lists the fixture");
        assert!(table.contains("0.0%"), "the perfect service reads 0.0%");
        assert!(table.contains("25.0%"), "the one-error service reads 25.0%");
        assert!(
            table.contains("mean"),
            "table carries a per-service summary"
        );
    }

    #[test]
    fn placeholder_fixtures_are_present_on_disk() {
        // The committed fixtures (single / poor-mic / concurrent) exist and load,
        // each with audio bytes and a non-empty reference. Their WER will be poor
        // (the audio is a synthetic placeholder, see the fixtures README), which
        // is why this asserts presence, not accuracy.
        let fixtures = load_fixtures(&fixtures_dir());
        let names: Vec<&str> = fixtures.iter().map(|f| f.name.as_str()).collect();
        for want in ["single-speaker", "poor-mic", "concurrent-speakers"] {
            assert!(
                names.contains(&want),
                "missing fixture {want}; have {names:?}"
            );
        }
        for f in &fixtures {
            assert!(!f.audio.is_empty(), "{} has audio bytes", f.name);
            assert!(!f.reference.is_empty(), "{} has a reference", f.name);
        }
    }
}
