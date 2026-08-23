//! LC-100: internationalization.
//!
//! Message catalogs live at `server/locales/<lang>/*.ftl` (Fluent) and are
//! embedded at compile time by `fluent-templates::static_loader!`, so the
//! binary stays single-file. The active locale for a request is carried in a
//! `tokio::task_local!` set by `locale_middleware`; the Askama `t` / `tn`
//! filters read it, so templates localize a string with `{{ "key" | t }}`
//! without threading a locale through every view struct.
//!
//! Locale resolution order: the signed-in user's saved `users.locale`, else the
//! request's `Accept-Language`, else English. An unknown locale falls back to
//! English (Fluent's own `fallback_language`), so a render never breaks.

use fluent_templates::{static_loader, Loader};
use unic_langid::LanguageIdentifier;

static_loader! {
    static LOCALES = {
        locales: "./locales",
        fallback_language: "en",
    };
}

/// The source/fallback locale.
pub fn fallback() -> LanguageIdentifier {
    "en".parse().expect("en is a valid langid")
}

tokio::task_local! {
    /// Active locale for the current request. Set by `locale_middleware`.
    pub static CURRENT_LOCALE: LanguageIdentifier;
}

/// Available locales, sorted by code, for the settings dropdown.
pub fn available() -> Vec<LanguageIdentifier> {
    let mut v: Vec<LanguageIdentifier> = LOCALES.locales().cloned().collect();
    v.sort_by_key(|l| l.to_string());
    v
}

/// True if `code` parses to a locale we ship.
pub fn is_supported(code: &str) -> bool {
    match code.parse::<LanguageIdentifier>() {
        Ok(want) => LOCALES.locales().any(|l| l.language == want.language),
        Err(_) => false,
    }
}

/// Look up a message in a specific locale (falls back to English internally).
pub fn translate(lang: &LanguageIdentifier, key: &str) -> String {
    LOCALES.lookup(lang, key)
}

/// The current request's locale as a BCP-47 code (e.g. `en`, `es`) for the
/// `<html lang>` attribute (LC-354). Falls back to the default locale outside a
/// request scope (e.g. a render that runs off the task-local).
pub fn current_lang_code() -> String {
    CURRENT_LOCALE
        .try_with(|l| l.to_string())
        .unwrap_or_else(|_| fallback().to_string())
}

/// Look up a message in the current request's locale.
pub fn translate_current(key: &str) -> String {
    CURRENT_LOCALE
        .try_with(|l| translate(l, key))
        .unwrap_or_else(|_| translate(&fallback(), key))
}

/// Look up a plural-aware message in the current locale, passing `count` as the
/// Fluent `$count` argument so the catalog can select the right plural form.
pub fn translate_count(key: &str, count: i64) -> String {
    let mut args = std::collections::HashMap::new();
    args.insert(
        std::borrow::Cow::Borrowed("count"),
        fluent_templates::fluent_bundle::FluentValue::from(count),
    );
    let do_lookup = |l: &LanguageIdentifier| LOCALES.lookup_with_args(l, key, &args);
    CURRENT_LOCALE
        .try_with(|l| do_lookup(l))
        .unwrap_or_else(|_| do_lookup(&fallback()))
}

/// Resolve the best-matching shipped locale for a request. `user_locale` is the
/// signed-in user's saved preference (`None` => not set); `accept_language` is
/// the raw header. Matching is on the primary language subtag only (region is
/// ignored in v1), and anything unrecognized lands on the fallback.
pub fn resolve(user_locale: Option<&str>, accept_language: Option<&str>) -> LanguageIdentifier {
    if let Some(code) = user_locale {
        if let Some(l) = match_supported(code) {
            return l;
        }
    }
    if let Some(header) = accept_language {
        for part in header.split(',') {
            // Strip any ";q=..." weight; we take document order, good enough v1.
            let tag = part.split(';').next().unwrap_or("").trim();
            if tag.is_empty() {
                continue;
            }
            if let Some(l) = match_supported(tag) {
                return l;
            }
        }
    }
    fallback()
}

/// Human-readable, native-language name for a locale code, for the settings
/// dropdown. Falls back to the code itself for an unmapped locale.
pub fn native_name(code: &str) -> &'static str {
    match code {
        c if c.starts_with("en") => "English",
        c if c.starts_with("es") => "Español",
        _ => "",
    }
}

/// LC-486: English-language name of a locale code, for an LLM translation
/// prompt ("translate into Spanish"). Falls back to English for unknown codes.
pub fn language_name(code: &str) -> &'static str {
    match code {
        c if c.starts_with("es") => "Spanish",
        _ => "English",
    }
}

/// Return the shipped locale whose language subtag matches `code`, if any.
fn match_supported(code: &str) -> Option<LanguageIdentifier> {
    let want = code.parse::<LanguageIdentifier>().ok()?;
    LOCALES
        .locales()
        .find(|l| l.language == want.language)
        .cloned()
}

/// Askama custom filters. Generated template code calls `filters::t(..)`, so
/// each view module with a `#[derive(Template)]` struct does
/// `use crate::i18n::filters;`.
pub mod filters {
    /// `{{ "key" | t }}` -> the message in the current request locale.
    pub fn t(key: &str) -> askama::Result<String> {
        Ok(super::translate_current(key))
    }

    /// `{{ "key" | tn(count) }}` -> a plural-aware message; `count` is exposed
    /// to the catalog as the Fluent `$count` selector argument.
    pub fn tn(key: &str, count: i64) -> askama::Result<String> {
        Ok(super::translate_count(key, count))
    }

    /// `{{ ""|lang }}` -> the current request locale's BCP-47 code, for the
    /// `<html lang>` attribute (LC-354). The filtered input is ignored.
    pub fn lang(_: &str) -> askama::Result<String> {
        Ok(super::current_lang_code())
    }

    /// `{{ row.created_at|iso }}` -> the same instant as an unambiguous UTC
    /// RFC3339 string, for a `<time datetime>` attribute (LC-746). SQLite
    /// stores `YYYY-MM-DD HH:MM:SS` UTC, which browsers parse as LOCAL time.
    /// Lives here because this is the module every view imports its filters
    /// from; it does no translation.
    pub fn iso(ts: &str) -> askama::Result<String> {
        Ok(crate::views::room::to_iso_utc(ts))
    }

    /// LC-781 (F11): `{{ user_id|avatar_url }}` -> the versioned,
    /// immutable-cacheable avatar URL `/avatars/{id}?v={token}`. The token is the
    /// avatar file's mtime (cached in `crate::avatar_version`), so a re-upload
    /// changes it - busting the browser's immutable cache - while an unchanged
    /// avatar costs no revalidation. A user with no custom avatar gets `?v=0`,
    /// which is harmless: the route serves their default SVG `no-cache`.
    pub fn avatar_url(user_id: impl AsRef<str>) -> askama::Result<String> {
        let id = user_id.as_ref();
        Ok(format!(
            "/avatars/{id}?v={}",
            crate::avatar_version::version_of(id)
        ))
    }

    /// LC-784: the bare avatar version token for `user_id` (no URL), so a
    /// template can hand it to client JS that builds `/avatars/{id}?v={token}`
    /// itself. The call/voice surfaces build their avatar URLs in JS from a
    /// WS-delivered id (they cannot use `avatar_url`), so the token rides
    /// alongside the id in a `data-*` attribute and the JS appends `?v=`. Same
    /// source as `avatar_url`, so both stay in step on a re-upload.
    pub fn avatar_version(user_id: impl AsRef<str>) -> askama::Result<String> {
        Ok(crate::avatar_version::version_of(user_id.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_en_and_es() {
        let en: LanguageIdentifier = "en".parse().unwrap();
        let es: LanguageIdentifier = "es".parse().unwrap();
        assert_eq!(translate(&en, "login-title"), "Sign in");
        assert_eq!(translate(&es, "login-title"), "Iniciar sesión");
    }

    #[test]
    fn current_lang_code_reflects_locale_and_falls_back() {
        // LC-354: outside a request scope, fall back to the default locale.
        assert_eq!(current_lang_code(), fallback().to_string());
        // Within a scoped locale, return its BCP-47 code (drives <html lang>).
        let es: LanguageIdentifier = "es".parse().unwrap();
        CURRENT_LOCALE.sync_scope(es, || {
            assert_eq!(current_lang_code(), "es");
        });
    }

    #[test]
    fn unknown_locale_falls_back_to_english() {
        let zz: LanguageIdentifier = "zz".parse().unwrap();
        // Fluent's fallback_language fills the gap for an unshipped locale.
        assert_eq!(translate(&zz, "login-title"), "Sign in");
    }

    #[test]
    fn resolve_prefers_user_then_header_then_fallback() {
        assert_eq!(resolve(Some("es"), None).language.as_str(), "es");
        assert_eq!(
            resolve(None, Some("es-ES,es;q=0.9")).language.as_str(),
            "es"
        );
        assert_eq!(resolve(Some("zz"), Some("zz")).language.as_str(), "en");
        assert_eq!(resolve(None, None).language.as_str(), "en");
    }

    #[test]
    fn available_includes_en_and_es() {
        let codes: Vec<String> = available().iter().map(|l| l.to_string()).collect();
        assert!(codes.iter().any(|c| c.starts_with("en")));
        assert!(codes.iter().any(|c| c.starts_with("es")));
    }
}
