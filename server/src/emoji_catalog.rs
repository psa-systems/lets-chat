//! LC-389: the shared emoji catalog. Both the reaction picker
//! (`routes::reactions::get_picker`) and the composer picker
//! (`routes::emoji_complete::get_picker`) render the full `emojis`-crate set
//! organized into the eight standard Unicode categories, so the two surfaces
//! stay byte-identical in ordering and labeling. This is the single source of
//! that grouping.

/// The eight standard Unicode emoji categories, in `emojis::Group` order. Each
/// entry is `(group, slug, i18n label key, representative tab glyph)`:
/// - `slug` is the stable `data-lc-emoji-cat` / `data-lc-emoji-tab` hook the
///   filter-collapse and tab-scroll JS key on (must stay in sync, but it is
///   surface-agnostic so both pickers share it).
/// - the label key resolves through `i18n::translate_current`.
/// - `Group::emojis()` yields the group's members in Unicode order, so the grid
///   matches every other emoji UI the user has seen.
pub const GROUPS: &[(emojis::Group, &str, &str, &str)] = &[
    (
        emojis::Group::SmileysAndEmotion,
        "smileys",
        "partials-reaction-cat-smileys",
        "😀",
    ),
    (
        emojis::Group::PeopleAndBody,
        "people",
        "partials-reaction-cat-people",
        "👋",
    ),
    (
        emojis::Group::AnimalsAndNature,
        "nature",
        "partials-reaction-cat-nature",
        "🐻",
    ),
    (
        emojis::Group::FoodAndDrink,
        "food",
        "partials-reaction-cat-food",
        "🍔",
    ),
    (
        emojis::Group::TravelAndPlaces,
        "travel",
        "partials-reaction-cat-travel",
        "✈️",
    ),
    (
        emojis::Group::Activities,
        "activities",
        "partials-reaction-cat-activities",
        "⚽",
    ),
    (
        emojis::Group::Objects,
        "objects",
        "partials-reaction-cat-objects",
        "💡",
    ),
    (
        emojis::Group::Symbols,
        "symbols",
        "partials-reaction-cat-symbols",
        "🔣",
    ),
    (
        emojis::Group::Flags,
        "flags",
        "partials-reaction-cat-flags",
        "🚩",
    ),
];

/// Lowercased search-keyword string for an emoji: its human name followed by
/// every shortcode, space-joined. Both pickers filter case-insensitively
/// against this (so "heart" surfaces the heart family, "+1" / "thumbsup" find
/// 👍), so building it once here keeps the two search behaviours identical.
pub fn keywords(emoji: &emojis::Emoji) -> String {
    let mut s = emoji.name().to_string();
    for sc in emoji.shortcodes() {
        s.push(' ');
        s.push_str(sc);
    }
    s.make_ascii_lowercase();
    s
}
