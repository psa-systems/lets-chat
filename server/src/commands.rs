//! LC-76: built-in slash-command registry (metadata).
//!
//! Execution lives in `routes::slash`; this module is the single source of
//! truth for the built-in command list used by `/help` and the autocomplete
//! dropdown. Custom (admin-defined) commands come from `db::slash`.

/// Metadata for one built-in slash command.
pub struct BuiltinCommand {
    /// Command name without the leading slash (lowercase).
    pub name: &'static str,
    pub description: &'static str,
    /// Usage hint shown in `/help` and the autocomplete dropdown.
    pub usage: &'static str,
    /// When true, only org admins may run it (RBAC). None of the current
    /// built-ins are admin-only; the flag exists so the registry + dispatch
    /// honor it uniformly with custom commands.
    pub admin_only: bool,
}

/// The built-in commands. Order is the `/help` display order.
pub const BUILTINS: &[BuiltinCommand] = &[
    BuiltinCommand {
        name: "help",
        description: "List available slash commands.",
        usage: "/help",
        admin_only: false,
    },
    BuiltinCommand {
        name: "me",
        description: "Post an action in the third person.",
        usage: "/me <action>",
        admin_only: false,
    },
    BuiltinCommand {
        name: "shrug",
        description: "Append \u{00af}\\_(\u{30c4})_/\u{00af} to your text.",
        usage: "/shrug [text]",
        admin_only: false,
    },
    BuiltinCommand {
        name: "poll",
        description: "Post a poll.",
        usage: "/poll \"Question\" \"Option A\" \"Option B\" ...",
        admin_only: false,
    },
    BuiltinCommand {
        name: "remind",
        description: "Post a note and remind yourself about it later.",
        usage: "/remind <15m|1h|3h|1d> <text>",
        admin_only: false,
    },
    // LC-526: recognition. Posts a kudos card and tallies the leaderboard.
    BuiltinCommand {
        name: "kudos",
        description: "Give someone kudos (see the leaderboard at /kudos).",
        usage: "/kudos @user <reason>",
        admin_only: false,
    },
    // LC-492: in-channel AI assistant. Functions only when the operator has
    // configured an LLM and the room has the assistant enabled.
    BuiltinCommand {
        name: "ask",
        description: "Ask the room's AI assistant a question.",
        usage: "/ask <question>",
        admin_only: false,
    },
];

/// Look up a built-in by name (lowercase, no leading slash).
pub fn find_builtin(name: &str) -> Option<&'static BuiltinCommand> {
    BUILTINS.iter().find(|c| c.name == name)
}
