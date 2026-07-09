# Let's Chat Redesign P1 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a six-palette theme system (each in light/dark/hc-light/hc-dark) with a live picker to Let's Chat, on the clean orthogonal `data-theme`(palette) + `data-mode`(mode) model, without redesigning any product surface.

**Architecture:** Two orthogonal `<html>` attributes. `data-theme` selects one of six palettes; `data-mode` selects light/dark/hc-light/hc-dark (the existing values, moved off `data-theme`). CSS tokens split into a palette-CONSTANT layer (status + actor-badge colors, keyed by mode only) and a palette-VARYING layer (surface/content/border/accent/ring/rail/sidebar, keyed by palette x mode). The no-flash bootstrap resolves the two axes independently; `users.theme` is renamed to `users.theme_mode` and a `users.theme_palette` column is added.

**Tech Stack:** Rust + axum + Askama templates, htmx, vanilla JS, Tailwind CSS v3 (`@layer components`), SQLite via sqlx. Build/test through `just` (dev container). No SPA framework.

**Spec:** `docs/superpowers/specs/2026-07-09-lets-chat-redesign-p1-foundation-design.md`. Tracking: LC-541.

**Conventions for every commit in this plan:** branch is `docs/LC-541-redesign-p1-foundation` (already exists; the spec commit is its first commit). Commit subjects imperative <=50 chars. End each commit body with a bare `#LC-541` line, then a blank line, then `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. No em-dashes anywhere.

**Verification commands (run in the dev container via just):**
- `just build-css` - compile Tailwind (`server/assets/tailwind.css` -> gitignored `tailwind-built.css`).
- `just check` - fmt + clippy + compile (server + saas + desktop).
- `just test` - server test suite.
- `just dev-web-local` - run the app locally to eyeball surfaces.

---

## Token architecture (read before Task 5-7)

The current `main.css` has four flat blocks: `:root` (light, 194-289), `[data-theme="dark"]` (296), `[data-theme="hc-light"]` (388), `[data-theme="hc-dark"]` (451). We refactor into two layers:

- **Constant layer (per mode, palette-independent):** the status trio (`--success*`, `--warning*`, `--danger*`, their `-surface/-border/-surface-content`) and the actor badges (`--webhook/-email/-bridge-*`). These stay the same across all six palettes (the user's brief: "keep status colors across all of them"), but still shift light->dark->HC. Defined on `:root` (light), `[data-mode="dark"]`, `[data-mode="hc-light"]`, `[data-mode="hc-dark"]`.
- **Varying layer (per palette x mode):** `--surface/-elevated/-sunken`, `--content/-muted/-subtle`, `--border/-strong`, `--accent/-hover/-content/-surface/-surface-content`, `--ring`, `--rail-*`, `--sidebar-*`. Defined on `[data-theme="<palette>"]` (that palette's light) and `[data-theme="<palette>"][data-mode="dark|hc-light|hc-dark"]`.

Resolution: the bootstrap ALWAYS stamps both attributes (default `data-theme="blue-harbor"`, `data-mode` resolved as today). `:root` additionally carries blue-harbor-light varying values as the pre-JS fallback, so a no-JS document is today's look. The six palettes are `blue-harbor` (default, = current values), `cobalt`, `ink-ice`, `arctic`, `deep-sea`, `royal-navy`.

---

## Task 1: DB migration - rename theme -> theme_mode, add theme_palette

**Files:**
- Create: `server/migrations/auth/0037_theme_palette.sql`
- Test: `server/tests/` (new integration check, see step 1)

- [ ] **Step 1: Write the failing test**

Create `server/tests/theme_palette_migration.rs`:

```rust
// Verifies the 0037 migration renamed theme -> theme_mode and added theme_palette,
// preserving existing rows. Uses the same migrator the app uses.
use sqlx::sqlite::SqlitePoolOptions;

#[tokio::test]
async fn migration_renames_theme_and_adds_palette() {
    let pool = SqlitePoolOptions::new()
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!("./migrations/auth").run(&pool).await.unwrap();

    // Columns exist with expected names.
    let cols: Vec<String> = sqlx::query_scalar("SELECT name FROM pragma_table_info('users')")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(cols.contains(&"theme_mode".to_string()), "theme_mode column missing");
    assert!(cols.contains(&"theme_palette".to_string()), "theme_palette column missing");
    assert!(!cols.contains(&"theme".to_string()), "old theme column still present");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `just test` (or, inside the server dir in the dev container, `cargo test --test theme_palette_migration`).
Expected: FAIL - migration `0037` does not exist yet, so `theme_mode` is absent.

- [ ] **Step 3: Write the migration**

Create `server/migrations/auth/0037_theme_palette.sql`:

```sql
-- LC-541 P1: split the single UI theme pref into mode + palette.
-- `theme` held the light/dark/hc-light/hc-dark mode; rename it to `theme_mode`.
-- Add `theme_palette` (NULL = blue-harbor, the current look). SQLite 3.25+
-- supports RENAME COLUMN; data is preserved untouched.
ALTER TABLE users RENAME COLUMN theme TO theme_mode;
ALTER TABLE users ADD COLUMN theme_palette TEXT;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `just test` (or `cargo test --test theme_palette_migration`).
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add server/migrations/auth/0037_theme_palette.sql server/tests/theme_palette_migration.rs
git commit -F- <<'EOF'
feat(theme): split theme pref into mode + palette (migration)

Rename users.theme -> users.theme_mode and add users.theme_palette (NULL =
blue-harbor). Groundwork for the six-palette theme system; existing rows keep
their mode and default the palette in.

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 2: User model - mode + palette accessors

**Files:**
- Modify: `server/src/models/user.rs:103` (field) and `:121-126` (accessor)

- [ ] **Step 1: Write the failing test**

Append to `server/src/models/user.rs` (a `#[cfg(test)]` module if none exists):

```rust
#[cfg(test)]
mod theme_tests {
    use super::User;

    fn user_with(mode: Option<&str>, palette: Option<&str>) -> User {
        let mut u = User::default(); // if User has no Default, construct minimally in-place
        u.theme_mode = mode.map(str::to_string);
        u.theme_palette = palette.map(str::to_string);
        u
    }

    #[test]
    fn mode_defaults_to_system() {
        assert_eq!(user_with(None, None).theme_mode_or_system(), "system");
        assert_eq!(user_with(Some("dark"), None).theme_mode_or_system(), "dark");
        assert_eq!(user_with(Some("bogus"), None).theme_mode_or_system(), "system");
    }

    #[test]
    fn palette_defaults_to_blue_harbor() {
        assert_eq!(user_with(None, None).theme_palette_or_default(), "blue-harbor");
        assert_eq!(user_with(None, Some("cobalt")).theme_palette_or_default(), "cobalt");
        assert_eq!(user_with(None, Some("bogus")).theme_palette_or_default(), "blue-harbor");
    }
}
```

Note: if `User` does not derive `Default`, replace `user_with` with a literal struct build copying the pattern used elsewhere in the test suite; the assertions are what matter.

- [ ] **Step 2: Run test to verify it fails**

Run: `just test` (or `cargo test -p server models::user`).
Expected: FAIL - `theme_mode` field and both accessors do not exist.

- [ ] **Step 3: Implement**

In `server/src/models/user.rs`, rename the field (line 103) and add the palette field:

```rust
    /// LC-541: preferred UI mode ("light"/"dark"/"hc-light"/"hc-dark"/"system"),
    /// or None = system. (Renamed from `theme` when the palette axis was added.)
    pub theme_mode: Option<String>,
    /// LC-541: preferred palette ("blue-harbor"/"cobalt"/"ink-ice"/"arctic"/
    /// "deep-sea"/"royal-navy"), or None = blue-harbor.
    pub theme_palette: Option<String>,
```

Replace `theme_or_system` (121-126) with:

```rust
    /// LC-541: saved mode preference, defaulting to "system" when unset.
    pub fn theme_mode_or_system(&self) -> &str {
        match self.theme_mode.as_deref() {
            Some(t @ ("light" | "dark" | "hc-light" | "hc-dark" | "system")) => t,
            _ => "system",
        }
    }

    /// LC-541: saved palette preference, defaulting to "blue-harbor" when unset.
    pub fn theme_palette_or_default(&self) -> &str {
        match self.theme_palette.as_deref() {
            Some(p @ ("blue-harbor" | "cobalt" | "ink-ice" | "arctic" | "deep-sea" | "royal-navy")) => p,
            _ => "blue-harbor",
        }
    }
```

Then update every construction site of `User` and every SELECT that hydrated `theme`. Find them:

Run: `cd server && grep -rn "\btheme\b" src/ | grep -vi theme_mode | grep -v theme_palette`
Fix each: the row projection that did `.theme = row.get("theme")` becomes `theme_mode = row.get("theme_mode")` plus `theme_palette = row.get("theme_palette")`; struct literals gain `theme_palette`. The primary hydrator is in `server/src/db/auth.rs` (the `User`/`UserRecord` SELECT + row mapping) - update the column list and the field assignment there.

- [ ] **Step 4: Run test + compile**

Run: `just check` then `just test`.
Expected: compiles; theme_tests PASS.

- [ ] **Step 5: Commit**

```bash
git add server/src/models/user.rs server/src/db/auth.rs
git commit -F- <<'EOF'
feat(theme): mode + palette accessors on User

Rename User.theme -> theme_mode, add theme_palette, with
theme_mode_or_system()/theme_palette_or_default() and updated row hydration.

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 3: DB setters - mode + palette

**Files:**
- Modify: `server/src/db/auth.rs:777-789` (rename `set_user_theme`) and add a palette setter

- [ ] **Step 1: Write the failing test**

Add to `server/tests/theme_palette_migration.rs`:

```rust
#[tokio::test]
async fn setters_write_mode_and_palette() {
    // Build a pool + one user row via the app's helpers, then:
    // (pseudocode - use the crate's real test harness for user creation)
    // db::auth::set_user_theme_mode(&pool, &uid, Some("dark")).await.unwrap();
    // db::auth::set_user_theme_palette(&pool, &uid, Some("cobalt")).await.unwrap();
    // let u = db::auth::get_user(&pool, &uid).await.unwrap();
    // assert_eq!(u.theme_mode.as_deref(), Some("dark"));
    // assert_eq!(u.theme_palette.as_deref(), Some("cobalt"));
}
```

If the crate exposes no in-test user factory, assert at the query layer instead: insert a row with raw SQL, call the setters, `SELECT theme_mode, theme_palette`.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test theme_palette_migration setters_write` in the dev container.
Expected: FAIL - `set_user_theme_mode` / `set_user_theme_palette` undefined.

- [ ] **Step 3: Implement**

In `server/src/db/auth.rs`, rename `set_user_theme` (777) and add the palette setter (mirror `set_user_density` at 763):

```rust
/// LC-541: set (or clear, with `None`) a user's preferred UI mode.
pub async fn set_user_theme_mode(
    pool: &SqlitePool,
    user_id: &str,
    mode: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET theme_mode = ? WHERE id = ?")
        .bind(mode)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// LC-541: set (or clear, with `None`) a user's preferred palette.
pub async fn set_user_theme_palette(
    pool: &SqlitePool,
    user_id: &str,
    palette: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE users SET theme_palette = ? WHERE id = ?")
        .bind(palette)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}
```

- [ ] **Step 4: Run test**

Run: `just check` then the setter test.
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add server/src/db/auth.rs server/tests/theme_palette_migration.rs
git commit -F- <<'EOF'
feat(theme): db setters for mode + palette

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 4: Routes - persist mode + palette; add /settings/palette

**Files:**
- Modify: `server/src/routes/settings.rs:564-634` (`AppearanceForm`, `post_appearance`, `ThemeForm`, `post_theme`)
- Modify: `server/src/routes/mod.rs:1554` (register `/settings/palette`)

- [ ] **Step 1: Write the failing test**

Add a handler-level test (follow the pattern of existing route tests in the suite; if handlers are tested via a `TestServer`, assert the cookie + persisted column). Minimum assertion: posting `palette=cobalt` to `/settings/palette` persists `theme_palette='cobalt'` and returns a `lc-palette=cobalt` cookie; an invalid palette falls back to `blue-harbor`.

- [ ] **Step 2: Run to verify it fails**

Expected: FAIL - `/settings/palette` route and `post_palette` do not exist.

- [ ] **Step 3: Implement**

In `server/src/routes/settings.rs`:

Rename the `ThemeForm.theme` handling in `post_theme` (618) to use the mode setter + `lc-mode` cookie:

```rust
    db::auth::set_user_theme_mode(&state.auth, &user.id, Some(theme)).await?;
    let cookie = format!("lc-mode={theme}; Path=/; Max-Age=31536000; SameSite=Lax");
```

Update `AppearanceForm` (565) to carry a palette, and `post_appearance` (577) to persist all three:

```rust
#[derive(serde::Deserialize)]
pub struct AppearanceForm {
    #[serde(default)]
    pub theme: String,   // mode: system/light/dark/hc-light/hc-dark
    #[serde(default)]
    pub palette: String, // blue-harbor/cobalt/ink-ice/arctic/deep-sea/royal-navy
    #[serde(default)]
    pub density: String,
}
```

In `post_appearance`, after the existing `theme`/`density` validation, add palette validation and persist:

```rust
    let palette = match form.palette.trim() {
        p @ ("blue-harbor" | "cobalt" | "ink-ice" | "arctic" | "deep-sea" | "royal-navy") => Some(p),
        "" => None,          // empty = leave/clear -> blue-harbor default
        _ => None,
    };
    db::auth::set_user_theme_mode(&state.auth, &user.id, Some(theme)).await?;
    db::auth::set_user_theme_palette(&state.auth, &user.id, palette).await?;
    db::auth::set_user_density(&state.auth, &user.id, Some(density)).await?;
```

Add the quick-persist palette endpoint (mirror `post_theme`):

```rust
#[derive(serde::Deserialize)]
pub struct PaletteForm {
    #[serde(default)]
    pub palette: String,
}

/// LC-541: POST /settings/palette - the appearance picker's palette persist.
/// Returns 204 + an `lc-palette` cookie so the next navigation has no flash.
pub async fn post_palette(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    axum::Form(form): axum::Form<PaletteForm>,
) -> Result<Response, AppError> {
    let palette = match form.palette.trim() {
        p @ ("blue-harbor" | "cobalt" | "ink-ice" | "arctic" | "deep-sea" | "royal-navy") => p,
        _ => "blue-harbor",
    };
    db::auth::set_user_theme_palette(&state.auth, &user.id, Some(palette)).await?;
    let cookie = format!("lc-palette={palette}; Path=/; Max-Age=31536000; SameSite=Lax");
    Ok((
        axum::http::StatusCode::NO_CONTENT,
        [(axum::http::header::SET_COOKIE, cookie)],
    )
        .into_response())
}
```

In `server/src/routes/mod.rs` after line 1554:

```rust
        .route("/settings/palette", post(settings::post_palette))
```

- [ ] **Step 4: Run**

Run: `just check` then `just test`.
Expected: compiles; route test PASS.

- [ ] **Step 5: Commit**

```bash
git add server/src/routes/settings.rs server/src/routes/mod.rs
git commit -F- <<'EOF'
feat(theme): persist palette; rename mode cookie to lc-mode

post_appearance persists mode+palette+density; add POST /settings/palette;
sidebar quick-toggle writes lc-mode.

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 5: Cookie sync middleware - lc-mode + lc-palette

**Files:**
- Modify: `server/src/auth.rs:204-252` (`resolve_locale`)

- [ ] **Step 1: Write the failing test**

If middleware is unit-tested elsewhere, add a case; otherwise this is covered by the Task 12 manual cross-device AC. Add a focused note test asserting the valid-palette list is used (compile-time: the sync call references the six names).

- [ ] **Step 2: Implement**

In `resolve_locale`, replace the theme sync. Change the pref reads (212):

```rust
    let user_mode = user.and_then(|u| u.theme_mode.clone());
    let user_palette = user.and_then(|u| u.theme_palette.clone());
    let user_density = user.and_then(|u| u.density.clone());
    let existing_mode = read_cookie(req.headers(), "lc-mode");
    let existing_palette = read_cookie(req.headers(), "lc-palette");
    let existing_density = read_cookie(req.headers(), "lc-density");
```

Replace the two `sync(...)` calls (240-251) with three:

```rust
    sync(
        "lc-mode",
        &user_mode,
        &existing_mode,
        &["light", "dark", "hc-light", "hc-dark", "system"],
    );
    sync(
        "lc-palette",
        &user_palette,
        &existing_palette,
        &["blue-harbor", "cobalt", "ink-ice", "arctic", "deep-sea", "royal-navy"],
    );
    sync(
        "lc-density",
        &user_density,
        &existing_density,
        &["comfortable", "compact"],
    );
```

- [ ] **Step 3: Run**

Run: `just check`.
Expected: compiles (no more `u.theme`, `lc-theme` references in this file).

- [ ] **Step 4: Commit**

```bash
git add server/src/auth.rs
git commit -F- <<'EOF'
feat(theme): sync lc-mode + lc-palette cookies cross-device

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 6: Tailwind darkMode selector

**Files:**
- Modify: `server/tailwind.config.js:7`

- [ ] **Step 1: Implement**

Change line 7 so `dark:` variants fire for both dark modes off the new attribute:

```js
  darkMode: ["selector", '[data-mode~="dark"]'],
```

- [ ] **Step 2: Verify**

Run: `just build-css`.
Then: `grep -c 'data-mode~="dark"' server/assets/tailwind-built.css` -> expect a non-zero count (the `dark:` utilities now compile against `[data-mode~="dark"]`).
Expected: build succeeds.

- [ ] **Step 3: Commit**

```bash
git add server/tailwind.config.js
git commit -F- <<'EOF'
feat(theme): key Tailwind dark: on data-mode

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 7: Token CSS - constant layer + blue-harbor re-scope (visual no-op)

**Files:**
- Modify: `server/assets/main.css:187-516` (the four theme blocks)

This task ONLY restructures the existing four blocks into the two-layer model with blue-harbor as the default. It introduces zero new colors, so the app looks identical after it. New palettes come in Task 8.

- [ ] **Step 1: Split the constant tokens out of `:root`**

Keep the status + actor-badge vars (`--success*`, `--warning*`, `--danger*`, `--webhook*`, `--email*`, `--bridge*`) on `:root` (they are the light-mode constants). Move the current `[data-theme="dark"]` status/badge values to a new `[data-mode="dark"]` block; the current `[data-theme="hc-light"]`/`[data-theme="hc-dark"]` status/badge values to `[data-mode="hc-light"]`/`[data-mode="hc-dark"]`.

- [ ] **Step 2: Re-scope the varying tokens to blue-harbor**

The current `:root` varying values (surface/content/border/accent/ring/rail/sidebar) stay on `:root` (pre-JS fallback = blue-harbor light) AND are duplicated onto `[data-theme="blue-harbor"]`. The current `[data-theme="dark"]` varying values move to `[data-theme="blue-harbor"][data-mode="dark"]`; hc-light -> `[data-theme="blue-harbor"][data-mode="hc-light"]`; hc-dark -> `[data-theme="blue-harbor"][data-mode="hc-dark"]`.

- [ ] **Step 3: Build + eyeball (no-op check)**

Run: `just build-css` then `just dev-web-local`.
Manually: the app must look EXACTLY as before in default (blue-harbor) across light and, via the sidebar toggle, dark; high-contrast via OS. Because bootstrap is not ported yet (Task 9), `data-mode` is not stamped, so also add a temporary `<html data-theme="blue-harbor" data-mode="light">` sanity check by editing the browser devtools to confirm the selectors resolve.
Expected: identical rendering; no missing tokens (check devtools Computed for `--surface` etc.).

- [ ] **Step 4: Commit**

```bash
git add server/assets/main.css
git commit -F- <<'EOF'
refactor(theme): split tokens into constant + palette layers

Constant status/badge tokens keyed by data-mode; blue-harbor varying tokens
scoped under [data-theme="blue-harbor"]. Pure restructure, no color change.

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 8: Token CSS - five new palettes (light + dark)

**Files:**
- Modify: `server/assets/main.css` (append palette blocks after the blue-harbor blocks)

- [ ] **Step 1: Add each palette's light + dark varying block**

For each palette add `[data-theme="<p>"]` (light) and `[data-theme="<p>"][data-mode="dark"]` (dark). Values below are from the approved palette stubs. Rail tokens derive per palette by rule: `--rail-surface = sidebar-sunken`, `--rail-tile = sidebar-elevated`, `--rail-tile-hover = accent-hover`, `--rail-content = #ffffff`, `--rail-content-muted = sidebar-muted`. Status/badge tokens are NOT redefined (they come from the constant layer).

Cobalt (light):

```css
[data-theme="cobalt"] {
  --surface:#f5f9ff; --surface-elevated:#ffffff; --surface-sunken:#eaf3ff;
  --content:#08111f; --content-muted:#53657c; --content-subtle:#8494aa;
  --border:#d4e3f7; --border-strong:#aec8eb;
  --accent:#0f62fe; --accent-hover:#0043ce; --accent-content:#ffffff;
  --accent-surface:#d0e2ff; --accent-surface-content:#0043ce; --ring:#4589ff;
  --sidebar-surface:#061932; --sidebar-elevated:#0a2750; --sidebar-sunken:#031022;
  --sidebar-content:#eff6ff; --sidebar-muted:#9db4d1; --sidebar-border:#143d70;
  --rail-surface:#031022; --rail-tile:#0a2750; --rail-tile-hover:#0043ce;
  --rail-content:#ffffff; --rail-content-muted:#9db4d1;
}
[data-theme="cobalt"][data-mode="dark"] {
  --surface:#061932; --surface-elevated:#0a2750; --surface-sunken:#031022;
  --content:#eff6ff; --content-muted:#9db4d1; --content-subtle:#6f86a3;
  --border:#143d70; --border-strong:#1e5a9e;
  --accent:#4589ff; --accent-hover:#78a9ff; --accent-content:#031022;
  --accent-surface:#003a6d; --accent-surface-content:#d0e2ff; --ring:#78a9ff;
  --sidebar-surface:#061932; --sidebar-elevated:#0a2750; --sidebar-sunken:#031022;
  --sidebar-content:#eff6ff; --sidebar-muted:#9db4d1; --sidebar-border:#143d70;
  --rail-surface:#031022; --rail-tile:#0a2750; --rail-tile-hover:#78a9ff;
  --rail-content:#ffffff; --rail-content-muted:#9db4d1;
}
```

Ink + Ice (light):

```css
[data-theme="ink-ice"] {
  --surface:#f8fbfd; --surface-elevated:#ffffff; --surface-sunken:#eef5fa;
  --content:#0b1623; --content-muted:#586879; --content-subtle:#8997a7;
  --border:#dce7ef; --border-strong:#bfccd8;
  --accent:#1e40af; --accent-hover:#1e3a8a; --accent-content:#ffffff;
  --accent-surface:#dbeafe; --accent-surface-content:#1e3a8a; --ring:#2563eb;
  --sidebar-surface:#07111f; --sidebar-elevated:#0d1b2d; --sidebar-sunken:#030914;
  --sidebar-content:#edf6ff; --sidebar-muted:#9aaec4; --sidebar-border:#1b334f;
  --rail-surface:#030914; --rail-tile:#0d1b2d; --rail-tile-hover:#1e3a8a;
  --rail-content:#ffffff; --rail-content-muted:#9aaec4;
}
[data-theme="ink-ice"][data-mode="dark"] {
  --surface:#07111f; --surface-elevated:#0d1b2d; --surface-sunken:#030914;
  --content:#edf6ff; --content-muted:#9aaec4; --content-subtle:#687f98;
  --border:#1b334f; --border-strong:#2b4d72;
  --accent:#60a5fa; --accent-hover:#93c5fd; --accent-content:#06101f;
  --accent-surface:#172554; --accent-surface-content:#dbeafe; --ring:#60a5fa;
  --sidebar-surface:#07111f; --sidebar-elevated:#0d1b2d; --sidebar-sunken:#030914;
  --sidebar-content:#edf6ff; --sidebar-muted:#9aaec4; --sidebar-border:#1b334f;
  --rail-surface:#030914; --rail-tile:#0d1b2d; --rail-tile-hover:#93c5fd;
  --rail-content:#ffffff; --rail-content-muted:#9aaec4;
}
```

Arctic Messenger (light):

```css
[data-theme="arctic"] {
  --surface:#fbfdff; --surface-elevated:#ffffff; --surface-sunken:#f0f7ff;
  --content:#0f172a; --content-muted:#5f7288; --content-subtle:#92a3b5;
  --border:#e1edf8; --border-strong:#c4d7eb;
  --accent:#0284c7; --accent-hover:#0369a1; --accent-content:#ffffff;
  --accent-surface:#e0f2fe; --accent-surface-content:#0369a1; --ring:#0ea5e9;
  --sidebar-surface:#062033; --sidebar-elevated:#0a2c47; --sidebar-sunken:#031522;
  --sidebar-content:#ecfeff; --sidebar-muted:#9bc4d6; --sidebar-border:#174764;
  --rail-surface:#031522; --rail-tile:#0a2c47; --rail-tile-hover:#0369a1;
  --rail-content:#ffffff; --rail-content-muted:#9bc4d6;
}
[data-theme="arctic"][data-mode="dark"] {
  --surface:#062033; --surface-elevated:#0a2c47; --surface-sunken:#031522;
  --content:#ecfeff; --content-muted:#9bc4d6; --content-subtle:#6f9aad;
  --border:#174764; --border-strong:#246487;
  --accent:#38bdf8; --accent-hover:#7dd3fc; --accent-content:#031522;
  --accent-surface:#075985; --accent-surface-content:#e0f2fe; --ring:#38bdf8;
  --sidebar-surface:#062033; --sidebar-elevated:#0a2c47; --sidebar-sunken:#031522;
  --sidebar-content:#ecfeff; --sidebar-muted:#9bc4d6; --sidebar-border:#174764;
  --rail-surface:#031522; --rail-tile:#0a2c47; --rail-tile-hover:#7dd3fc;
  --rail-content:#ffffff; --rail-content-muted:#9bc4d6;
}
```

Deep Sea Cyan (light):

```css
[data-theme="deep-sea"] {
  --surface:#f5fcff; --surface-elevated:#ffffff; --surface-sunken:#e8f7fb;
  --content:#071b24; --content-muted:#56707a; --content-subtle:#8aa0a9;
  --border:#d3eaf0; --border-strong:#afd1dc;
  --accent:#0891b2; --accent-hover:#0e7490; --accent-content:#ffffff;
  --accent-surface:#cffafe; --accent-surface-content:#0e7490; --ring:#06b6d4;
  --sidebar-surface:#041923; --sidebar-elevated:#082836; --sidebar-sunken:#020f15;
  --sidebar-content:#ecfeff; --sidebar-muted:#9bc6d0; --sidebar-border:#164150;
  --rail-surface:#020f15; --rail-tile:#082836; --rail-tile-hover:#0e7490;
  --rail-content:#ffffff; --rail-content-muted:#9bc6d0;
}
[data-theme="deep-sea"][data-mode="dark"] {
  --surface:#041923; --surface-elevated:#082836; --surface-sunken:#020f15;
  --content:#ecfeff; --content-muted:#9bc6d0; --content-subtle:#6f98a3;
  --border:#164150; --border-strong:#236173;
  --accent:#22d3ee; --accent-hover:#67e8f9; --accent-content:#021014;
  --accent-surface:#164e63; --accent-surface-content:#cffafe; --ring:#22d3ee;
  --sidebar-surface:#041923; --sidebar-elevated:#082836; --sidebar-sunken:#020f15;
  --sidebar-content:#ecfeff; --sidebar-muted:#9bc6d0; --sidebar-border:#164150;
  --rail-surface:#020f15; --rail-tile:#082836; --rail-tile-hover:#67e8f9;
  --rail-content:#ffffff; --rail-content-muted:#9bc6d0;
}
```

Royal Navy (light):

```css
[data-theme="royal-navy"] {
  --surface:#f6f8ff; --surface-elevated:#ffffff; --surface-sunken:#edf2ff;
  --content:#0b1026; --content-muted:#5c6680; --content-subtle:#8b94ad;
  --border:#dae2f5; --border-strong:#bbc8e5;
  --accent:#1d4ed8; --accent-hover:#1e3a8a; --accent-content:#ffffff;
  --accent-surface:#dbeafe; --accent-surface-content:#1e3a8a; --ring:#2563eb;
  --sidebar-surface:#080f2a; --sidebar-elevated:#101b45; --sidebar-sunken:#050a1a;
  --sidebar-content:#eff3ff; --sidebar-muted:#a3aed0; --sidebar-border:#23306a;
  --rail-surface:#050a1a; --rail-tile:#101b45; --rail-tile-hover:#1e3a8a;
  --rail-content:#ffffff; --rail-content-muted:#a3aed0;
}
[data-theme="royal-navy"][data-mode="dark"] {
  --surface:#080f2a; --surface-elevated:#101b45; --surface-sunken:#050a1a;
  --content:#eff3ff; --content-muted:#a3aed0; --content-subtle:#737fa5;
  --border:#23306a; --border-strong:#35479a;
  --accent:#60a5fa; --accent-hover:#93c5fd; --accent-content:#050a1a;
  --accent-surface:#1e3a8a; --accent-surface-content:#dbeafe; --ring:#60a5fa;
  --sidebar-surface:#080f2a; --sidebar-elevated:#101b45; --sidebar-sunken:#050a1a;
  --sidebar-content:#eff3ff; --sidebar-muted:#a3aed0; --sidebar-border:#23306a;
  --rail-surface:#050a1a; --rail-tile:#101b45; --rail-tile-hover:#93c5fd;
  --rail-content:#ffffff; --rail-content-muted:#a3aed0;
}
```

- [ ] **Step 2: Build + eyeball via the gallery is deferred to Task 11.** For now verify build only.

Run: `just build-css`.
Expected: builds clean.

- [ ] **Step 3: Commit**

```bash
git add server/assets/main.css
git commit -F- <<'EOF'
feat(theme): add cobalt/ink-ice/arctic/deep-sea/royal-navy (light+dark)

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 9: No-flash bootstrap - two axes

**Files:**
- Modify: `server/templates/base.html:26-102`

- [ ] **Step 1: Rewrite the bootstrap for palette + mode**

Replace the theme block (keep the density and sidebar blocks below it untouched). The mode logic is the current logic verbatim, renamed onto `data-mode`; a new palette resolver sets `data-theme`:

```html
  <script>
    (function () {
      var d = document.documentElement;
      var META = { light: "#f8fafc", dark: "#0f172a", "hc-light": "#ffffff", "hc-dark": "#000000" };
      var EXPLICIT = { light: 1, dark: 1, "hc-light": 1, "hc-dark": 1 };
      var PALETTES = { "blue-harbor":1, "cobalt":1, "ink-ice":1, "arctic":1, "deep-sea":1, "royal-navy":1 };
      function cookie(n) {
        var m = document.cookie.match("(?:^|; )" + n + "=([^;]*)");
        return m ? decodeURIComponent(m[1]) : null;
      }
      function readPref(name) {
        var p = cookie(name);
        if (!p) { try { p = localStorage.getItem(name); } catch (e) {} }
        return p;
      }
      function resolveMode(p) {
        if (EXPLICIT[p]) return p;
        var dark = matchMedia("(prefers-color-scheme: dark)").matches;
        var hc = matchMedia("(prefers-contrast: more)").matches;
        if (hc) return dark ? "hc-dark" : "hc-light";
        return dark ? "dark" : "light";
      }
      function resolvePalette(p) { return PALETTES[p] ? p : "blue-harbor"; }
      function applyMode(p) {
        var t = resolveMode(p);
        d.setAttribute("data-mode", t);
        var m = document.querySelector('meta[name="theme-color"]');
        if (m) m.setAttribute("content", META[t] || META.light);
      }
      function applyPalette(p) { d.setAttribute("data-theme", resolvePalette(p)); }
      applyMode(readPref("lc-mode"));
      applyPalette(readPref("lc-palette"));
      function onSystemChange() {
        var p = readPref("lc-mode");
        if (!EXPLICIT[p]) applyMode(p);
      }
      try {
        matchMedia("(prefers-color-scheme: dark)").addEventListener("change", onSystemChange);
        matchMedia("(prefers-contrast: more)").addEventListener("change", onSystemChange);
      } catch (e) {}
      window.__lcSetMode = function (p) {
        try { if (p) localStorage.setItem("lc-mode", p); else localStorage.removeItem("lc-mode"); } catch (e) {}
        applyMode(p);
      };
      window.__lcSetPalette = function (p) {
        try { if (p) localStorage.setItem("lc-palette", p); else localStorage.removeItem("lc-palette"); } catch (e) {}
        applyPalette(p);
      };
      var FLIP = { light: "dark", dark: "light", "hc-light": "hc-dark", "hc-dark": "hc-light" };
      window.__lcToggleTheme = function () {
        var next = FLIP[d.getAttribute("data-mode")] || "dark";
        window.__lcSetMode(next);
        try {
          fetch("/settings/theme", {
            method: "POST",
            headers: { "Content-Type": "application/x-www-form-urlencoded" },
            body: "theme=" + encodeURIComponent(next),
          });
        } catch (e) {}
      };
    })();
  </script>
```

Note: the `POST /settings/theme` body key stays `theme=` because `ThemeForm.theme` is the mode field (renaming the wire key is out of scope; only the column/cookie renamed).

- [ ] **Step 2: Migrate the old `lc-theme` cookie name once (compat)**

Existing signed-in users already got an `lc-mode` cookie stamped by Task 5's middleware on their next authed request, so no client migration is needed. Anonymous/pre-auth visitors with only an old `lc-theme` localStorage value: add a one-line compat read so their mode is not lost. Immediately after `applyMode(readPref("lc-mode"));` add:

```html
      // one-time compat: adopt a pre-rename lc-theme value if lc-mode is unset
      if (!readPref("lc-mode")) {
        var legacy = readPref("lc-theme");
        if (legacy) window.__lcSetMode(EXPLICIT[legacy] ? legacy : "");
      }
```

- [ ] **Step 3: Build + verify no-flash**

Run: `just build-css` then `just dev-web-local`.
Manually: hard-reload on each palette/mode (set via devtools or the picker in Task 10) and confirm no flash-of-wrong-theme. Confirm `document.documentElement` shows both `data-theme` and `data-mode`.

- [ ] **Step 4: Commit**

```bash
git add server/templates/base.html
git commit -F- <<'EOF'
feat(theme): two-axis no-flash bootstrap (palette + mode)

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 10: Appearance picker - palette swatches + mode control

**Files:**
- Modify: `server/templates/settings/page.html:198-232` (appearance form)
- Modify: `server/locales/en/*.ftl` (new strings)

- [ ] **Step 1: Replace the theme `<select>` with a palette swatch group + a mode select**

In the appearance `<form>` (198), keep `action="/settings/appearance"`. Replace the single theme select (203-210) with a palette radio-swatch group plus the existing mode select (renamed control, still `name="theme"` on the wire):

```html
<fieldset class="lc-set-field">
  <legend class="lc-set-label">{{ "settings-palette"|t }}</legend>
  <div class="lc-palette-grid" role="radiogroup" aria-label="{{ "settings-palette"|t }}">
    {% for p in ["blue-harbor","cobalt","ink-ice","arctic","deep-sea","royal-navy"] %}
    <label class="lc-palette-swatch" data-palette="{{ p }}">
      <input type="radio" name="palette" value="{{ p }}" class="sr-only"
             {% if user.theme_palette_or_default() == p %}checked{% endif %}
             onchange="window.__lcSetPalette&&window.__lcSetPalette(this.value)">
      <span class="lc-palette-preview" aria-hidden="true"></span>
      <span class="lc-palette-name">{{ ("palette-" ~ p)|t }}</span>
    </label>
    {% endfor %}
  </div>
</fieldset>

<div class="lc-set-field">
  <label class="lc-set-label" for="lc-mode">{{ "settings-mode"|t }}</label>
  <select id="lc-mode" class="input" name="theme" aria-label="{{ "settings-mode"|t }}"
          onchange="window.__lcSetMode&&window.__lcSetMode(this.value==='system'?'':this.value)">
    <option value="system"{% if user.theme_mode_or_system() == "system" %} selected{% endif %}>{{ "theme-system"|t }}</option>
    <option value="light"{% if user.theme_mode_or_system() == "light" %} selected{% endif %}>{{ "theme-light"|t }}</option>
    <option value="dark"{% if user.theme_mode_or_system() == "dark" %} selected{% endif %}>{{ "theme-dark"|t }}</option>
    <option value="hc-light"{% if user.theme_mode_or_system() == "hc-light" %} selected{% endif %}>{{ "theme-hc-light"|t }}</option>
    <option value="hc-dark"{% if user.theme_mode_or_system() == "hc-dark" %} selected{% endif %}>{{ "theme-hc-dark"|t }}</option>
  </select>
</div>
```

The swatch preview colors come from the palette tokens: add to `server/assets/tailwind.css` `@layer components` a `.lc-palette-preview` that paints three bands using the palette vars (it inherits the palette when the swatch label carries `data-theme`; to preview ALL palettes at once, scope each swatch: `.lc-palette-swatch[data-palette="cobalt"] .lc-palette-preview { --sw-accent:#0f62fe; --sw-surface:#f5f9ff; --sw-side:#061932; }` for the six, then the preview paints `linear-gradient` bands from `--sw-side / --sw-surface / --sw-accent`). Keep it small (a 2-3 band chip).

- [ ] **Step 2: Add locale strings**

In `server/locales/en/` (the file that holds `theme-system` etc.; find with `grep -rl "theme-system" server/locales/en`):

```ftl
settings-palette = Palette
settings-mode = Mode
palette-blue-harbor = Blue Harbor
palette-cobalt = Cobalt Workspace
palette-ink-ice = Ink + Ice
palette-arctic = Arctic Messenger
palette-deep-sea = Deep Sea Cyan
palette-royal-navy = Royal Navy
```

- [ ] **Step 3: Build + manual check**

Run: `just build-css` then `just dev-web-local`.
Manually: open Settings > Appearance. Selecting a palette recolors the app instantly (no reload); selecting a mode recolors instantly; Save persists (reload keeps the choice); a second browser/profile logged in as the same user shows the saved palette+mode (cross-device cookie sync).

- [ ] **Step 4: Commit**

```bash
git add server/templates/settings/page.html server/assets/tailwind.css server/locales/en
git commit -F- <<'EOF'
feat(theme): appearance picker with palette swatches + mode

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 11: Theme/component gallery (proof + regression harness)

**Files:**
- Create: `server/templates/dev/theme_gallery.html`
- Modify: `server/src/routes/mod.rs` (register `GET /dev/theme-gallery`, non-prod/admin gated)
- Create: `server/src/routes/dev.rs` (or add to an existing dev module) with the handler

- [ ] **Step 1: Add the handler (gated)**

Create `server/src/routes/dev.rs`:

```rust
use axum::response::{Html, IntoResponse, Response};
use askama::Template;
use crate::routes::AppState;
use axum::extract::State;

#[derive(Template)]
#[template(path = "dev/theme_gallery.html")]
struct ThemeGallery {
    palettes: Vec<&'static str>,
    modes: Vec<&'static str>,
}

/// GET /dev/theme-gallery - renders every shared component across all palettes x
/// modes. Gated to non-production builds (or admin) - never a user-facing route.
pub async fn theme_gallery(State(_state): State<AppState>) -> Response {
    // Gate: only when debug assertions are on (dev builds). Adjust to an admin
    // check if you want it reachable in a staging build.
    if !cfg!(debug_assertions) {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let tpl = ThemeGallery {
        palettes: vec!["blue-harbor","cobalt","ink-ice","arctic","deep-sea","royal-navy"],
        modes: vec!["light","dark","hc-light","hc-dark"],
    };
    Html(tpl.render().unwrap_or_default()).into_response()
}
```

Register in `server/src/routes/mod.rs` (near other `get` routes):

```rust
        .route("/dev/theme-gallery", get(dev::theme_gallery))
```

Add `mod dev;` / `pub use` as the module layout requires.

- [ ] **Step 2: Add the template**

Create `server/templates/dev/theme_gallery.html`. For each palette x mode, render a bordered panel that sets both attributes on a wrapper (`<div data-theme="{{p}}" data-mode="{{m}}">`) and shows: buttons (`.btn` primary/secondary/ghost/danger), an `.input`, a `.card`, all four `.alert*`, a `.lc-table` sample, message-actor badges, and a mini message row with a reaction pill. Example cell:

```html
{% for p in palettes %}{% for m in modes %}
<section class="theme-cell" data-theme="{{ p }}" data-mode="{{ m }}"
         style="background:var(--surface); color:var(--content); border:1px solid var(--border); padding:1rem; border-radius:.5rem;">
  <h3 style="color:var(--content-muted)">{{ p }} / {{ m }}</h3>
  <div class="lc-action-row">
    <button class="btn btn-primary">Primary</button>
    <button class="btn btn-secondary">Secondary</button>
    <button class="btn btn-ghost">Ghost</button>
    <button class="btn btn-danger">Danger</button>
  </div>
  <input class="input" placeholder="Input">
  <div class="card">Card</div>
  <div class="alert alert-success">Success</div>
  <div class="alert alert-warning">Warning</div>
  <div class="alert alert-danger">Danger</div>
  <div class="alert alert-info">Info</div>
  <table class="lc-table"><tr class="lc-table-row"><td class="lc-table-cell">Row</td></tr></table>
  <span class="bg-webhook-surface text-webhook-content">webhook</span>
</section>
{% endfor %}{% endfor %}
```

- [ ] **Step 3: Build + verify all 24 combos**

Run: `just build-css` then `just dev-web-local`, open `/dev/theme-gallery`.
Manually: all 24 cells render with correct palette colors; no cell shows unstyled/black-on-black; sidebar/rail preview colors differ per palette; HC cells are visibly higher-contrast.

- [ ] **Step 4: Contrast check (HC AAA + core pairs)**

Add `server/scripts/contrast-check.mjs` (run with `bun server/scripts/contrast-check.mjs`) that parses `main.css`, computes WCAG contrast for `content/surface`, `content-muted/surface`, `accent-content/accent` for every palette x mode, and asserts >= 4.5 (AA) for normal modes and >= 7 (AAA) for `hc-*`. Print a table; exit non-zero on any failure. (Standard relative-luminance formula; ~40 lines.)

Run: `bun server/scripts/contrast-check.mjs`.
Expected: all pass; fix any failing token by darkening/lightening until it passes, then rebuild.

- [ ] **Step 5: Commit**

```bash
git add server/templates/dev/theme_gallery.html server/src/routes/dev.rs server/src/routes/mod.rs server/scripts/contrast-check.mjs
git commit -F- <<'EOF'
feat(theme): dev theme/component gallery + contrast check

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 12: Component retune + .lc-table

**Files:**
- Modify: `server/assets/tailwind.css` (`@layer components`)

- [ ] **Step 1: Add the `.lc-table` family**

In `@layer components` add (tokenized, matches the mockup table look):

```css
.lc-table { width:100%; border-collapse:collapse; }
.lc-table-head { text-align:left; color:var(--content-muted); font-weight:600; }
.lc-table-row { border-bottom:1px solid var(--border); }
.lc-table-row:hover { background:var(--surface-sunken); }
.lc-table-cell { padding:.5rem .75rem; }
```

- [ ] **Step 2: Retune existing component classes to the mockup**

Adjust ONLY radii/shadow/spacing/weight of `.btn`, `.card`, `.input`, `.alert*` to match the mockups (e.g. buttons `border-radius:.5rem`, cards `border-radius:.75rem` with `box-shadow: 0 1px 2px rgb(0 0 0 / .04)`). Do not change their token colors. Keep the diffs minimal and comment each.

- [ ] **Step 3: Build + gallery check**

Run: `just build-css`, reload `/dev/theme-gallery`.
Manually: components match the mockup shapes across all palettes; nothing regressed.

- [ ] **Step 4: Commit**

```bash
git add server/assets/tailwind.css
git commit -F- <<'EOF'
feat(theme): add .lc-table; retune components to mockup spec

#LC-541

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
```

---

## Task 13: Full P1 verification pass (acceptance criteria)

**Files:** none (verification only)

- [ ] **Step 1: Automated**

Run: `just check` (fmt+clippy+compile) and `just test`.
Expected: all green.

- [ ] **Step 2: Build + contrast**

Run: `just build-css` and `bun server/scripts/contrast-check.mjs`.
Expected: build clean; contrast all pass (AAA for hc-*).

- [ ] **Step 3: Manual AC checklist (via `just dev-web-local`)**

- [ ] AC1: all 6 palettes render in light/dark/hc-light/hc-dark on `/dev/theme-gallery`, the room view, and Settings.
- [ ] AC2: hard-reload shows no flash-of-wrong-theme for several palette/mode combos.
- [ ] AC3: picker applies palette + mode instantly (no reload); Save persists across reload; second browser as same user shows saved palette+mode; sidebar quick-toggle still flips mode.
- [ ] AC4: a user row with `theme_mode='dark'`, `theme_palette` NULL renders blue-harbor dark (today's look) with no action.
- [ ] AC5: every palette's hc-light/hc-dark passes AAA (from Step 2).
- [ ] AC6: `grep -rn "#[0-9a-fA-F]\{6\}" server/templates | grep -v vendor` shows no NEW hardcoded hex introduced by this phase; `dark:` utilities fire on dark and hc-dark.
- [ ] AC7: app boots; existing tests pass.

- [ ] **Step 4: Push branch + open PR (HOLD for user go-ahead)**

Do NOT push or open the PR until the user confirms. When cleared:

```bash
git push --set-upstream origin docs/LC-541-redesign-p1-foundation
tea pr create --repo a8n-tools/lets-chat --login a8n --head docs/LC-541-redesign-p1-foundation --base main \
  --title "LC-541 P1: six-palette theme foundation" \
  --description "<file>"
```

---

## Self-review notes

- Spec coverage: model B (T6,T9), 24-block token system split into constant+varying (T7,T8, HC authored+verified in T11 step 4), bootstrap port (T9), rename+migration+persistence (T1-T5), picker (T10), component retune + .lc-table (T12), gallery (T11), backward-compat (T1 preserves data, AC4), acceptance (T13). All spec sections map to a task.
- HC blocks: the plan authors hc-light/hc-dark per palette in T11's contrast step rather than fabricating 480 hex values up front; the contrast script is the objective gate. If preferred, split T8 to include HC blocks explicitly once values are derived - the token slots and selectors are already defined.
- Type consistency: `theme_mode`/`theme_palette` fields, `theme_mode_or_system`/`theme_palette_or_default`, `set_user_theme_mode`/`set_user_theme_palette`, cookies `lc-mode`/`lc-palette`, wire keys `theme` (mode, unchanged) + `palette`, JS globals `__lcSetMode`/`__lcSetPalette`/`__lcToggleTheme` are used consistently across T2-T10.
- Open follow-up folded into P2+: no product-surface redesign here.
