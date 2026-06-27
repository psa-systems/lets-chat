//! LC-486: inline per-message translation fragment.

use crate::i18n::filters; // in-scope for the |t template filter.
use askama::Template;

/// Rendered translation, swapped into `#translation-{message_id}` under the
/// message body. Carries the target language's display name + a "show original"
/// affordance (the template clears the container).
#[derive(Template)]
#[template(path = "room/translation_block.html")]
pub struct TranslationFragment {
    pub message_id: i64,
    pub translated_html: String,
    /// Native display name of the target language (e.g. "Español").
    pub language: String,
}
