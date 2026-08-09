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
    /// LC-674: a concrete example, shown under the abstract usage for commands
    /// with non-trivial syntax (a filled-in `/remind 1h ...` teaches faster than
    /// `<15m|1h|3h|1d> <text>`). `None` for commands with no arguments.
    pub example: Option<&'static str>,
    /// When true, only org admins may run it (RBAC). None of the current
    /// built-ins are admin-only; the flag exists so the registry + dispatch
    /// honor it uniformly with custom commands.
    pub admin_only: bool,
    /// LC-686: when true, the command is part of the LLM/AI surface and is
    /// listed only when AI is available for the viewer's room (runtime flag on,
    /// an LLM configured, and the viewer privileged) - the same gate the
    /// per-message AI menu items use. Dispatch still enforces this server-side.
    pub llm_only: bool,
}

/// The built-in commands. Order is the `/help` display order.
pub const BUILTINS: &[BuiltinCommand] = &[
    BuiltinCommand {
        name: "help",
        description: "List available slash commands.",
        usage: "/help",
        example: None,
        admin_only: false,
        llm_only: false,
    },
    BuiltinCommand {
        name: "me",
        description: "Post an action in the third person.",
        usage: "/me <action>",
        example: Some("/me waves hello"),
        admin_only: false,
        llm_only: false,
    },
    BuiltinCommand {
        name: "shrug",
        description: "Append \u{00af}\\_(\u{30c4})_/\u{00af} to your text.",
        usage: "/shrug [text]",
        example: Some("/shrug oh well"),
        admin_only: false,
        llm_only: false,
    },
    BuiltinCommand {
        name: "poll",
        description: "Post a poll.",
        usage: "/poll \"Question\" \"Option A\" \"Option B\" ...",
        example: Some("/poll \"Lunch?\" \"Tacos\" \"Pizza\""),
        admin_only: false,
        llm_only: false,
    },
    BuiltinCommand {
        name: "remind",
        description: "Post a note and remind yourself about it later.",
        usage: "/remind <15m|1h|3h|1d> <text>",
        example: Some("/remind 1h Check the deploy"),
        admin_only: false,
        llm_only: false,
    },
    // LC-526: recognition. Posts a kudos card and tallies the leaderboard.
    BuiltinCommand {
        name: "kudos",
        description: "Give someone kudos (see the leaderboard at /kudos).",
        usage: "/kudos @user <reason>",
        example: Some("/kudos @dave for the code review"),
        admin_only: false,
        llm_only: false,
    },
    // LC-492: in-channel AI assistant. Functions only when the operator has
    // configured an LLM and the room has the assistant enabled.
    // LC-686: llm_only, so it is hidden from the slash list when AI is off.
    BuiltinCommand {
        name: "ask",
        description: "Ask the room's AI assistant a question.",
        usage: "/ask <question>",
        example: Some("/ask what did we decide about the launch?"),
        admin_only: false,
        llm_only: true,
    },
];

/// Look up a built-in by name (lowercase, no leading slash).
pub fn find_builtin(name: &str) -> Option<&'static BuiltinCommand> {
    BUILTINS.iter().find(|c| c.name == name)
}
