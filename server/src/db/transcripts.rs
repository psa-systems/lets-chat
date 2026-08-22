//! LC-393: persistence for call transcription sessions (Phase 1, 1:1 DM calls).
//!
//! A session is opened when a call participant turns transcription on and
//! closed on hangup (or by the WS-disconnect backstop). Segments are appended
//! as each participant's browser emits a final speech result; the speaker is
//! the poster. See `migrations/chat/0066_call_transcripts.sql`.

use sqlx::{Row, SqlitePool};

/// Hard cap on a single segment's text. A speech result is a sentence or two;
/// 4 KiB is generous headroom while bounding what a client can push per POST
/// (the body is stored and later rendered as PLAIN TEXT, not markdown, so this
/// is the only length bound needed - cf. the LC-153 markdown caps).
pub const MAX_SEGMENT_CHARS: usize = 4_000;

/// LC-752: `started_at` / `spoken_at` written with millisecond precision. The
/// columns default to `datetime('now')` (whole seconds), which quantized the
/// exported VTT cue offset (`spoken_at - started_at`) to whole seconds, so a
/// segment 1.4s into a call exported as if it were at 1.0s. Milliseconds match
/// the resolution `duration_ms` already carries.
const NOW_MS: &str = "strftime('%Y-%m-%d %H:%M:%f', 'now')";

/// One transcription session.
#[derive(Debug, Clone)]
pub struct Transcript {
    pub id: i64,
    pub room_id: i64,
    pub started_by: String,
    /// `YYYY-MM-DD HH:MM:SS.mmm` (LC-752); rows written before that are
    /// whole-second `YYYY-MM-DD HH:MM:SS`.
    pub started_at: String,
    pub ended_at: Option<String>,
    pub status: String,
    /// LC-396: cached LLM summary (markdown), `None` until generated.
    pub summary: Option<String>,
}

/// One stored speech result.
#[derive(Debug, Clone)]
pub struct Segment {
    pub id: i64,
    pub transcript_id: i64,
    pub user_id: String,
    /// The DISPLAY text: LC-629 correction output when an LLM is configured,
    /// else the raw recognition. This is what the live caption + saved page show.
    pub text: String,
    /// LC-629: the original, uncorrected recognition when it differs from
    /// `text`; `None` when no correction was applied (raw equals display).
    pub raw_text: Option<String>,
    /// Same format as [`Transcript::started_at`] (LC-752).
    pub spoken_at: String,
    /// LC-591: real spoken length in ms from the engine's segment timings, or 0
    /// when unknown (browser Web Speech, or a non-verbose_json engine).
    pub duration_ms: i64,
}

impl Segment {
    /// LC-629: the raw, uncorrected recognition - `raw_text` when a correction
    /// replaced it, otherwise the display `text` (which is itself the raw text).
    pub fn raw(&self) -> &str {
        self.raw_text.as_deref().unwrap_or(&self.text)
    }
}

fn map_transcript(row: &sqlx::sqlite::SqliteRow) -> Transcript {
    Transcript {
        id: row.get("id"),
        room_id: row.get("room_id"),
        started_by: row.get("started_by"),
        started_at: row.get("started_at"),
        ended_at: row.get("ended_at"),
        status: row.get("status"),
        summary: row.get("summary"),
    }
}

/// Return the room's currently-open session, if any.
pub async fn open_session_for_room(
    pool: &SqlitePool,
    room_id: i64,
) -> sqlx::Result<Option<Transcript>> {
    let row = sqlx::query(
        "SELECT id, room_id, started_by, started_at, ended_at, status, summary \
         FROM call_transcripts WHERE room_id = ? AND status = 'active' \
         ORDER BY id DESC LIMIT 1",
    )
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(map_transcript))
}

/// Open a session for a call, or return the one already open for the room so a
/// second participant toggling transcription joins the existing session rather
/// than forking a duplicate.
pub async fn start_session(
    pool: &SqlitePool,
    room_id: i64,
    started_by: &str,
) -> sqlx::Result<Transcript> {
    if let Some(existing) = open_session_for_room(pool, room_id).await? {
        return Ok(existing);
    }
    let id = sqlx::query(&format!(
        "INSERT INTO call_transcripts (room_id, started_by, started_at) VALUES (?, ?, {NOW_MS})"
    ))
    .bind(room_id)
    .bind(started_by)
    .execute(pool)
    .await?
    .last_insert_rowid();
    get(pool, id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// Fetch a session by id.
pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<Transcript>> {
    let row = sqlx::query(
        "SELECT id, room_id, started_by, started_at, ended_at, status, summary \
         FROM call_transcripts WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(map_transcript))
}

/// LC-396: cap on a stored summary. It is rendered through the markdown
/// pipeline (LC-153), so the input must be bounded on the write path.
pub const MAX_SUMMARY_CHARS: usize = 16_000;

/// Store (or replace) a transcript's LLM summary, truncated to the cap.
pub async fn set_summary(pool: &SqlitePool, id: i64, summary: &str) -> sqlx::Result<()> {
    let capped: String = summary.chars().take(MAX_SUMMARY_CHARS).collect();
    sqlx::query("UPDATE call_transcripts SET summary = ? WHERE id = ?")
        .bind(&capped)
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// Append a final speech result. `text` is the recognized caption, stored
/// verbatim. `raw_text` is a legacy second copy of the recognition (retained for
/// transcripts written while the LC-629 live-correction pass existed); new
/// segments always pass `None`, leaving the column NULL. Both are truncated to
/// [`MAX_SEGMENT_CHARS`] on the char boundary. Returns the new segment id.
pub async fn append_segment(
    pool: &SqlitePool,
    transcript_id: i64,
    user_id: &str,
    text: &str,
    raw_text: Option<&str>,
    duration_ms: i64,
) -> sqlx::Result<i64> {
    let capped: String = text.chars().take(MAX_SEGMENT_CHARS).collect();
    let capped_raw: Option<String> = raw_text.map(|r| r.chars().take(MAX_SEGMENT_CHARS).collect());
    let id = sqlx::query(&format!(
        "INSERT INTO transcript_segments \
         (transcript_id, user_id, text, raw_text, duration_ms, spoken_at) \
         VALUES (?, ?, ?, ?, ?, {NOW_MS})"
    ))
    .bind(transcript_id)
    .bind(user_id)
    .bind(&capped)
    .bind(&capped_raw)
    .bind(duration_ms.max(0))
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

/// All segments of a session, oldest first.
pub async fn list_segments(pool: &SqlitePool, transcript_id: i64) -> sqlx::Result<Vec<Segment>> {
    let rows = sqlx::query(
        "SELECT id, transcript_id, user_id, text, raw_text, spoken_at, duration_ms \
         FROM transcript_segments WHERE transcript_id = ? ORDER BY id ASC",
    )
    .bind(transcript_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Segment {
            id: row.get("id"),
            transcript_id: row.get("transcript_id"),
            user_id: row.get("user_id"),
            text: row.get("text"),
            raw_text: row.get("raw_text"),
            spoken_at: row.get("spoken_at"),
            duration_ms: row.get("duration_ms"),
        })
        .collect())
}

/// A transcript joined with its room, for the archive list.
#[derive(Debug, Clone)]
pub struct TranscriptListRow {
    pub id: i64,
    pub room_id: i64,
    pub room_name: String,
    pub room_type: String,
    pub is_voice: bool,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub status: String,
    pub segment_count: i64,
}

/// LC-394: the most recent transcripts across all rooms (newest first), joined
/// with the room + a segment count. The caller filters by per-room access (the
/// archive page gates each row with `is_room_accessible`), so this stays a
/// simple bounded scan rather than encoding the access rules in SQL.
///
/// LC-395: when `query` is `Some`, restrict to transcripts containing a segment
/// matching it (case-insensitive, LIKE-escaped) - the archive search.
pub async fn list_recent(
    pool: &SqlitePool,
    query: Option<&str>,
    limit: i64,
) -> sqlx::Result<Vec<TranscriptListRow>> {
    let base = "SELECT t.id, t.room_id, t.started_at, t.ended_at, t.status, \
                r.name AS room_name, r.room_type, r.is_voice, \
                (SELECT COUNT(*) FROM transcript_segments s WHERE s.transcript_id = t.id) AS segment_count \
         FROM call_transcripts t JOIN rooms r ON r.id = t.room_id";
    let pattern = query.map(|q| {
        let escaped = q
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        format!("%{escaped}%")
    });
    let sql = if pattern.is_some() {
        format!(
            "{base} WHERE EXISTS (SELECT 1 FROM transcript_segments s \
             WHERE s.transcript_id = t.id AND s.text LIKE ? ESCAPE '\\' COLLATE NOCASE) \
             ORDER BY t.id DESC LIMIT ?"
        )
    } else {
        format!("{base} ORDER BY t.id DESC LIMIT ?")
    };
    let mut q = sqlx::query(&sql);
    if let Some(p) = &pattern {
        q = q.bind(p);
    }
    let rows = q.bind(limit).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|row| TranscriptListRow {
            id: row.get("id"),
            room_id: row.get("room_id"),
            room_name: row.get("room_name"),
            room_type: row.get("room_type"),
            is_voice: row.get::<i64, _>("is_voice") != 0,
            started_at: row.get("started_at"),
            ended_at: row.get("ended_at"),
            status: row.get("status"),
            segment_count: row.get("segment_count"),
        })
        .collect())
}

/// LC-441: delete a transcript and its segments. Done in one transaction and
/// deleting both tables explicitly so it works regardless of the connection's
/// `foreign_keys` pragma (the `ON DELETE CASCADE` on `transcript_segments` only
/// fires when foreign keys are enabled). Returns `true` if a row was removed.
pub async fn delete(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM transcript_segments WHERE transcript_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    let res = sqlx::query("DELETE FROM call_transcripts WHERE id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(res.rows_affected() > 0)
}

/// Close a session. Returns `true` only when this call actually transitioned an
/// active session to ended (so the caller posts the "transcript saved" notice
/// exactly once even if both an explicit /end and the disconnect backstop race).
pub async fn end_session(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE call_transcripts SET status = 'ended', ended_at = datetime('now') \
         WHERE id = ? AND status = 'active'",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Backstop for a hard WS drop that never sent an explicit /end: close every
/// session this user started that is still open, returning their ids so the
/// caller can finalize each (post the notice, broadcast the end). Mirrors
/// `remote_control_audit::end_sessions_for_user`.
pub async fn end_open_sessions_started_by(
    pool: &SqlitePool,
    user_id: &str,
) -> sqlx::Result<Vec<i64>> {
    // Capture the ids first so we can report exactly which sessions WE closed
    // (a concurrent finalizer may close others between the SELECT and UPDATE;
    // the per-row UPDATE guard on status keeps that race safe).
    let rows =
        sqlx::query("SELECT id FROM call_transcripts WHERE started_by = ? AND status = 'active'")
            .bind(user_id)
            .fetch_all(pool)
            .await?;
    let mut closed = Vec::new();
    for row in rows {
        let id: i64 = row.get("id");
        if end_session(pool, id).await? {
            closed.push(id);
        }
    }
    Ok(closed)
}
