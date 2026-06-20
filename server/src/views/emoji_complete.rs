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

/// LC-389: one labeled category in the composer picker - a section header + a
/// grid of click-to-insert buttons. `slug` is the `data-lc-emoji-cat` /
/// `data-lc-emoji-tab` hook (shared with the reaction picker's JS); `label` is
/// already translated; `tab_glyph` is the strip icon that scrolls to it.
pub struct EmojiCategory {
    pub slug: &'static str,
    pub label: String,
    pub tab_glyph: &'static str,
    pub suggestions: Vec<EmojiSuggestion>,
}

/// LC-316 / LC-389: the composer emoji picker panel. Originally a small "popular"
/// grid (LC-316); LC-389 expands it to the full `emojis`-crate set organized
/// into the eight standard categories (browsable via the tab strip + scroll)
/// plus the room's custom emojis, matching the reaction picker. Filtered
/// client-side (LC-274 + the LC-389 section-collapse) and inserted at the
/// textarea cursor; the `:shortcode:` autocomplete (LC-296) remains the
/// search-by-typing surface.
#[derive(Template)]
#[template(path = "partials/emoji_picker.html")]
pub struct EmojiPickerFragment {
    pub room_id: i64,
    pub categories: Vec<EmojiCategory>,
}
