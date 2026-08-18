#!/usr/bin/env nu

# Guard the UI convention classes the 2026-08-11 audit closed (LC-749).
#
# Each rule below closes a class that reopens the moment someone adds a file:
# the audit re-found several of them after an earlier issue had cleared the
# class once. One script, one CI step, one place to read the rules.
#
# Two rules of the same set already have their own guards and are NOT
# duplicated here; both run in the same `just check` / Check-workflow job:
#   - raw numbered palette utilities in `server/assets/**/*.js`
#     -> ci-build/check-asset-color-tokens.nu (LC-735, LC-736)
#   - U+2026 in the locale catalogs
#     -> ci-build/check-locale-ellipsis.nu (LC-750)
#
# A rule whose class is not clear yet carries `pending: "<issue>"`. It runs and
# prints its hits on every run but does not fail the build until that issue
# lands and deletes the marker; each pending rule's issue already carries "add
# the CI check" in its own acceptance criteria. Marking rather than commenting
# the rule out keeps the pattern executing, so a rule that silently stops
# matching shows up as a hit count going to zero instead of rotting behind a
# comment.
#
# Files are read with `open --raw`, never `grep -r`: server/templates/layout.html
# contains a literal NUL byte (a separator in an inline JS `join`), which makes
# grep skip that whole file as binary - and it is the file two of these rules
# match today. Tracked in LC-757.

# Tailwind utilities that take a color, and the full default hue set; same pair
# as ci-build/check-asset-color-tokens.nu, which guards the JS half.
const PREFIXES = "bg|text|border|divide|ring|ring-offset|outline|decoration|placeholder|caret|accent|shadow|fill|stroke|from|via|to"
const HUES = "slate|gray|grey|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose"

# The one exception LC-741 allows: the deliberately-dark fullscreen call stage.
# The line (or the line above it) must carry this marker in a comment naming
# why the literal is intended, so the exception is stated, not assumed.
const PALETTE_ALLOW = "lc-allow-palette"

# Character-for-character what `.btn-danger-outline` (main.css) expands to.
const DANGER_OUTLINE = "text-danger border border-danger-border hover:bg-danger-surface"

# A border utility paints Tailwind's `colors.gray[200]` default unless a color
# token comes with it: fixed light grey in all four modes and every palette.
const BARE_BORDER = '(^| )(border|border-[0-9]+|border-[trblxy](-[0-9]+)?)( |$)'
const BORDER_COLOR = 'border-(border|accent|danger|success|warning|transparent|current)'
const BARE_DIVIDE = '(^| )divide-[xy](-[0-9]+)?( |$)'
const DIVIDE_COLOR = 'divide-(border|accent|danger|success|warning|transparent|current)'

# Raw size utilities on an `<h1>`: the page-title size comes from `.lc-h1` or
# `.lc-display` (main.css), not from a per-page pick out of six.
const H1_RAW_SIZE = '<h1[^>]*text-(xs|sm|base|lg|xl|2xl|3xl|4xl|5xl)'

const EM_DASH = "\u{2014}"

# The em-dash rule sweeps every tracked text file, so it needs an extension
# allowlist rather than a glob; `justfile` and the Dockerfiles carry no
# extension and are matched by name.
const TEXT_EXTENSIONS = ["rs" "html" "js" "css" "ftl" "md" "nu" "toml" "yml" "yaml" "json" "sql" "sh" "txt"]

# Email templates are excluded from every template rule: they render in a mail
# client with no stylesheet, so a Tailwind class there is inert.
def template-files [] {
    let files = (glob server/templates/**/*.html --exclude ["**/email/**"] | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }
    $files
}

def admin-template-files [] {
    let files = (glob server/templates/admin/**/*.html | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/admin/"
        exit 1
    }
    $files
}

def tracked-text-files [] {
    let tracked = (^git ls-files | lines | where {|f| ($f | str trim) != "" })
    if ($tracked | is-empty) {
        print --stderr "`git ls-files` returned nothing; run this from the repository root"
        exit 1
    }
    $tracked
    | where {|f| not ($f | str starts-with "server/assets/vendor/") }
    | where {|f|
        let name = ($f | path basename)
        (($f | path parse | get extension) in $TEXT_EXTENSIONS) or ($name == "justfile") or ($name | str starts-with "Dockerfile")
    }
    # A staged deletion is still tracked; there is nothing left on disk to read.
    | where {|f| $f | path exists }
    | sort
}

def scan-lines [files: list<string>, pattern: string] {
    $files | each {|file|
        open --raw $file
        | decode utf-8
        | lines
        | enumerate
        | where {|row| $row.item =~ $pattern }
        | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
    } | flatten
}

# Every `class="..."` value on one line; a rule about which utilities travel
# together has to read the attribute, not the line, or a second element on the
# same line lends its color token to the first.
def class-attrs [line: string] {
    $line | parse --regex 'class="(?<value>[^"]*)"' | get value
}

def scan-class-attrs [files: list<string>, matches: closure] {
    $files | each {|file|
        open --raw $file
        | decode utf-8
        | lines
        | enumerate
        | each {|row|
            class-attrs $row.item
            | where {|value| do $matches $value }
            | each {|value| $"($file):($row.index + 1): class=\"($value)\"" }
        }
        | flatten
    } | flatten
}

def palette-in-templates [] {
    let pattern = $"\\b\(($PREFIXES)\)-\(($HUES)\)-[0-9]{2,3}\\b"
    template-files | each {|file|
        let lines = (open --raw $file | decode utf-8 | lines)
        $lines
        | enumerate
        | where {|row| $row.item =~ $pattern }
        | where {|row|
            let prev = (if $row.index == 0 { "" } else { $lines | get ($row.index - 1) })
            not (($row.item | str contains $PALETTE_ALLOW) or ($prev | str contains $PALETTE_ALLOW))
        }
        | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
    } | flatten
}

def bare-border [value: string] {
    ($value =~ $BARE_BORDER) and ($value !~ $BORDER_COLOR)
}

def bare-divide [value: string] {
    ($value =~ $BARE_DIVIDE) and ($value !~ $DIVIDE_COLOR)
}

def untokenized-borders [] {
    scan-class-attrs (template-files) {|value| (bare-border $value) or (bare-divide $value) }
}

def clipping-table-wrappers [] {
    scan-class-attrs (admin-template-files) {|value|
        ($value =~ '(^| )card( |$)') and ($value =~ '(^| )overflow-hidden( |$)')
    }
}

def raw-h1-sizes [] {
    # landing.html is a marketing hero deliberately outside the app scale
    # (LC-746 states the same exclusion).
    let files = (template-files | where {|f| ($f | path basename) != "landing.html" })
    scan-lines $files $H1_RAW_SIZE
}

def rules [] {
    [
        {
            id: "no-palette-literals-in-templates"
            pending: "LC-741"
            fix: $"use the semantic tokens from tailwind.config.js \(bg-surface-elevated, text-content, border-border, bg-danger, ...\); the one deliberately-dark call stage line must carry a comment with the ($PALETTE_ALLOW) marker naming why"
            check: {|| palette-in-templates }
        }
        {
            id: "no-fake-link-buttons"
            pending: "LC-743"
            fix: "a control that performs an action looks like a button: `btn btn-sm btn-danger-outline` / `btn-ghost` / `btn-primary`, never a color plus hover:underline, which is what the 24 real anchors use"
            check: {|| scan-lines (template-files) '<button[^>]*hover:underline' }
        }
        {
            id: "no-open-coded-danger-outline"
            pending: "LC-743"
            fix: $"use `btn btn-sm btn-danger-outline` \(main.css\); the inline copy is character-for-character what the class expands to"
            check: {|| scan-lines (template-files) $DANGER_OUTLINE }
        }
        {
            id: "no-untokenized-borders"
            pending: "LC-744"
            fix: "add a border color token (`border-border`, `divide-border`, ...) or use `.card` / `.input`; a bare border resolves to Tailwind's gray-200 default, which is brighter than the panel it borders in dark mode"
            check: {|| untokenized-borders }
        }
        {
            id: "no-clipping-table-wrappers"
            pending: null
            fix: "wrap an admin table in `<div class=\"card lc-table-wrap\">`; `overflow-hidden` clips the trailing columns instead of scrolling them, and the actions cell is what overflows at 375px (LC-737)"
            check: {|| clipping-table-wrappers }
        }
        {
            id: "no-raw-h1-sizes"
            pending: "LC-746"
            fix: "put the page title on `.lc-h1` (or `.lc-display` on a standalone centered page); a raw size utility is how 36 h1 elements ended up rendering at six sizes"
            check: {|| raw-h1-sizes }
        }
        {
            id: "no-em-dash"
            pending: null
            fix: "U+2014 (em dash) is banned repo-wide: use a hyphen, a colon, parentheses, or a period and a new sentence"
            check: {|| scan-lines (tracked-text-files) $EM_DASH }
        }
    ]
}

def main [] {
    mut failing = 0
    for rule in (rules) {
        let hits = (do $rule.check)
        if ($hits | is-empty) {
            if $rule.pending == null {
                print $"  ok      ($rule.id)"
            } else {
                print $"  ok      ($rule.id) - clear now: drop `pending: ($rule.pending)` in ci-build/check-ui-conventions.nu to enforce it"
            }
            continue
        }
        if $rule.pending != null {
            print $"  pending ($rule.id) - ($hits | length) hit\(s\), enforced when ($rule.pending) lands:"
            for h in $hits { print $"            ($h)" }
            continue
        }
        $failing = $failing + 1
        print --stderr $"  FAIL    ($rule.id) - ($rule.fix)"
        for h in $hits { print --stderr $"            ($h)" }
    }
    if $failing > 0 {
        print --stderr $"($failing) UI convention rule\(s\) failed."
        exit 1
    }
    print "UI conventions OK."
}
