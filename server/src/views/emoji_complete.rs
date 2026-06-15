//! LC-296: composer `:shortcode:` emoji autocomplete popover fragment.
//!
//! Mirrors `views::mentions` in shape: a small `<ul role="listbox">` the
//! composer swaps into `#lc-emoji-popover`. Two row kinds:
//! - `custom`: a per-enclave custom emoji; inserts its `:shortcode:` token (the
//!   markdown pipeline rewrites it to an `<img>` on render) and previews the
//!   image inline.
//! - `unicode`: a Unicode emoji from the `emojis` crate; inserts the literal
//!   glyph (rendered as plain text, no pipeline change needed).
#[allow(unused_imports)]
use crate::i18n::filters; // LC-316: in-scope for the `|t` filter in emoji_picker.html.
use askama::Template;

/// One autocomplete row. `insert` is the exact string the composer splices in
/// at the `:` token (a `:shortcode:` for custom, a glyph for unicode).
pub struct EmojiSuggestion {
    /// "custom" | "unicode" - selects the template branch.
    pub kind: &'static str,
    /// The text inserted into the textarea when this row is chosen.
    pub insert: String,
    /// Shortcode label shown to the user (without surrounding colons).
    pub shortcode: String,
    /// Unicode glyph, for the `unicode` branch's leading visual.
    pub glyph: Option<String>,
    /// Custom-emoji id, for the `custom` branch's `<img src="/api/emojis/{id}">`.
    pub emoji_id: Option<i64>,
    /// Human name (unicode) or empty; shown muted after the shortcode.
    pub name: String,
}

impl EmojiSuggestion {
    pub fn custom(shortcode: String, emoji_id: i64) -> Self {
        let insert = format!(":{shortcode}:");
        Self {
            kind: "custom",
            insert,
            shortcode,
            glyph: None,
            emoji_id: Some(emoji_id),
            name: String::new(),
        }
    }

    pub fn unicode(glyph: &str, shortcode: String, name: String) -> Self {
        Self {
            kind: "unicode",
            insert: glyph.to_string(),
            shortcode,
            glyph: Some(glyph.to_string()),
            emoji_id: None,
            name,
        }
    }
}

#[derive(Template)]
#[template(path = "partials/emoji_popover.html")]
pub struct EmojiPopoverFragment<'a> {
    pub results: &'a [EmojiSuggestion],
}

/// LC-316: the composer emoji picker panel - a browse-and-click grid of popular
/// Unicode emojis plus the room's custom emojis, filtered client-side (LC-274)
/// and inserted at the textarea cursor. Full-Unicode search-by-typing is the
/// `:shortcode:` autocomplete's job (LC-296); this is the click surface.
#[derive(Template)]
#[template(path = "partials/emoji_picker.html")]
pub struct EmojiPickerFragment {
    pub room_id: i64,
    pub suggestions: Vec<EmojiSuggestion>,
}
