# Phase 23 - Upload hygiene (thumbnails + EXIF stripping + orphan GC)

## Goal

Three deferred items from phase 13 land as one coherent upload-hygiene phase:

1. **Image thumbnailing.** Every accepted image upload produces a `~360px` preview written next to the original at `${LETS_CHAT_DATA_DIR}/uploads/{sha256}_preview.{ext}`. Inline message rendering uses the preview; clicking opens the original.
2. **EXIF / metadata stripping.** Every accepted image is re-encoded through the `image` crate before storage, dropping EXIF / XMP / IPTC / PNG text chunks / WebP metadata. The stripped version is what gets stored at `{sha256}.{ext}` (the sha256 is computed AFTER strip, so the dedup key reflects the cleaned bytes).
3. **Orphan upload GC.** Hourly background sweep deletes `file_uploads` rows where `message_id IS NULL` older than 24 hours. Dedup-aware: the on-disk file is removed only when no other row references the same `storage_path`. Same sweep underlies a "purge now" admin button.

All three subfeatures share one image-processing pipeline and one storage layout. The phase adds the `image` crate, a `server/src/uploads/` module for non-route/non-DB upload code, and admin surfaces for force-regenerate-thumbnails, purge-orphans-now, and a disk-usage panel.

## Architecture

- **Image pipeline (`server/src/uploads/pipeline.rs`).** Single function `process_image(tmp_path, mime) -> ProcessedImage` that decodes via `image::ImageReader`, produces two byte buffers: a stripped re-encoded original and a stripped re-encoded preview (max-dimension 360px, aspect-preserving). Re-encode quality for JPEG is q=92; PNG/GIF/WebP are lossless. Animated GIFs preserve their animation in the stored original via a multi-frame re-encode (which is what does the metadata strip); the preview is the first frame as a static GIF. Inline-render animation is a distraction trap, so click-through is where the animation lives. PDFs and any non-image MIME bypass the pipeline entirely. Returned `ProcessedImage` carries `(stripped_original_bytes, stripped_preview_bytes, final_mime)`; the caller writes both to disk and updates `size_bytes` from the stripped original length.
- **Upload handler integration.** `post_upload` in `server/src/routes/uploads.rs` keeps the existing temp-file + `infer` magic-byte sniff. After the sniff confirms an image MIME, it acquires a permit from a process-wide `Semaphore(THUMBNAIL_CONCURRENCY)`, calls `process_image`, writes the stripped original bytes to `{sha256}.{ext}` (sha256 computed from stripped bytes), writes the preview to `{sha256}_preview.{ext}`, updates `size_bytes` to the post-strip length, then inserts the `file_uploads` row. Non-image MIMEs (PDF) skip the pipeline and follow the existing path unchanged.
- **Serve route.** `GET /api/files/:id` accepts an optional `?size=preview` query parameter. When present and the upload is an image MIME, the handler computes the preview path (replace `.ext` with `_preview.ext`) and serves that file. If the preview is missing on disk (failed-to-generate or pre-phase row), fall back to the original. Browsers cache `?size=preview` and the no-query URL as distinct entries; both are content-addressed so the existing `Cache-Control: private, max-age=86400` stays correct.
- **Template integration.** `templates/partials/attachment.html` swaps `src="{{ a.url }}"` to `src="{{ a.url }}?size=preview"` for image attachments, and wraps the `<img>` in an `<a href="{{ a.url }}" target="_blank">` so click opens the original. No other template changes; the partial is the single render site for both first-paint and WS-broadcast paths.
- **Orphan sweeper (`server/src/uploads/sweep.rs`).** `run_orphan_sweep(state, threshold_hours)` selects all `file_uploads` rows where `message_id IS NULL AND created_at < datetime('now', '-N hours')`. For each row: count other rows referencing the same `storage_path` (`SELECT COUNT(*) FROM file_uploads WHERE storage_path = ? AND id <> ?`); if zero, delete the original file and the preview file (treat "file doesn't exist" as success). Then delete the row. The count-then-delete pair runs inside a single transaction so two GC processes can't both decide "no other references" and double-delete. Single-process invariant documented in a comment at the function head. Errors per row log a warning and continue to the next row; the next tick retries.
- **Scheduler.** `spawn_orphan_sweeper(state)` mirrors `spawn_idle_scanner` and `spawn_digest_sender` in `server/src/main.rs`: bare `tokio::spawn`, `tokio::time::interval(Duration::from_secs(3600))`, skip the immediate fire, log-and-continue on error. Wired into `main()` alongside the existing two spawns. Threshold is a `const ORPHAN_AGE_HOURS: i64 = 24` at the top of `sweep.rs`.
- **Admin surfaces.** Three additions to `server/src/routes/admin.rs`:
  - `POST /admin/uploads/regenerate-thumbnails` streams through `file_uploads WHERE mime_type LIKE 'image/%'`, for each row reads the on-disk original, runs the preview half of `process_image`, writes `{sha256}_preview.{ext}` if missing or `--force` flag present. Returns `303 Redirect` to `/admin/settings` with a flash count.
  - `POST /admin/uploads/purge-orphans` calls `run_orphan_sweep` with `threshold_hours = 0`. Same redirect.
  - `GET /admin/settings` (existing route) gains a small "Uploads" panel block: total bytes summed from `file_uploads.size_bytes`, count of rows where `message_id IS NULL`. Scoped tight: no MIME breakdown, no averages, no charts. Two buttons (regenerate, purge) live in the same panel.
- **No new migrations.** The orphan-GC index already exists at `server/migrations/chat/0012_uploads.sql:14`. No schema changes. No new settings rows (EXIF stripping is unconditional, no operator toggle).

## Tech Stack

- New crate: `image = { version = "0.25", default-features = false, features = ["jpeg", "png", "gif", "webp"] }`. Narrow feature set: matches the existing `allowed_ext_for_mime` allowlist (jpg/png/gif/webp/pdf), drops TIFF/BMP/AVIF/HDR/etc. to keep binary size and dependency surface bounded.
- New dev-dependency: `kamadak-exif = "0.5"`. Used only in tests to verify EXIF was actually stripped. Production code never reads EXIF.
- `tokio::sync::Semaphore` for `THUMBNAIL_CONCURRENCY = 4`. Already a transitive dependency via `tokio`'s default features; no Cargo change needed.
- No filesystem layout change: thumbnails are adjacent files in the existing `${LETS_CHAT_DATA_DIR}/uploads/` directory.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Edit | `server/Cargo.toml` | Add `image` (narrowed features), add `kamadak-exif` as `[dev-dependencies]`. |
| Add | `server/src/uploads/mod.rs` | Module root. `pub mod pipeline; pub mod sweep;`. Defines `pub const THUMBNAIL_CONCURRENCY: usize = 4;` and the process-wide `Semaphore` (singleton via `OnceLock`, mirroring `server/src/push/mod.rs:148`). |
| Add | `server/src/uploads/pipeline.rs` | `process_image(tmp_path, sniffed_mime) -> Result<ProcessedImage, PipelineError>`. Decodes once, produces stripped original bytes and stripped preview bytes. JPEG re-encode at q=92; PNG/GIF/WebP lossless. GIF collapses to first frame. Owns the format-matching logic and the 360px max-dimension thumbnail math. |
| Add | `server/src/uploads/sweep.rs` | `run_orphan_sweep(state, threshold_hours) -> Result<SweepStats, sqlx::Error>`. Dedup-aware per-row delete inside a transaction. Returns `(rows_deleted, files_deleted)` for logging and the admin flash. Single-process invariant documented in module comment. |
| Edit | `server/src/lib.rs` | `pub mod uploads;` (alongside existing `pub mod digest;` etc.). |
| Edit | `server/src/routes/uploads.rs` | In `post_upload`: after `infer` sniff, branch on `mime.starts_with("image/")`. Image branch: acquire semaphore permit, call `pipeline::process_image`, write stripped original + preview, sha256 the STRIPPED bytes, insert row with post-strip `size_bytes`. Non-image branch: existing path unchanged. In `get_file`: parse optional `?size=preview` from `axum::extract::Query`, swap to preview path for image MIMEs, fall back to original on missing-file. |
| Edit | `server/src/routes/admin.rs` | Add `post_regenerate_thumbnails`, `post_purge_orphans`. Route them in `routes()` alongside existing admin routes. Augment `get_settings` to pass `uploads_total_bytes` and `uploads_orphan_count` into `AdminSettingsView`. |
| Edit | `server/src/views/admin.rs` | Add `uploads_total_bytes: i64` and `uploads_orphan_count: i64` to `AdminSettingsView`. |
| Edit | `server/src/db/uploads.rs` | Add `count_uploads_sharing_path(pool, storage_path, exclude_id) -> i64`, `delete_upload_row(pool, id)`, `select_orphans_older_than(pool, hours)`, `iter_image_uploads(pool) -> stream` for the regen action, `sum_size_bytes(pool) -> i64`, `count_orphans(pool) -> i64`. Group these under a `// ── Orphan GC + admin queries ──` comment block. |
| Edit | `server/src/main.rs` | Add `spawn_orphan_sweeper(state.clone());` next to the existing two spawns. Add the function definition modeled on `spawn_digest_sender`. |
| Edit | `server/templates/partials/attachment.html` | For image MIMEs, change `<img src="{{ a.url }}">` to `<a href="{{ a.url }}" target="_blank" rel="noopener"><img src="{{ a.url }}?size=preview" ...></a>`. Non-image (PDF) branch unchanged. |
| Edit | `server/templates/admin/settings.html` | Add the "Uploads" panel: total size, orphan count, "Regenerate thumbnails" form, "Purge orphans now" form. Compact, two buttons, no chart. |
| Add | `server/tests/uploads_pipeline.rs` | Unit-ish tests for `process_image`: JPEG with synthetic EXIF GPS block in, verify `kamadak-exif::Reader` finds no fields in the output. PNG with `tEXt` chunk in, verify gone. WebP with EXIF block in, verify gone. Preview dimension check: input 1200x800 outputs preview with longer side == 360. GIF input produces a static thumbnail (single frame). |
| Add | `server/tests/uploads_sweep.rs` | The centerpiece dedup-aware GC test (see Task 7). Plus: orphans younger than threshold survive; linked rows are never touched; missing file is treated as success; preview file is removed alongside original. |
| Edit | `server/tests/routes_uploads.rs` | Add: upload a JPEG, GET `?size=preview` returns smaller-dimensioned bytes; upload a PDF, GET `?size=preview` falls back to original (200 with PDF body); upload an image whose decoded form fails (truncated bytes that pass `infer` but fail `image::ImageReader::decode`), confirm the upload is rejected with 4xx (we treat decode-failure-after-sniff as accept-with-fallback per the design discussion - revisit during Task 3 if it turns out the rejection is cleaner). Preview-regeneration-on-dedup-hit test: insert a row with no `_preview` file on disk, upload the same content as another user, confirm preview file now exists. |
| Add | `server/tests/admin_uploads.rs` | Admin force-regenerate runs over a missing-preview row and creates it. Admin purge-now removes a stale orphan. Anonymous and non-admin calls to both endpoints return 403/401 as the existing admin routes do. |

## Tasks

### Task 1 - Dependencies + module skeleton

- [ ] Add to `server/Cargo.toml` `[dependencies]`:
  ```toml
  image = { version = "0.25", default-features = false, features = ["jpeg", "png", "gif", "webp"] }
  ```
- [ ] Add to `server/Cargo.toml` `[dev-dependencies]` (create the table if missing):
  ```toml
  kamadak-exif = "0.5"
  ```
- [ ] Create `server/src/uploads/mod.rs` with the module declarations and the semaphore singleton modeled on `server/src/push/mod.rs:148-160`:
  ```rust
  pub mod pipeline;
  pub mod sweep;

  /// Process-wide cap on concurrent image-pipeline tasks. Decode + re-encode
  /// is memory-hungry; without a cap a burst of large uploads could OOM the
  /// server. Tune here if operator-side load tells a different story.
  pub const THUMBNAIL_CONCURRENCY: usize = 4;

  static SEM: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
  pub fn thumbnail_semaphore() -> &'static tokio::sync::Semaphore {
      SEM.get_or_init(|| tokio::sync::Semaphore::new(THUMBNAIL_CONCURRENCY))
  }
  ```
- [ ] Create stub `server/src/uploads/pipeline.rs` and `server/src/uploads/sweep.rs` so `cargo check` is happy.
- [ ] Add `pub mod uploads;` to `server/src/lib.rs`.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git checkout -b feat/upload-hygiene`
- [ ] `git add server/Cargo.toml Cargo.lock server/src/uploads/mod.rs server/src/uploads/pipeline.rs server/src/uploads/sweep.rs server/src/lib.rs`
- [ ] `git commit -m "feat(uploads): add image crate + uploads module skeleton"`

### Task 2 - Image pipeline (thumbnail + EXIF strip)

- [ ] Implement `pipeline::process_image(tmp_path: &Path, sniffed_mime: &str) -> Result<ProcessedImage, PipelineError>` in `server/src/uploads/pipeline.rs`:
  - `ProcessedImage { original_bytes: Vec<u8>, preview_bytes: Vec<u8>, mime: String }`.
  - Decode via `image::ImageReader::open(tmp_path)?.with_guessed_format()?.decode()`. Map decode errors to `PipelineError::Decode`.
  - Strip pass for the original: re-encode the decoded `DynamicImage` to a `Vec<u8>` using the format implied by `sniffed_mime`. Re-encoding through `image` drops EXIF/XMP/IPTC/PNG text chunks by default; that is the entire EXIF-strip mechanism.
  - JPEG path uses `image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92)`.
  - PNG path uses default `PngEncoder` (lossless).
  - WebP path uses `image::codecs::webp::WebPEncoder::new_lossless(&mut buf)`.
  - GIF path: decode collapses to first frame via `DynamicImage`. Re-encode as a single-frame GIF.
  - Thumbnail: `decoded.thumbnail(360, 360)` (preserves aspect ratio, longer side capped at 360). Encode using the same format as the original.
  - Return both buffers and the final mime.
- [ ] Define `PipelineError` as a thiserror enum: `Decode`, `Encode`, `Io`. Implement `From<image::ImageError>` and `From<std::io::Error>`.
- [ ] One inline comment at the top of `process_image` documenting the "re-encode IS the EXIF strip" semantics so future readers don't add a redundant strip step. One line at the JPEG encoder documenting `q=92`. No other comments.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/uploads/pipeline.rs`
- [ ] `git commit -m "feat(uploads): image pipeline (re-encode strips EXIF, 360px preview)"`

### Task 3 - Wire pipeline into POST /api/upload

- [ ] Edit `server/src/routes/uploads.rs::post_upload`:
  - Keep the existing temp-file streaming + `infer` sniff unchanged.
  - After the allowlist check, branch on `mime_type.starts_with("image/")`:
    - **Image branch**:
      1. Acquire a permit: `let _permit = crate::uploads::thumbnail_semaphore().acquire().await.expect("semaphore not closed");`
      2. Call `pipeline::process_image(&tmp_path, &mime_type)`. On `PipelineError::Decode`, log a warning and return `400 BAD_REQUEST` with body `"image could not be decoded"`. On any other error, return `500`. (Reject-on-decode-failure rather than the discussion's "accept-with-fallback" lean: in practice `infer` is strict enough that a successful sniff followed by a decode failure is a real-but-broken image, and shipping a broken file inline is worse than the rare false rejection. Reconsider after first deploy if support traffic disagrees.)
      3. Compute sha256 over `processed.original_bytes` (NOT the temp file). The dedup key is the stripped form so two users uploading the same photo with different cameras-of-origin metadata dedup correctly.
      4. Final paths: `final_path = uploads_root.join("{hex}.{ext}")`, `preview_path = uploads_root.join("{hex}_preview.{ext}")`.
      5. If `final_path` already exists: dedup hit, write the preview only if missing on disk (heals pre-phase rows), do NOT rewrite the original. Delete the temp file.
      6. Else: write `processed.original_bytes` to `final_path` atomically (write to `{final_path}.partial`, then rename), then write `processed.preview_bytes` to `preview_path` similarly. If the preview write fails (disk full, etc.), log a warning and continue: the original is committed and the serve route will fall back. Delete the temp file.
      7. Insert the row with `size_bytes = processed.original_bytes.len() as i64`. The DB reflects what is actually on disk after strip; this matches what the user will download.
    - **Non-image branch (PDF)**: existing path unchanged.
  - Drop the permit (the `let _permit` binding goes out of scope at the end of the image branch).
- [ ] Replace the existing `sha256_file(&tmp_path)` call in the image branch with `sha256_bytes(&processed.original_bytes)` (a small helper added in the same file).
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/routes/uploads.rs`
- [ ] `git commit -m "feat(uploads): pipeline runs on image uploads (strip + thumbnail)"`

### Task 4 - Serve ?size=preview from GET /api/files/:id

- [ ] Edit `server/src/routes/uploads.rs::get_file`:
  - Add an extractor: `Query(params): Query<HashMap<String, String>>` (use `axum::extract::Query`).
  - After the access-control branch, compute the path to serve:
    - If `params.get("size") == Some("preview")` AND `row.mime_type.starts_with("image/")`: derive preview path by inserting `_preview` before the extension in `row.storage_path`. Try `tokio::fs::metadata(&preview_path).await`. If present, serve it. If absent, fall through to the original.
    - Otherwise: serve the original (existing behaviour).
  - `Content-Type` for the preview is the same as the original (the pipeline preserves format).
  - Leave `Content-Disposition` and `Cache-Control` unchanged.
- [ ] Edit `server/templates/partials/attachment.html`:
  - Image branch:
    ```html
    <a href="{{ a.url }}" target="_blank" rel="noopener" class="block max-w-sm">
      <img src="{{ a.url }}?size=preview" alt="{{ a.filename }}" class="max-w-sm max-h-96 rounded mt-1">
    </a>
    ```
  - PDF branch unchanged.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] Manual smoke (deferred to Task 8 verification): upload a large JPEG, confirm the inline render is the preview (check Network tab byte size).
- [ ] `git add server/src/routes/uploads.rs server/templates/partials/attachment.html`
- [ ] `git commit -m "feat(uploads): serve ?size=preview from /api/files/:id"`

### Task 5 - Orphan sweeper + hourly tick

- [ ] Implement `sweep::run_orphan_sweep(state: &AppState, threshold_hours: i64) -> Result<SweepStats, sqlx::Error>` in `server/src/uploads/sweep.rs`:
  - Module-head comment documenting the single-process invariant: the dedup-aware "is this file still referenced?" check and the row delete must run inside the same SQLite transaction. Two GC processes running concurrently could otherwise both see "no other references", both delete the file, and one would lose the race with another user's still-linked dedup-share. Today the scheduler is single-process, so this is a code-comment invariant rather than a defended-against scenario; if the GC ever moves to a sidecar, the transaction is what makes it correct.
  - `SELECT id, storage_path FROM file_uploads WHERE message_id IS NULL AND created_at < datetime('now', '-' || ? || ' hours')` with `threshold_hours` bound. Use the existing `idx_file_uploads_orphan` index.
  - For each candidate:
    1. `BEGIN TRANSACTION`.
    2. `SELECT COUNT(*) FROM file_uploads WHERE storage_path = ? AND id <> ?` to count siblings.
    3. `DELETE FROM file_uploads WHERE id = ?`.
    4. `COMMIT`.
    5. If sibling count was zero, delete the on-disk files: `uploads_dir().join(&storage_path)` AND the corresponding `_preview` path. Treat "file already gone" as success (the goal is "no orphan rows"; the filesystem is best-effort). Any other I/O error is logged as a warning; the row is already deleted so the next tick won't retry.
  - Errors mid-batch: log the row id and the error, continue to the next. Partial progress is fine.
  - Return `SweepStats { rows_deleted, files_deleted, errors }`. Log at INFO when non-zero.
- [ ] Add the DB helpers in `server/src/db/uploads.rs`:
  - `pub async fn select_orphans_older_than(pool: &SqlitePool, hours: i64) -> Result<Vec<(i64, String)>, sqlx::Error>`
  - `pub async fn count_uploads_sharing_path<E: SqliteExecutor>(exec: E, storage_path: &str, exclude_id: i64) -> Result<i64, sqlx::Error>`
  - `pub async fn delete_upload_row<E: SqliteExecutor>(exec: E, id: i64) -> Result<(), sqlx::Error>`
  - The `SqliteExecutor` generics let the sweep pass an open transaction in for the count+delete pair.
- [ ] In `server/src/main.rs`, add `spawn_orphan_sweeper(state.clone());` alongside the existing two spawn calls, and add the function definition modeled exactly on `spawn_digest_sender`:
  ```rust
  /// Hourly tick that calls `uploads::sweep::run_orphan_sweep` with the
  /// 24-hour threshold. Internal short-circuit if no orphans exist makes
  /// this safe to run on quiet deployments. Modeled on `spawn_digest_sender`.
  fn spawn_orphan_sweeper(state: AppState) {
      const TICK_SECS: u64 = 3600;
      const THRESHOLD_HOURS: i64 = 24;
      tokio::spawn(async move {
          let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
          tick.tick().await;
          loop {
              tick.tick().await;
              match lets_chat::uploads::sweep::run_orphan_sweep(&state, THRESHOLD_HOURS).await {
                  Ok(stats) if stats.rows_deleted > 0 => {
                      tracing::info!(
                          rows = stats.rows_deleted,
                          files = stats.files_deleted,
                          "orphan sweep complete"
                      );
                  }
                  Ok(_) => {}
                  Err(e) => tracing::warn!(error = %e, "orphan sweep failed"),
              }
          }
      });
  }
  ```
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/uploads/sweep.rs server/src/db/uploads.rs server/src/main.rs`
- [ ] `git commit -m "feat(uploads): hourly orphan sweeper with dedup-aware file delete"`

### Task 6 - Admin surfaces

- [ ] In `server/src/db/uploads.rs`, add:
  - `pub async fn sum_size_bytes(pool: &SqlitePool) -> Result<i64, sqlx::Error>` -> `SELECT COALESCE(SUM(size_bytes), 0) FROM file_uploads`.
  - `pub async fn count_orphans(pool: &SqlitePool) -> Result<i64, sqlx::Error>` -> `SELECT COUNT(*) FROM file_uploads WHERE message_id IS NULL`.
  - `pub async fn list_image_uploads(pool: &SqlitePool) -> Result<Vec<UploadRow>, sqlx::Error>` -> `SELECT ... WHERE mime_type LIKE 'image/%' ORDER BY id`. Used by the regen action.
- [ ] In `server/src/views/admin.rs::AdminSettingsView`, add `uploads_total_bytes: i64` and `uploads_orphan_count: i64`. Populate from the new DB helpers in `routes::admin::get_settings`.
- [ ] In `server/src/routes/admin.rs`:
  - `post_regenerate_thumbnails(state, AdminUser(_)) -> Result<Redirect, AppError>`: iterate `list_image_uploads`, for each row read the original via `tokio::fs::read(uploads_dir().join(&row.storage_path))`, run a preview-only variant of `pipeline::process_image` (factor out `pipeline::preview_only(bytes, mime) -> Vec<u8>` during this task), write `{sha256}_preview.{ext}` only if missing on disk. Cap concurrency via the same semaphore. Return `Redirect::to("/admin/settings")`. Use the existing flash mechanism if present, otherwise add a `?regenerated=N` query string to the redirect for the template to display.
  - `post_purge_orphans(state, AdminUser(_))`: call `sweep::run_orphan_sweep(&state, 0)`. Redirect with the row count in the query string.
  - Add the routes to the `routes()` builder alongside other admin routes.
- [ ] Factor `pipeline::preview_only(decoded: &image::DynamicImage, mime: &str) -> Result<Vec<u8>, PipelineError>` out of `process_image` so the regen action reuses the same encoder logic without re-running the original-strip pass. Update `process_image` to call it internally.
- [ ] Edit `server/templates/admin/settings.html` to add the Uploads panel: two read-only stats (total bytes formatted as MiB, orphan row count) and two POST forms with submit buttons. Mirror the existing settings-panel styling; do not introduce new Tailwind classes.
- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/src/uploads/pipeline.rs server/src/db/uploads.rs server/src/views/admin.rs server/src/routes/admin.rs server/templates/admin/settings.html`
- [ ] `git commit -m "feat(uploads): admin regenerate-thumbnails, purge-orphans-now, disk-usage panel"`

### Task 7 - Tests

- [ ] `server/tests/uploads_pipeline.rs`:
  - Helper: `synthetic_jpeg_with_gps()` returns a `Vec<u8>` that is a valid JPEG with a fake GPS EXIF block. (Either hand-craft via a hex literal or use `image` to encode a small image and then splice an APP1 marker; the latter is easier and small enough to inline.)
  - Test: run `process_image` on it, parse the output with `kamadak-exif::Reader::new().read_from_container(...)`, assert no GPS fields.
  - Same shape for PNG with `tEXt` chunk and WebP with EXIF block.
  - Test: 1200x800 JPEG input -> preview output has `max(width, height) == 360`.
  - Test: animated GIF input (3 frames) -> preview output is a single-frame GIF (`image` exposes frame count).
  - Test: malformed bytes that pass an `infer` PNG sniff but fail `image::ImageReader::decode` -> `PipelineError::Decode` returned.
- [ ] `server/tests/uploads_sweep.rs`:
  - **Centerpiece (dedup + GC)**: user A inserts an upload row, links to a message, sha is `"abc.png"`. User B inserts an orphan row with the same `storage_path = "abc.png"` (manual SQL, simulating dedup). Place a real file on disk at `{tempdir}/uploads/abc.png`. Force B's `created_at` to 25 hours ago via UPDATE. Call `run_orphan_sweep(state, 24)`. Assert: B's row is gone; A's row is unchanged; the file at `abc.png` still exists.
  - Orphan younger than threshold survives: insert orphan at `now`, run sweep at threshold 24h, row still present.
  - Linked row is never touched: insert linked row at `created_at = 'now', -100 days'`, run sweep, row still present.
  - Missing file is treated as success: insert orphan with `storage_path = "nonexistent.png"`, no file on disk, run sweep, row is deleted without error.
  - Preview file is also removed: place both `abc.png` and `abc_preview.png` on disk, sole-referencing orphan past threshold, run sweep, both files gone.
- [ ] Extend `server/tests/routes_uploads.rs`:
  - **Preview regeneration on dedup hit**: insert a row with `storage_path = "xyz.png"`, place `xyz.png` on disk but NOT `xyz_preview.png` (simulating pre-phase data). Upload the same PNG bytes as a different user via `POST /api/upload`. Assert the response is 200 and `xyz_preview.png` now exists.
  - Upload a JPEG with synthetic EXIF GPS, then `GET /api/files/:id` (no preview), parse the response body with `kamadak-exif`, assert no GPS fields. (End-to-end EXIF-strip assertion through the HTTP boundary.)
  - Upload a JPEG, `GET /api/files/:id?size=preview`, assert response Content-Length < the original's Content-Length and the decoded image is smaller-dimensioned.
  - Upload a PDF, `GET /api/files/:id?size=preview`, assert fallback: 200 with `Content-Type: application/pdf` and the original byte count.
  - Decode-failure rejection: POST a synthetic file whose first bytes match the PNG magic but the body is truncated noise, assert 400.
- [ ] `server/tests/admin_uploads.rs`:
  - Force-regenerate creates missing previews: seed a row with original-only on disk, run the admin POST as an admin user, assert preview file exists after.
  - Purge-now removes a stale orphan: seed a 25h-old orphan, run the admin POST, assert row gone.
  - Anonymous calls to both endpoints return 401 (or whatever the existing AdminUser extractor returns; mirror an existing `admin_*` test).
  - Non-admin authed call returns 403.
- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/tests/uploads_pipeline.rs server/tests/uploads_sweep.rs server/tests/routes_uploads.rs server/tests/admin_uploads.rs`
- [ ] `git commit -m "test(uploads): pipeline, sweep, dedup-GC interaction, admin actions"`

### Task 8 - Final verification + PR

- [ ] `just check` (clippy + fmt + check for server and desktop)
- [ ] If clippy warns, fix and re-run; do not silence with `#[allow]` unless the warning is genuinely wrong.
- [ ] `just test`
- [ ] `just verify` (release binary still serves /login)
- [ ] Manual smoke via `just dev-web-local` at http://localhost:18080:
  - Upload an iPhone photo (or any JPEG with EXIF). Confirm the message shows the inline preview. Open the original in a new tab; download it; run `exiftool` on the download; assert no GPS / camera fields.
  - Upload a 5 MB PNG; confirm the inline render fetches `~< 100 KB` (preview), and the click-through fetches the full file.
  - Upload an animated GIF; confirm the inline render is a static first-frame; click opens the animated original.
  - Upload a small PNG, immediately reload the page; the inline preview should render.
  - Log in as admin; visit `/admin/settings`; observe the Uploads panel with byte total and orphan count.
  - Click "Purge orphans now"; observe the count drop.
  - Click "Regenerate thumbnails"; delete `_preview` files via the host shell first, then click the button, confirm files reappear.
- [ ] `git push -u origin feat/upload-hygiene`
- [ ] Open PR titled `feat(uploads): thumbnails + EXIF stripping + orphan GC (Phase 23)`. Body lists:
  - new endpoints (`?size=preview` on the existing serve route, two admin POSTs)
  - new module (`server/src/uploads/`)
  - new dependency (`image`)
  - operator notes: hourly GC tick, 24-hour orphan threshold, `THUMBNAIL_CONCURRENCY = 4`, single-process invariant for GC
  - no DB migration (existing orphan index already in place)
- [ ] After merge: `git checkout main && git pull` per the project's standing git workflow.

## Out of scope

- Audio / video thumbnailing or waveforms.
- External image services or CDN integration. Local processing only.
- GC of uploads from hard-deleted messages. This phase scopes orphan to `message_id IS NULL`; the soft-delete vs hard-delete cleanup question is a separate future phase if it ever materialises.
- An admin toggle for the EXIF strip pass. Default-on, no UI. Add it if real demand surfaces.
- A unified "media pipeline" abstraction. Image-only is one feature with three sub-tasks; abstracting prematurely is the wrong move.
- Per-MIME / per-room disk-usage breakdowns in the admin panel. Two numbers (total + orphan count) is the line.
- Multi-process GC. Today's scheduler is single-process; the in-transaction sibling-count is the invariant that would let a future sidecar work, not something to engineer for now.

## Things to confirm / deviations

- **No new chat or settings migrations.** The orphan-GC index from phase 13 (`server/migrations/chat/0012_uploads.sql:14`) is exactly what the sweeper needs. Confirm before Task 5 that the index still exists; if a later phase has dropped or replaced it, add it back in `0022_uploads_orphan_index.sql` and update the file map.
- **Decode-failure policy.** The design discussion leaned "accept-with-fallback" but Task 3 ships "reject with 400". The reasoning: `infer` is strict enough that a sniff-then-decode-fail input is a real-but-broken file, and the inline-render fallback path produces a worse user experience than a clear error. The cost of being wrong is a small number of legitimate-but-broken images being rejected; the cost of being wrong the other way is shipping corrupted files. Easy to flip later; revisit after first deploy if support traffic complains.
- **Dedup key changes meaning.** Pre-phase, the sha256 was computed over user-supplied bytes. Post-phase, it is computed over the stripped re-encoded bytes. This is intentional (two users uploading the same photo with different camera-of-origin metadata should dedup) but it means: pre-phase rows on a deployment that upgrades will keep their old sha256-of-raw-bytes paths and will not collide with new uploads. The regen-thumbnails admin action handles preview backfill for these rows; the originals stay where they are and serve correctly. Document this in the PR body.
- **`size_bytes` reflects on-disk post-strip size.** Phase 13 wrote the pre-sniff streamed byte count. Phase 23 writes the post-strip length so the DB matches what the user will download. Tests in Task 7 assert this. Non-image uploads (PDF) are unaffected.
- **GIF animated thumbnails.** Static first-frame only. If a user complains in week one, the fix is a separate concern (animated thumbnails via `image`'s GIF encoder are slow and bloaty); not in this phase.
- **Admin panel naming.** "Regenerate thumbnails" and "Purge orphans now" are user-facing button labels. Confirm with the existing admin-page tone (the existing settings page leans terse and technical, so these fit).
