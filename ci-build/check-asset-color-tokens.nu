#!/usr/bin/env nu

# Guard the design-token layer in browser assets (LC-735).
#
# tailwind.config.js uses `extend`, so the raw numbered palette (text-slate-700,
# bg-blue-500, ...) still compiles and still ships. A raw shade is fixed for all
# four modes, so it breaks in the ones nobody was looking at: the offline outbox
# banner rendered text-slate-700 on bg-surface-elevated, ~1.6:1 in dark mode.
# Templates get eyeballed in the theme gallery; these JS files inject markup only
# in transient states (offline, failed send, active search row), which is exactly
# why they need a mechanical check. Vendor bundles carry their own styling and are
# out of scope.

# Tailwind utilities that take a color, and the full default hue set. Matching
# the prefix (rather than any `<hue>-<n>` token) keeps CSS var names and message
# ids from tripping the guard.
const PREFIXES = "bg|text|border|divide|ring|ring-offset|outline|decoration|placeholder|caret|accent|shadow|fill|stroke|from|via|to"
const HUES = "slate|gray|grey|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose"

# LC-736: the same defect also lives one layer down. The keyboard-highlight rule
# for `[role="option"][aria-selected="true"]` shipped a literal `rgb(241 245 249)`
# in main.css and outranked the tokenized `.lc-search-row` rule, so tokenizing the
# JS half alone left the highlight fixed at slate-100 in all four modes. Every
# selection highlight is keyed on `aria-selected`, so that attribute is the shape
# to guard: any background it paints must come from a `var(--...)` token.
const CSS_FILE = "server/assets/main.css"
const SELECTED_ATTR = '[aria-selected="true"]'

def raw_palette_utilities [] {
    let pattern = $"\\b\(($PREFIXES)\)-\(($HUES)\)-[0-9]{2,3}\\b"
    let files = (glob server/assets/**/*.js --exclude ["**/vendor/**"] | sort)
    if ($files | is-empty) {
        print --stderr "No JS assets found under server/assets/"
        exit 1
    }

    let problems = (
        $files | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row| $row.item =~ $pattern }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } | flatten
    )
    { files: ($files | length), problems: $problems }
}

# Split on "}" so each chunk holds at most one rule body; the body is whatever
# follows the chunk's last "{" (an @media wrapper contributes an earlier one).
def untokenized_selection_backgrounds [] {
    open --raw $CSS_FILE
    | split row "}"
    | where {|chunk| ($chunk | str contains $SELECTED_ATTR) and ($chunk | str contains "{") }
    | each {|chunk|
        $chunk
        | split row "{"
        | last
        | lines
        | each {|line| $line | str trim }
        | where {|line| $line =~ '^background(-color)?\s*:' }
        | where {|line| not ($line | str contains "var(--") }
        | each {|line| $"($CSS_FILE): ($line)" }
    }
    | flatten
}

def main [] {
    let js = (raw_palette_utilities)
    if ($js.problems | is-not-empty) {
        print --stderr "Raw palette utilities found in browser assets; use the semantic tokens from tailwind.config.js (text-content, text-content-muted, border-border, bg-surface-sunken, ...):"
        for p in $js.problems { print --stderr $"  ($p)" }
        exit 1
    }

    let css = (untokenized_selection_backgrounds)
    if ($css | is-not-empty) {
        print --stderr $"Untokenized background on an ($SELECTED_ATTR) rule; paint the selection highlight from a var\(--...\) token \(--surface-sunken\):"
        for p in $css { print --stderr $"  ($p)" }
        exit 1
    }

    print $"Asset color tokens OK across ($js.files) JS files and every ($SELECTED_ATTR) rule in ($CSS_FILE)."
}
