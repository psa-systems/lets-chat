//! LC-486: inline per-message translation fragment.
//! LC-688: the target language is user-chosen (an in-block picker), not the
//! viewer's UI locale, so translation is not limited to the two shipped locales.

use crate::i18n::filters; // in-scope for the |t template filter.
use askama::Template;

/// One offerable translation target. `english` is the language name fed to the
/// LLM prompt ("translate into French"); `native` is the picker label; `code`
/// is the stable key used in the request + the translation cache. This is a
/// deliberately small, curated list of common languages - the LLM can translate
/// into any of them, and it is independent of the shipped UI (Fluent) locales.
pub struct TranslateTarget {
    pub code: &'static str,
    pub english: &'static str,
    pub native: &'static str,
}

/// The offered target languages (the picker order). English first as the most
/// common lingua franca; the rest roughly by speaker count.
pub const TARGETS: &[TranslateTarget] = &[
    TranslateTarget {
        code: "en",
        english: "English",
        native: "English",
    },
    TranslateTarget {
        code: "es",
        english: "Spanish",
        native: "Español",
    },
    TranslateTarget {
        code: "zh",
        english: "Chinese",
        native: "中文",
    },
    TranslateTarget {
        code: "hi",
        english: "Hindi",
        native: "हिन्दी",
    },
    TranslateTarget {
        code: "ar",
        english: "Arabic",
        native: "العربية",
    },
    TranslateTarget {
        code: "pt",
        english: "Portuguese",
        native: "Português",
    },
    TranslateTarget {
        code: "fr",
        english: "French",
        native: "Français",
    },
    TranslateTarget {
        code: "de",
        english: "German",
        native: "Deutsch",
    },
    TranslateTarget {
        code: "ru",
        english: "Russian",
        native: "Русский",
    },
    TranslateTarget {
        code: "ja",
        english: "Japanese",
        native: "日本語",
    },
    TranslateTarget {
        code: "ko",
        english: "Korean",
        native: "한국어",
    },
    TranslateTarget {
        code: "it",
        english: "Italian",
        native: "Italiano",
    },
];

/// Resolve the target language for a translate request. Prefers an explicit
/// `requested` code (from the picker), else the viewer's UI locale, else English.
/// Matching is on the primary language subtag (so `es-MX` maps to `es`), and an
/// unknown code falls through - the allowlist bounds what reaches the LLM prompt.
pub fn resolve_target(requested: Option<&str>, viewer_locale: &str) -> &'static TranslateTarget {
    let find = |code: &str| -> Option<&'static TranslateTarget> {
        let primary = code.split(['-', '_']).next().unwrap_or(code);
        TARGETS
            .iter()
            .find(|t| t.code.eq_ignore_ascii_case(primary))
    };
    requested
        .and_then(find)
        .or_else(|| find(viewer_locale))
        .unwrap_or(&TARGETS[0])
}

/// Rendered translation block, swapped into `#translation-{message_id}` under
/// the message body. Carries the in-block language picker (`targets` + the
/// `selected` code) and a "show original" affordance (the template clears the
/// container).
///
/// LC-694: `translated_html` is `None` in the picker-only state (the first
/// Translate click opens the picker and runs no LLM call, so an already-in-your-
/// language message never burns a wasted translation) and `Some(html)` once a
/// target has been chosen and translated.
#[derive(Template)]
#[template(path = "room/translation_block.html")]
pub struct TranslationFragment {
    pub message_id: i64,
    /// `None` = picker only (no AI call yet); `Some` = the rendered translation.
    pub translated_html: Option<String>,
    /// The chosen (or default) target's code, to mark the selected `<option>`.
    pub selected: &'static str,
    /// The full picker list (always [`TARGETS`]).
    pub targets: &'static [TranslateTarget],
}

#[cfg(test)]
mod tests {
    use super::resolve_target;

    #[test]
    fn resolve_target_prefers_request_then_viewer_then_english() {
        // Explicit request wins.
        assert_eq!(resolve_target(Some("fr"), "es").code, "fr");
        // Region subtag maps to the primary language.
        assert_eq!(resolve_target(Some("es-MX"), "en").code, "es");
        // Unknown request falls back to the viewer's locale.
        assert_eq!(resolve_target(Some("zz"), "de").code, "de");
        // No request: viewer locale.
        assert_eq!(resolve_target(None, "ja").code, "ja");
        // Neither known: English.
        assert_eq!(resolve_target(Some("zz"), "zz").code, "en");
        assert_eq!(resolve_target(None, "xx").code, "en");
    }
}
