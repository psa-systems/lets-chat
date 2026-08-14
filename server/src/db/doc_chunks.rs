//! LC-712: storage for documentation chunks used by the AI help desk
//! (`/support`). One row per chunk of a product docs page. Vectors are
//! little-endian `f32` BLOBs (`embeddings::vec_to_bytes`), same as
//! `message_embeddings`; ranking loads the corpus and cosine-scans it in Rust
//! (`embeddings::cosine_similarity`), which is fine at self-host doc-set scale.
//!
//! A page's chunks all share one `content_hash` (the SHA-256 of the page's
//! extracted text), so a refresh can cheaply skip an unchanged page. Chunks are
//! upserted by `(source_url, chunk_index)`; [`delete_by_source`] clears a page
//! before re-insert so a page that lost sections leaves no stale trailing rows.

use sqlx::{Row, SqlitePool};

/// One stored doc chunk decoded for ranking + citation. `vec` is the decoded
/// embedding; `product`, `title`, and `source_url` render the citation.
pub struct DocChunk {
    pub product: String,
    pub source_url: String,
    pub title: String,
    pub heading: String,
    pub body: String,
    pub vec: Vec<f32>,
}

/// Insert or replace one chunk keyed by `(source_url, chunk_index)`. `vec` is the
/// little-endian byte encoding from [`crate::embeddings::vec_to_bytes`]; `dim` is
/// its length.
#[allow(clippy::too_many_arguments)]
pub async fn upsert(
    pool: &SqlitePool,
    product: &str,
    source_url: &str,
    title: &str,
    heading: &str,
    chunk_index: i64,
    body: &str,
    content_hash: &str,
    dim: i64,
    vec: &[u8],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO doc_chunks \
           (product, source_url, title, heading, chunk_index, body, content_hash, dim, vec) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(source_url, chunk_index) DO UPDATE SET \
           product = excluded.product, title = excluded.title, heading = excluded.heading, \
           body = excluded.body, content_hash = excluded.content_hash, dim = excluded.dim, \
           vec = excluded.vec, updated_at = datetime('now')",
    )
    .bind(product)
    .bind(source_url)
    .bind(title)
    .bind(heading)
    .bind(chunk_index)
    .bind(body)
    .bind(content_hash)
    .bind(dim)
    .bind(vec)
    .execute(pool)
    .await?;
    Ok(())
}

/// The `content_hash` stored for a page (any chunk of it), if the page is
/// already indexed. Used to skip re-embedding a page whose text is unchanged.
pub async fn source_content_hash(
    pool: &SqlitePool,
    source_url: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row = sqlx::query("SELECT content_hash FROM doc_chunks WHERE source_url = ? LIMIT 1")
        .bind(source_url)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.get("content_hash")))
}

/// Remove every chunk of one source page (called before re-inserting its fresh
/// chunks, so a shrunk page leaves no stale trailing rows).
pub async fn delete_by_source(pool: &SqlitePool, source_url: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM doc_chunks WHERE source_url = ?")
        .bind(source_url)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove every chunk of a product (called when a product is dropped from the
/// configured sources, so its docs stop being retrievable).
pub async fn delete_by_product(pool: &SqlitePool, product: &str) -> Result<u64, sqlx::Error> {
    let res = sqlx::query("DELETE FROM doc_chunks WHERE product = ?")
        .bind(product)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Load the whole corpus decoded for ranking. `product` limits the scan to one
/// product; `None` scans across all products. At self-host doc-set scale this is
/// a cheap full load + in-Rust cosine scan (mirrors `message_embeddings`).
pub async fn list_for_scan(
    pool: &SqlitePool,
    product: Option<&str>,
) -> Result<Vec<DocChunk>, sqlx::Error> {
    let rows = match product {
        Some(p) => {
            sqlx::query(
                "SELECT product, source_url, title, heading, body, vec FROM doc_chunks \
                 WHERE product = ?",
            )
            .bind(p)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query("SELECT product, source_url, title, heading, body, vec FROM doc_chunks")
                .fetch_all(pool)
                .await?
        }
    };
    Ok(rows
        .into_iter()
        .map(|r| {
            let bytes: Vec<u8> = r.get("vec");
            DocChunk {
                product: r.get("product"),
                source_url: r.get("source_url"),
                title: r.get("title"),
                heading: r.get("heading"),
                body: r.get("body"),
                vec: crate::embeddings::bytes_to_vec(&bytes),
            }
        })
        .collect())
}

/// Total number of indexed chunks (shown on the admin settings page).
pub async fn count(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM doc_chunks")
        .fetch_one(pool)
        .await?;
    Ok(row.get("n"))
}

/// Distinct product names currently indexed (shown on the admin settings page).
pub async fn distinct_products(pool: &SqlitePool) -> Result<Vec<String>, sqlx::Error> {
    let rows = sqlx::query("SELECT DISTINCT product FROM doc_chunks ORDER BY product")
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().map(|r| r.get("product")).collect())
}
