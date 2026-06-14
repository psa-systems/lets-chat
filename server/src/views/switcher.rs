// LC-260: quick switcher (Ctrl/Cmd+K) result fragment. A flat, ranked list of
// rooms (current enclave), the viewer's DMs, and people, rendered into the
// palette modal's listbox. Reuses server/assets/search.js for keyboard nav, so
// each row is an `<a role="option" id=...>` anchor.
#[allow(unused_imports)]
use crate::i18n::filters;
use askama::Template;

#[derive(Template)]
#[template(path = "switcher/results.html")]
pub struct SwitcherResults {
    pub items: Vec<SwitcherItem>,
}

pub struct SwitcherItem {
    /// Navigation target: `/room/{id}` or `/dm/{peer_id}`.
    pub href: String,
    /// Leading glyph: `#` (text room), a speaker (voice room), or `@`
    /// (DM / person).
    pub glyph: String,
    /// Primary label (room name or person display name).
    pub label: String,
    /// Secondary hint (`@username` for people/DMs; empty for rooms).
    pub sublabel: String,
    /// Stable option id for the combobox `aria-activedescendant`.
    pub opt_id: String,
}
