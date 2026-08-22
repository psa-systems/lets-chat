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
# Files are read with `open --raw`, never `grep -r`, so no rule depends on
# grep's binary heuristic: one raw control byte would make it skip a whole file
# silently. The `no-raw-nul-bytes` rule below keeps that class closed at source
# (LC-757).

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

# LC-755: the third shape of the same defect - the primary accent fill spelled
# out inline instead of `.btn-primary` (tailwind.css). Matched on the whole
# opening tag, because the class attribute usually sits on a continuation line.
const CONTROL_TAG = '(?<tag><(?:button|a|label)(?:\s[^>]*)?>)'

# Template syntax separates utilities the way whitespace does: an active-state
# branch reads `{% if x %}bg-accent text-accent-content{% endif %}`.
const TEMPLATE_TAG = '\{[%{][^{}]*[%}]\}'

# A border utility used to paint Tailwind's `colors.gray[200]` default - a fixed
# light grey in all four modes and every palette. LC-744 set `borderColor.DEFAULT`
# to `var(--border)`, so that is no longer the failure mode; the rule stays
# because a template that names its border color cannot be silently repainted by
# a later config change, and because `.card` / `.input` already carry the right
# one. A numbered palette color counts as named here and is LC-741's rule, not
# this one.
const BARE_BORDER = '(^| )(border|border-[0-9]+|border-[trblxy](-[0-9]+)?)( |$)'
const BARE_DIVIDE = '(^| )divide-[xy](-[0-9]+)?( |$)'
const COLOR_NAMES = "border|accent|danger|success|warning|star|transparent|current|inherit|black|white"

# Raw size utilities on an `<h1>`: the page-title size comes from `.lc-h1` or
# `.lc-display` (main.css), not from a per-page pick out of six.
const H1_RAW_SIZE = '<h1[^>]*text-(xs|sm|base|lg|xl|2xl|3xl|4xl|5xl)'

# LC-746: the other half of the same rule. An `<h1>` with no size utility at all
# still renders at the 1rem body size, because Tailwind's preflight resets it to
# `font-size: inherit`; the class has to be on the element, not merely absent.
const H1_TAG = '<h1[\s>]'
const H1_ON_SCALE = '<h1[^>]*class="[^"]*\b(lc-h1|lc-display)\b'

# LC-746: a rendered timestamp is a `<time>` with a machine-readable `datetime`
# (`{{ x|iso }}`), so assistive technology gets the instant and the LC-314
# relative-time upgrade can key on it. Same pattern as the issue's acceptance
# grep; a line carrying `<time` has already been converted.
const BARE_TIMESTAMP = '\{\{ *[a-z_.]*_at[a-z_.()]* *\}\}'

# LC-744: the shared components that shipped and were then not adopted. These
# three rules guard what that issue deleted or converted, so none of it reopens
# the way it did between LC-557 / LC-561 / LC-562 and the 2026-08-11 audit.
#
# The deleted names: a second empty-state component that duplicated `.lc-empty`,
# a sixth page width at the same 48rem as `.lc-page-medium`, and the three
# explicit table sub-classes whose only user was the dev gallery. The sweep reads
# the templates and both stylesheets, so a name is gone only when neither the
# markup nor the CSS mentions it - prose in this repo says "head / row / cell
# sub-classes" rather than spelling them.
const DEAD_CLASSES = ['lc-tx-empty' 'lc-admin-narrow' 'lc-table-head' 'lc-table-cell' 'lc-table-row']

# A page's content column takes its width from `.lc-page-narrow/medium/wide`, not
# from a per-page `max-w-*`. landing.html is a marketing page with a deliberately
# wider grid and is out of scope (LC-744 states the same exclusion).
const CENTERED_WIDTH = 'mx-auto[^"]*max-w-|max-w-[a-z0-9]+[^"]*mx-auto'

# A bordered soft-surface box IS `.alert`. A pill (`rounded-full`) and an edge
# rule (`border-b` with no all-sides `border`) are different components and do
# not match: the pair that identifies a callout is an all-sides border plus the
# matching `-surface` / `-border` token pair.
const CALLOUT_TONES = ["success" "warning" "danger"]
const ALL_SIDES_BORDER = '(^| )border( |$)'

const EM_DASH = "\u{2014}"

# LC-748: the service worker's offline fallback. It is a standalone document
# outside the template layer, so nothing else here covers it: it must stay
# mode-aware (no light-only `color-scheme`) and must call the product by its
# name. A comment may keep the repo name.
const OFFLINE_ASSETS = ["server/assets/offline.html" "server/assets/sw.js"]
const REPO_NAME = "lets-chat"
const LIGHT_ONLY_SCHEME = 'color-scheme:\s*light\s*;'
const COMMENT_LINE = '^\s*(//|/\*|\*|<!--)'

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

# `border-border`, `border-danger-border`, `border-slate-700`, `border-black`.
def color-pattern [prefix: string] {
    $"($prefix)-\(\(($COLOR_NAMES)\)|\(($HUES)\)-[0-9]{2,3}\)"
}

def bare-border [value: string] {
    ($value =~ $BARE_BORDER) and ($value !~ (color-pattern "border"))
}

def bare-divide [value: string] {
    ($value =~ $BARE_DIVIDE) and ($value !~ (color-pattern "divide"))
}

def untokenized-borders [] {
    scan-class-attrs (template-files) {|value| (bare-border $value) or (bare-divide $value) }
}

def clipping-table-wrappers [] {
    scan-class-attrs (admin-template-files) {|value|
        ($value =~ '(^| )card( |$)') and ($value =~ '(^| )overflow-hidden( |$)')
    }
}

def app-h1-files [] {
    # landing.html is a marketing hero deliberately outside the app scale
    # (LC-746 states the same exclusion).
    template-files | where {|f| ($f | path basename) != "landing.html" }
}

def raw-h1-sizes [] {
    scan-lines (app-h1-files) $H1_RAW_SIZE
}

def h1-off-the-scale [] {
    (app-h1-files) | each {|file|
        open --raw $file
        | decode utf-8
        | lines
        | enumerate
        | where {|row| ($row.item =~ $H1_TAG) and ($row.item !~ $H1_ON_SCALE) }
        | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
    } | flatten
}

# One `<h1>` per template: the page header bar already renders the page title, so
# a second one in a body branch gives the page two competing top-level headings.
def extra-h1s [] {
    template-files | each {|file|
        let hits = (
            open --raw $file
            | decode utf-8
            | lines
            | enumerate
            | where {|row| $row.item =~ $H1_TAG }
        )
        let total = ($hits | reduce --fold 0 {|row, acc| $acc + (($row.item | split row --regex $H1_TAG | length) - 1) })
        if $total > 1 {
            $hits | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } else {
            []
        }
    } | flatten
}

# The dead class names, swept over the templates AND the two stylesheets: a
# removal only holds if neither the markup nor the CSS brings the name back.
def dead-classes [] {
    let files = (template-files | append ["server/assets/main.css" "server/assets/tailwind.css"])
    $DEAD_CLASSES | each {|name| scan-lines $files $name } | flatten | sort
}

def per-page-widths [] {
    let files = (template-files | where {|f| ($f | path basename) != "landing.html" })
    scan-lines $files $CENTERED_WIDTH
}

def hand-rolled-callouts [] {
    scan-class-attrs (template-files) {|value|
        if ($value =~ '(^| )alert( |$)') or ($value =~ 'rounded-full') { return false }
        if ($value !~ $ALL_SIDES_BORDER) { return false }
        # Unprefixed only: a `hover:` fill is an outline button (LC-743), not a
        # callout, and it is that rule's to own.
        ($CALLOUT_TONES | any {|tone|
            ($value =~ $"\(^| \)bg-($tone)-surface\( |$\)") and ($value =~ $"\(^| \)border-($tone)-border\( |$\)")
        })
    }
}

# LC-755: only an unprefixed pair counts. `hover:bg-accent`, `focus:bg-accent`
# and `aria-pressed:bg-accent` paint a state, not the resting primary fill, and
# `bg-accent-surface` is a different token, so the check is on whole utilities.
def class-tokens [value: string] {
    $value
    | str replace --all --regex $TEMPLATE_TAG " "
    | split row --regex '\s+'
    | where {|t| $t != "" }
}

def open-coded-primary-fill [] {
    template-files | each {|file|
        # `parse` reads the whole document only from a bound string; piped
        # straight out of `open` it matches line by line and misses every tag
        # whose class attribute sits on a continuation line.
        let text = (open --raw $file | decode utf-8)
        let lines = ($text | lines)
        $text
        | parse --regex $CONTROL_TAG
        | get tag
        | each {|tag|
            class-attrs $tag
            | where {|value|
                let tokens = (class-tokens $value)
                ("bg-accent" in $tokens) and ("text-accent-content" in $tokens) and ("btn-primary" not-in $tokens)
            }
            | each {|value|
                let at = ($lines | enumerate | where {|row| $row.item | str contains $value } | get index)
                let line = (if ($at | is-empty) { "?" } else { ($at | first) + 1 })
                $"($file):($line): class=\"($value)\""
            }
        }
        | flatten
    } | flatten
}

def bare-timestamps [] {
    scan-lines (template-files) $BARE_TIMESTAMP
    | where {|hit| not ($hit | str contains "<time") }
}

# LC-757: a raw NUL makes grep, git grep and ripgrep classify the file as binary
# and skip it, so a grep-based gate over a directory reads nothing and passes.
# Read as bytes: `open --raw` hands back a string for valid UTF-8, and a NUL is
# valid UTF-8.
def raw-nul-bytes [] {
    tracked-text-files | each {|file|
        let offset = (open --raw $file | into binary | bytes index-of 0x[00])
        if $offset >= 0 {
            [$"($file): raw NUL byte at offset ($offset)"]
        } else {
            []
        }
    } | flatten
}

def offline-brand-name [] {
    $OFFLINE_ASSETS | each {|file|
        open --raw $file
        | decode utf-8
        | lines
        | enumerate
        | where {|row| ($row.item | str contains $REPO_NAME) and ($row.item !~ $COMMENT_LINE) }
        | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
    } | flatten
}

def rules [] {
    [
        {
            id: "no-palette-literals-in-templates"
            pending: "LC-741"
            fix: $"use the semantic tokens from tailwind.config.js \(bg-surface-elevated, text-content, border-border, bg-danger, ...\); the one deliberately-dark call stage line must carry a comment with the ($PALETTE_ALLOW) marker naming why \(LC-741, and LC-735 for the same rule over the browser assets\)"
            check: {|| palette-in-templates }
        }
        {
            id: "no-fake-link-buttons"
            pending: "LC-743"
            fix: "a control that performs an action looks like a button: `btn btn-sm btn-danger-outline` / `btn-ghost` / `btn-primary`, never a color plus hover:underline, which is what the 24 real anchors use (LC-743)"
            check: {|| scan-lines (template-files) '<button[^>]*hover:underline' }
        }
        {
            id: "no-open-coded-danger-outline"
            pending: "LC-743"
            fix: $"use `btn btn-sm btn-danger-outline` \(main.css\); the inline copy is character-for-character what the class expands to \(LC-743\)"
            check: {|| scan-lines (template-files) $DANGER_OUTLINE }
        }
        {
            id: "no-open-coded-primary-fill"
            pending: null
            fix: "a primary action gets `btn btn-primary` (plus `btn-sm` when it is the compact one); the inline `bg-accent` + `text-accent-content` pair is what `.btn-primary` expands to, and 28 sites carried their own copy of it until LC-755. A `hover:` / `focus:` / `aria-pressed:` accent is a state, not the resting fill, and does not match"
            check: {|| open-coded-primary-fill }
        }
        {
            id: "no-untokenized-borders"
            pending: null
            fix: "add a border color token (`border-border`, `divide-border`, ...) or use `.card` / `.input`; `borderColor.DEFAULT` in tailwind.config.js is the backstop, and naming the color at the call site is what keeps a config change from repainting the element silently (LC-744)"
            check: {|| untokenized-borders }
        }
        {
            id: "no-superseded-component-classes"
            pending: null
            fix: "these class names were deleted by LC-744 because a shared component already covered them: the transcript-list empty state is partials/empty_state.html, the admin form width is `lc-page-medium lc-page-stack`, and a table is bare `<th>` / `<td>` inside `.lc-table` (main.css). Re-adding one restores the duplicate this issue removed"
            check: {|| dead-classes }
        }
        {
            id: "page-width-from-a-helper"
            pending: null
            fix: "center a content column with `lc-page-narrow` (login / error / short forms), `lc-page-medium` (settings and content) or `lc-page-wide` (admin tables), not a per-page `mx-auto max-w-*`; landing.html is the one marketing page excluded (LC-744)"
            check: {|| per-page-widths }
        }
        {
            id: "no-hand-rolled-callouts"
            pending: null
            fix: "an all-sides bordered box on a `-surface` / `-border` token pair is `.alert` plus `alert-success` / `alert-warning` / `alert-danger` (tailwind.css); the hand-rolled copies differed from it by 2px and 4px of padding and by one radius step (LC-744)"
            check: {|| hand-rolled-callouts }
        }
        {
            id: "no-clipping-table-wrappers"
            pending: null
            fix: "wrap an admin table in `<div class=\"card lc-table-wrap\">`; `overflow-hidden` clips the trailing columns instead of scrolling them, and the actions cell is what overflows at 375px (LC-737)"
            check: {|| clipping-table-wrappers }
        }
        {
            id: "no-raw-h1-sizes"
            pending: null
            fix: "put the page title on `.lc-h1` (or `.lc-display` on a standalone centered page); a raw size utility is how 36 h1 elements ended up rendering at six sizes (LC-746)"
            check: {|| raw-h1-sizes }
        }
        {
            id: "h1-on-the-scale"
            pending: null
            fix: "give the `<h1>` a `class` carrying `lc-h1` (a page with a header bar) or `lc-display` (a standalone centered page: error, not-found, maintenance, the two auth pages); with no class it inherits the 1rem body size (LC-746)"
            check: {|| h1-off-the-scale }
        }
        {
            id: "one-h1-per-template"
            pending: null
            fix: "keep a single top-level heading per page; demote the extra one to `<h2 class=\"lc-display\">`, which is what home/welcome.html's welcome hero does (LC-746)"
            check: {|| extra-h1s }
        }
        {
            id: "timestamps-are-time-elements"
            pending: null
            fix: "render a timestamp as `<time datetime=\"{{ x|iso }}\" title=\"{{ x }}\">{{ x }}</time>`; a bare string gives assistive technology no machine-readable instant and cannot take the LC-314 relative-time upgrade. Add `data-lc-ts` on the seven feed-like surfaces (activity, inbox, pins, saved, related, search results, transcripts list) and leave it off the admin audit tables, where the exact stamp is the point (LC-746)"
            check: {|| bare-timestamps }
        }
        {
            id: "offline-page-brand-name"
            pending: null
            fix: $"the offline page and the push fallback title say \"Let's Chat\", the name every other user-visible surface uses; \"($REPO_NAME)\" is the repo, and belongs only in a comment \(LC-748\)"
            check: {|| offline-brand-name }
        }
        {
            id: "offline-page-follows-mode"
            pending: null
            fix: "server/assets/offline.html resolves `lc-mode` and paints from its own light/dark custom properties; a light-only `color-scheme` flashes a white page at a dark-mode user at the worst moment (LC-748)"
            check: {|| scan-lines ["server/assets/offline.html"] $LIGHT_ONLY_SCHEME }
        }
        {
            id: "no-raw-nul-bytes"
            pending: null
            fix: "write the byte as a language escape (`\\u0000` in a JS string literal), never as a raw control character: a literal NUL makes every grep-family tool treat the whole file as binary and skip it, so a grep-based gate over the directory reads nothing and still passes (LC-757)"
            check: {|| raw-nul-bytes }
        }
        {
            id: "no-em-dash"
            pending: null
            fix: "U+2014 (em dash) is banned repo-wide: use a hyphen, a colon, parentheses, or a period and a new sentence (CLAUDE.md style rules, folded into this job by LC-749)"
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
