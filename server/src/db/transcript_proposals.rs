//! LC-1135: proposed YouTrack issues drawn from a call transcript.
//!
//! Generation (`routes::transcripts::propose_issues`) parses the LLM output
//! into [`NewProposal`]s and calls [`replace_unfiled`], which swaps out the
//! transcript's still-`proposed` rows while leaving `filed` / `dismissed` ones
//! untouched. Filing (`routes::transcripts::file_issues`) reads a row with
//! [`get`], creates the YouTrack issue, and calls [`mark_filed`], which is a
//! no-op on a row that is no longer `proposed` (so a double submit cannot
//! file twice).

use sqlx::{Row, SqlitePool};

/// A stored proposal row.
#[derive(Debug, Clone)]
pub struct IssueProposal {
    pub id: i64,
    pub transcript_id: i64,
    pub summary: String,
    pub assignee_name: Option<String>,
    pub source_lines: String,
    pub status: String,
    pub youtrack_issue_id: Option<String>,
    pub filed_by: Option<String>,
    pub filed_at: Option<String>,
    pub created_at: String,
}

/// A freshly parsed proposal, before it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProposal {
    pub summary: String,
    pub assignee_name: Option<String>,
    pub source_lines: String,
}

/// Replace the transcript's still-`proposed` rows with `proposals`, in one
/// transaction. Rows that are `filed` or `dismissed` are left in place, so a
/// regenerate never loses or re-files an accepted proposal.
pub async fn replace_unfiled(
    pool: &SqlitePool,
    transcript_id: i64,
    proposals: &[NewProposal],
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM transcript_issue_proposals WHERE transcript_id = ? AND status = 'proposed'")
        .bind(transcript_id)
        .execute(&mut *tx)
        .await?;
    for p in proposals {
        sqlx::query(
            "INSERT INTO transcript_issue_proposals \
             (transcript_id, summary, assignee_name, source_lines, status) \
             VALUES (?, ?, ?, ?, 'proposed')",
        )
        .bind(transcript_id)
        .bind(&p.summary)
        .bind(p.assignee_name.as_deref())
        .bind(&p.source_lines)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

fn row_to_proposal(r: &sqlx::sqlite::SqliteRow) -> IssueProposal {
    IssueProposal {
        id: r.get("id"),
        transcript_id: r.get("transcript_id"),
        summary: r.get("summary"),
        assignee_name: r.get("assignee_name"),
        source_lines: r.get("source_lines"),
        status: r.get("status"),
        youtrack_issue_id: r.get("youtrack_issue_id"),
        filed_by: r.get("filed_by"),
        filed_at: r.get("filed_at"),
        created_at: r.get("created_at"),
    }
}

/// Every proposal for one transcript, oldest first (stable review order).
pub async fn list_for_transcript(
    pool: &SqlitePool,
    transcript_id: i64,
) -> sqlx::Result<Vec<IssueProposal>> {
    let rows = sqlx::query(
        "SELECT id, transcript_id, summary, assignee_name, source_lines, status, \
         youtrack_issue_id, filed_by, filed_at, created_at \
         FROM transcript_issue_proposals WHERE transcript_id = ? ORDER BY id",
    )
    .bind(transcript_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_proposal).collect())
}

/// One proposal by id.
pub async fn get(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<IssueProposal>> {
    let row = sqlx::query(
        "SELECT id, transcript_id, summary, assignee_name, source_lines, status, \
         youtrack_issue_id, filed_by, filed_at, created_at \
         FROM transcript_issue_proposals WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_proposal))
}

/// Mark a proposal `filed` with the created issue key and filer. Updates ONLY
/// when the row is still `proposed`, so a concurrent or repeated file cannot
/// flip it twice; returns true iff a row changed.
pub async fn mark_filed(
    pool: &SqlitePool,
    id: i64,
    youtrack_issue_id: &str,
    filed_by: &str,
) -> sqlx::Result<bool> {
    let res = sqlx::query(
        "UPDATE transcript_issue_proposals \
         SET status = 'filed', youtrack_issue_id = ?, filed_by = ?, filed_at = datetime('now') \
         WHERE id = ? AND status = 'proposed'",
    )
    .bind(youtrack_issue_id)
    .bind(filed_by)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn chat_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations/chat")
            .run(&pool)
            .await
            .unwrap();
        pool
    }

    fn np(summary: &str, assignee: Option<&str>, lines: &str) -> NewProposal {
        NewProposal {
            summary: summary.to_string(),
            assignee_name: assignee.map(|s| s.to_string()),
            source_lines: lines.to_string(),
        }
    }

    #[tokio::test]
    async fn replace_unfiled_then_list_round_trips() {
        let pool = chat_pool().await;
        replace_unfiled(
            &pool,
            7,
            &[
                np("Ship the thing", Some("Vas"), "Vas: I'll ship it"),
                np("Write the doc", None, "Ada: someone should document it"),
            ],
        )
        .await
        .unwrap();
        let rows = list_for_transcript(&pool, 7).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].summary, "Ship the thing");
        assert_eq!(rows[0].assignee_name.as_deref(), Some("Vas"));
        assert_eq!(rows[0].status, "proposed");
        assert_eq!(rows[1].assignee_name, None);
    }

    #[tokio::test]
    async fn replace_unfiled_keeps_filed_rows() {
        let pool = chat_pool().await;
        replace_unfiled(&pool, 7, &[np("Keep me", None, "")])
            .await
            .unwrap();
        let id = list_for_transcript(&pool, 7).await.unwrap()[0].id;
        assert!(mark_filed(&pool, id, "LC-1", "u1").await.unwrap());
        // Regenerate: a fresh proposal set replaces only the proposed rows.
        replace_unfiled(&pool, 7, &[np("New proposal", None, "")])
            .await
            .unwrap();
        let rows = list_for_transcript(&pool, 7).await.unwrap();
        let summaries: Vec<&str> = rows.iter().map(|r| r.summary.as_str()).collect();
        assert!(summaries.contains(&"Keep me"), "filed row survives regenerate");
        assert!(summaries.contains(&"New proposal"));
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn mark_filed_sets_fields_and_is_idempotent() {
        let pool = chat_pool().await;
        replace_unfiled(&pool, 7, &[np("Once", None, "")])
            .await
            .unwrap();
        let id = list_for_transcript(&pool, 7).await.unwrap()[0].id;
        assert!(mark_filed(&pool, id, "LC-42", "alice").await.unwrap());
        // Second file attempt on the same row is a no-op.
        assert!(!mark_filed(&pool, id, "LC-99", "bob").await.unwrap());
        let row = get(&pool, id).await.unwrap().unwrap();
        assert_eq!(row.status, "filed");
        assert_eq!(row.youtrack_issue_id.as_deref(), Some("LC-42"));
        assert_eq!(row.filed_by.as_deref(), Some("alice"));
        assert!(row.filed_at.is_some());
    }
}
