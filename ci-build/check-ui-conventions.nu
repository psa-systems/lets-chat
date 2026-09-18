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
#     -> ci-build/check-locale-ellipsis.nu (LC-750); the `no-ellipsis-outside-
#        locales` rule below is that same spelling rule widened to the
#        templates and browser scripts, so the two together cover every
#        first-party surface except `sw.js`'s truncation marker (LC-891)
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

# LC-743: a `<button>` painted as a text link (`hover:underline` plus a color),
# which is the treatment the real anchors use. Matched on the whole opening tag
# for the same reason as CONTROL_TAG: a line-scoped pattern misses every button
# whose class attribute sits on a continuation line, and 10 of the 24 sites this
# rule closed were of exactly that shape.
const BUTTON_TAG = '(?<tag><button(?:\s[^>]*)?>)'

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

# LC-891: the ellipsis spelling rule widened past the locale catalogs. Same
# two spellings `check-locale-ellipsis.nu` rejects, checked against the
# templates and the browser scripts instead.
const ELLIPSIS_CHAR = "\u{2026}"
const ELLIPSIS_ENTITY = "&#8230;"

# LC-891: any `.lc-*` selector defined at the start of a line in main.css is a
# component class; matched on the bare prefix (not a word boundary) against
# the templates, the browser scripts and the Rust markup sources, so a name
# built by string concatenation (`'lc-set-avatar-fallback-' + id`) still
# counts as reachable.
const LC_CLASS_SELECTOR = '^\.(?<name>lc-[a-z0-9-]+)'

# LC-891: a `base.html` `window.__lcI18n` table entry, matched whole-line so a
# key commented out or reshaped some other way does not count.
const I18N_TABLE_ENTRY = '^\s*(?<key>[a-zA-Z_][a-zA-Z0-9_]*):\s*"\{\{\s*"[a-z0-9-]+"\|t\s*\}\}",?\s*$'

# LC-891: a call site for a table key: direct (`window.__lcS('key', ...)`), or
# through one of the file-local wrappers that just forward to it - `S(k, fb)`
# (call_reactions.js, huddle_popout.js), `s(key, fallback)` (huddle_ring.js),
# `str(key, fallback)` (huddle_control.js) - or as the second argument to
# `lcToast(kind, key, fallback, name)` (call.js). `\b` keeps `S(` / `s(` from
# matching as part of a longer identifier or `window.__lcToast(`, which takes
# no key at all.
const I18N_KEY_CALL = '\b(__lcS|S|s|str)\(\s*[\x27"](?<key>[a-zA-Z0-9_]+)[\x27"]'
const I18N_TOAST_KEY_CALL = '\blcToast\(\s*[\x27"][^\x27"]*[\x27"]\s*,\s*[\x27"](?<key>[a-zA-Z0-9_]+)[\x27"]'

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

# LC-937: the env-var-template parity gate. `LETS_CHAT_*`, `IP2LOCATION_*` and
# `LOGIN_APPROVAL_*` names are read across server/src, desktop/src and
# services/*/src; the two files an operator actually copies to deploy,
# `.env.standalone` and `.env.saas`, drifted out of sync with
# docs/configuration.md three times running (LiveKit and embeddings families,
# then transcription-agent tokens and STT quality knobs, then
# `LETS_CHAT_ENVIRONMENT`) because nothing enforced the three surfaces staying
# in lockstep. This rule extracts every name a read site names as a literal
# and fails unless the name is documented in docs/configuration.md and
# present in both templates, or carries a reason in the allowlist below.
#
# Each allowlist entry is a name this rule would otherwise flag, paired with
# why it is deliberately exempt. The desktop-only names have no server .env
# surface at all; the SaaS/dev-only gaps predate LC-937 and are tracked by
# LC-952 rather than fixed here, since closing them touches auth/mail
# behavior questions bigger than a template edit.
const ENV_VAR_ALLOWLIST = {
    LETS_CHAT_SERVER_URL: "desktop-only (docs/configuration.md Desktop app section); no server .env surface exists for it (LC-952)"
    LETS_CHAT_UPDATE_REGISTRY_URL: "desktop-only, see LETS_CHAT_SERVER_URL above (LC-952)"
    LETS_CHAT_UPDATE_REPOSITORY: "desktop-only, see LETS_CHAT_SERVER_URL above (LC-952)"
    LETS_CHAT_UPDATE_TAG: "desktop-only, see LETS_CHAT_SERVER_URL above (LC-952)"
    LETS_CHAT_UPDATE_TOKEN: "desktop-only, see LETS_CHAT_SERVER_URL above (LC-952)"
    LETS_CHAT_UPDATE_URL_ALLOW_PRIVATE: "desktop-only, see LETS_CHAT_SERVER_URL above (LC-952)"
    LETS_CHAT_UPDATE_BASE_URL: "dead: no Rust source has read it since LC-733; the name survives only as the literal desktop/src/update.rs asserts is absent from the Dockerfiles (LC-594)"
    LETS_CHAT_BASE_URL: ".env.saas omits it along with the rest of the mail/SSO block; pre-existing gap tracked by LC-952, not one of LC-937's eight families"
    LETS_CHAT_SECRET_KEY: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_BUNYIP_SSO_ISSUER: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_BUNYIP_SSO_CLIENT_ID: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_BUNYIP_SSO_CLIENT_SECRET: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_BUNYIP_SSO_REDIRECT_URI: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_BUNYIP_SSO_INSECURE_TLS: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_SMTP_HOST: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_SMTP_PORT: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_SMTP_TLS: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_SMTP_FROM: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_SMTP_USERNAME: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_SMTP_PASSWORD: "same .env.saas gap as LETS_CHAT_BASE_URL above (LC-952)"
    LETS_CHAT_DEV_NO_SSO: "documented but present in neither template; pre-existing gap tracked by LC-952, not one of LC-937's eight families"
    LETS_CHAT_PUSH_CONTACT: "documented but present in neither template; pre-existing gap tracked by LC-952, not one of LC-937's eight families"
}

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

# The browser scripts that build markup with string concatenation. Tailwind
# scans them (`content` in tailwind.config.js lists `./assets/**/*.js`), so a
# class spelled out there renders exactly like one in a template, and a rule
# that swept only the templates would leave that half open (LC-743).
def browser-asset-files [] {
    let files = (glob server/assets/**/*.js --exclude ["**/vendor/**"] | sort)
    if ($files | is-empty) {
        print --stderr "No browser scripts found under server/assets/"
        exit 1
    }
    $files
}

# The Rust sources that build markup by hand (the autolinker and mention chips
# in views/room.rs, the deleted-message and scheduled-status fragments in
# routes/). Tailwind scans these too (`content` lists `./src/**/*.rs`), so a
# raw palette utility spelled out in a `push_str` or `format!` is live in the
# built stylesheet, and this was the one scanned surface no palette rule
# covered (LC-787).
def rust-markup-files [] {
    let files = (glob server/src/**/*.rs | sort)
    if ($files | is-empty) {
        print --stderr "No Rust sources found under server/src/"
        exit 1
    }
    $files
}

def markup-files [] {
    (template-files) | append (browser-asset-files)
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

# One pattern over whichever file set is passed in; the templates and the Rust
# markup sources share it and the `lc-allow-palette` marker.
def palette-literals [files: list<string>] {
    let pattern = $"\\b\(($PREFIXES)\)-\(($HUES)\)-[0-9]{2,3}\\b"
    $files | each {|file|
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

# LC-891: the general form of `dead-classes` above - any `.lc-*` component
# class in main.css, not just the six names LC-744 already closed - swept over
# the same three surfaces Tailwind (and the Rust markup) actually scans.
def dead-lc-css-selectors [] {
    let file = "server/assets/main.css"
    let selectors = (
        open --raw $file | decode utf-8 | lines | enumerate
        | each {|row|
            let m = ($row.item | parse --regex $LC_CLASS_SELECTOR)
            if ($m | is-empty) { null } else { {line: ($row.index + 1), name: $m.0.name, text: ($row.item | str trim)} }
        }
        | where {|x| $x != null }
    )
    let haystacks = (
        (template-files | append (browser-asset-files) | append (rust-markup-files))
        | each {|f| open --raw $f | decode utf-8 }
    )
    # The bare prefix: a BEM modifier (`lc-status--ok`, `lc-tx-badge--dm`) is
    # usually built by template interpolation (`lc-status--{% if ok %}ok{%
    # else %}err{% endif %}`), so the literal full name never appears in one
    # piece; checking the part before the modifier's `--` is what keeps that
    # reachable while still catching a base name nothing refers to at all.
    let bases = ($selectors | each {|s| ($s.name | split row "--" | first) } | uniq)
    let dead_bases = ($bases | where {|b| not ($haystacks | any {|t| $t | str contains $b }) })
    $selectors
    | where {|s| ($s.name | split row "--" | first) in $dead_bases }
    | each {|s| $"($file):($s.line): ($s.text)" }
}

# LC-891: the same ellipsis spelling rule `check-locale-ellipsis.nu` enforces
# on the catalogs, widened to the templates and browser scripts. `sw.js` is
# excluded: its one U+2026 is a truncation marker on a notification preview,
# not prose that should match the catalog spelling.
def ellipsis-outside-locales [] {
    let files = (
        (template-files | append (browser-asset-files))
        | where {|f| ($f | path basename) != "sw.js" }
    )
    (scan-lines $files $ELLIPSIS_CHAR) | append (scan-lines $files $ELLIPSIS_ENTITY) | sort
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

# LC-743: `<button>` opening tags carrying a link's `hover:underline`. Same
# whole-document parse as open-coded-primary-fill, and over the browser scripts
# as well, so neither a wrapped tag nor a JS-built one slips the rule.
def fake-link-buttons [] {
    markup-files | each {|file|
        let text = (open --raw $file | decode utf-8)
        let lines = ($text | lines)
        $text
        | parse --regex $BUTTON_TAG
        | get tag
        | each {|tag|
            class-attrs $tag
            | where {|value| $value | str contains "hover:underline" }
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

# LC-881: a `:focus-visible` rule that turns off the default outline has to
# replace it with the shared ring, or a keyboard user cannot tell "focused"
# from "hover" (the resize handle) or from "nothing" (a roving-tabindex menu).
# Matched on the selector-to-brace block so a `:focus-visible` selector with an
# unrelated declaration block (e.g. `:focus-visible::before`, which only ever
# recolors the pseudo-element) does not trip the rule.
def focus-visible-rings [] {
    let file = "server/assets/main.css"
    let text = (open --raw $file | decode utf-8)
    $text
    | parse --regex '(?s)(?<selector>[^{}]+)\{(?<body>[^{}]*)\}'
    | where {|rule| ($rule.selector | str contains ":focus-visible") and ($rule.body =~ 'outline:\s*none') and ($rule.body !~ 'box-shadow') }
    | each {|rule|
        let selector = ($rule.selector | str trim | str replace --all "\n" ' ')
        let body = ($rule.body | str trim | str replace --all "\n" ' ')
        $"($file): selector `($selector)`, body `($body)`"
    }
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

# LC-875: a control that flips its `.lc-cbtn-label` text but not its
# `aria-label` leaves the accessible name pointing at the old label (aria-label
# overrides text content), so the two writes have to land in the same
# function. Brace-matched rather than line-scoped: the enclosing function can
# run many lines past the `.lc-cbtn-label` reference itself.
def line-of [text: string, offset: int] {
    ($text | str substring 0..$offset | str replace --all --regex '[^\n]' '' | str length) + 1
}

def enclosing-function-text [lines: list<string>, from_index: int] {
    mut start = $from_index
    mut found = -1
    while $start >= 0 {
        if ($lines | get $start) =~ '\bfunction\b' {
            $found = $start
            break
        }
        $start = $start - 1
    }
    if $found < 0 { return "" }
    mut depth = 0
    mut started = false
    mut end = $found
    mut i = $found
    let n = ($lines | length)
    while $i < $n {
        let line = ($lines | get $i)
        for c in ($line | split chars) {
            if $c == "{" { $depth = $depth + 1; $started = true }
            if $c == "}" { $depth = $depth - 1 }
        }
        if $started and $depth <= 0 {
            $end = $i
            break
        }
        $i = $i + 1
    }
    $lines | slice $found..$end | str join "\n"
}

# The variable a line binds to the `.lc-cbtn-label` span, whether by resolving
# an existing one (`var l = btn.querySelector('.lc-cbtn-label')`) or by
# stamping the class onto a freshly created one (`bl.className =
# 'lc-cbtn-label'`); "" when the line only mentions the class in passing (a
# comment, a CSS selector elsewhere in the file).
def cbtn-label-var [line: string] {
    let via_query = ($line | parse --regex "(?:var|let|const) (?<v>\\w+) = .*querySelector\\('\\.lc-cbtn-label'\\)")
    if not ($via_query | is-empty) { return ($via_query | get v.0) }
    let via_class = ($line | parse --regex "(?<v>\\w+)\\.className = .lc-cbtn-label.")
    if not ($via_class | is-empty) { return ($via_class | get v.0) }
    ""
}

def cbtn-label-missing-aria [] {
    browser-asset-files | each {|file|
        let text = (open --raw $file | decode utf-8)
        let lines = ($text | lines)
        $lines
        | enumerate
        | where {|row| $row.item =~ 'lc-cbtn-label' }
        | each {|row|
            let v = (cbtn-label-var $row.item)
            if $v == "" {
                []
            } else {
                let body = (enclosing-function-text $lines $row.index)
                let write_pattern = $"($v)\\.textContent\\s*="
                if ($body != "") and ($body =~ $write_pattern) and ($body !~ 'aria-label') {
                    [$"($file):($row.index + 1): ($row.item | str trim)"]
                } else {
                    []
                }
            }
        }
        | flatten
    } | flatten
}

# LC-937: every source file that may read a `LETS_CHAT_*` / `IP2LOCATION_*` /
# `LOGIN_APPROVAL_*` name: the server, the desktop wrapper, and the LiveKit
# transcription-agent sidecar (a separate TypeScript service under services/).
def env-var-read-sites [] {
    (glob server/src/**/*.rs)
    | append (glob desktop/src/**/*.rs)
    | append (glob services/*/src/**/*.ts)
    | sort
}

# Every name literal a read site names, whether quoted (`env::var("X")`,
# `required(env, 'X')`), bare (`env.X` in the TS agent), or captured in a
# `const FLAG_ENV: &str = "X"` indirection (retention/sweep.rs) - one pattern
# over the raw text covers all three shapes.
def env-var-literals [] {
    let pattern = '(?<name>\b(?:LETS_CHAT|IP2LOCATION|LOGIN_APPROVAL)(?:_[A-Z0-9]+)+\b)'
    (env-var-read-sites) | each {|file|
        open --raw $file | decode utf-8 | parse --regex $pattern | get name
    } | flatten | uniq | sort
}

# docs/configuration.md spells a variable family once in full and abbreviates
# the rest of the row to their differing suffix (`` `LETS_CHAT_LIVEKIT_URL` /
# `_API_KEY` / `_API_SECRET` ``). A name counts as documented if it appears in
# full, or if some split of it into `head` + `_` + `tail` has both `` `_tail` ``
# and `head` somewhere in the doc (as the prefix of the row's full name).
def env-var-documented [name: string, doc: string] {
    if ($doc | str contains $"`($name)`") {
        return true
    }
    let parts = ($name | split row "_")
    let n = ($parts | length)
    mut i = 1
    while $i < $n {
        let head = ($parts | first $i | str join "_")
        let tail = ($parts | skip $i | str join "_")
        if ($doc | str contains $"`_($tail)`") and ($doc | str contains $head) {
            return true
        }
        $i = $i + 1
    }
    false
}

# Unlike the doc, both templates always spell every name in full.
def env-var-in-template [name: string, template: string] {
    $template | str contains $name
}

def undocumented-env-vars [] {
    let doc = (open --raw "docs/configuration.md" | decode utf-8)
    let standalone = (open --raw ".env.standalone" | decode utf-8)
    let saas = (open --raw ".env.saas" | decode utf-8)
    (env-var-literals)
    | where {|name| $name not-in ($ENV_VAR_ALLOWLIST | columns) }
    | each {|name|
        mut missing = []
        if not (env-var-documented $name $doc) { $missing = ($missing | append "docs/configuration.md") }
        if not (env-var-in-template $name $standalone) { $missing = ($missing | append ".env.standalone") }
        if not (env-var-in-template $name $saas) { $missing = ($missing | append ".env.saas") }
        if ($missing | is-empty) { null } else { $"($name): missing from ($missing | str join ', ')" }
    }
    | where {|x| $x != null }
}

# LC-891: the `window.__lcI18n` table entries in base.html.
def i18n-table-entries [] {
    let file = "server/templates/base.html"
    open --raw $file | decode utf-8 | lines | enumerate
    | each {|row|
        let m = ($row.item | parse --regex $I18N_TABLE_ENTRY)
        if ($m | is-empty) { null } else { {file: $file, line: ($row.index + 1), key: $m.0.key, text: ($row.item | str trim)} }
    }
    | where {|x| $x != null }
}

# LC-891: every table key a browser script or an inline template `<script>`
# actually calls (direct or through one of the file-local wrappers); several
# partials (composer.html, picker.html) call `window.__lcS` from a `<script>`
# block of their own rather than from `server/assets`, so the sweep has to
# cover the templates too, the same set `markup-files` already names for the
# palette and fake-link-button rules.
def used-i18n-keys [] {
    markup-files | each {|file|
        open --raw $file | decode utf-8 | lines | enumerate
        | each {|row|
            let direct = ($row.item | parse --regex $I18N_KEY_CALL | each {|m| {file: $file, line: ($row.index + 1), key: $m.key} })
            let toast = ($row.item | parse --regex $I18N_TOAST_KEY_CALL | each {|m| {file: $file, line: ($row.index + 1), key: $m.key} })
            $direct | append $toast
        }
        | flatten
    } | flatten
}

# LC-891: the key-pairing rule from both directions - a table entry nothing
# calls, and a call site whose key has no table entry (a broken lookup, not
# just a dead one).
def i18n-key-pairing [] {
    let table = (i18n-table-entries)
    let used = (used-i18n-keys)
    let table_keys = ($table | get key | uniq)
    # A table entry is dead only if its key never shows up as a quoted string
    # anywhere in the browser scripts. Broader than `used-i18n-keys`, which
    # only understands the known call shapes: some keys reach `__lcS` through
    # an indirection table (`voice.js`'s `QUALITY_LABEL` map, keyed by
    # connection state, values are table keys) that no fixed set of call
    # patterns fully covers, and a false "dead" here breaks the build, while a
    # false "live" just leaves a genuinely dead entry for the next pass.
    let asset_text = (markup-files | each {|f| open --raw $f | decode utf-8 } | str join "\n")
    let dead_entries = (
        $table
        | where {|t| not (($asset_text | str contains $"'($t.key)'") or ($asset_text | str contains $'"($t.key)"')) }
        | each {|t| $"($t.file):($t.line): ($t.text)" }
    )
    let orphan_calls = ($used | where {|u| $u.key not-in $table_keys } | each {|u| $"($u.file):($u.line): __lcS\(\"($u.key)\"\) has no base.html table entry" })
    $dead_entries | append $orphan_calls
}

def rules [] {
    [
        {
            id: "no-palette-literals-in-templates"
            pending: null
            fix: $"use the semantic tokens from tailwind.config.js \(bg-surface-elevated, text-content, border-border, bg-danger, ...\); the one deliberately-dark call stage line must carry a comment with the ($PALETTE_ALLOW) marker naming why \(LC-741, and LC-735 for the same rule over the browser assets\)"
            check: {|| palette-literals (template-files) }
        }
        {
            id: "no-palette-literals-in-rust-markup"
            pending: null
            fix: $"the Rust sources that build markup by hand are in Tailwind's `content` too, so the same tokens apply \(text-accent for a link, bg-accent-surface text-accent-surface-content for a chip, text-success / text-content-subtle for status copy\); an exception carries a comment with the ($PALETTE_ALLOW) marker naming why \(LC-787\)"
            check: {|| palette-literals (rust-markup-files) }
        }
        {
            id: "no-fake-link-buttons"
            pending: null
            fix: "a control that performs an action looks like a button: `btn btn-sm btn-danger-outline` (destructive), `btn-ghost` (dismiss / secondary) or `btn-primary` (an inline save), never a color plus hover:underline, which is the treatment the real anchors use (LC-743)"
            check: {|| fake-link-buttons }
        }
        {
            id: "no-open-coded-danger-outline"
            pending: null
            fix: $"use `btn btn-sm btn-danger-outline` \(main.css\); the inline copy is character-for-character what the class expands to \(LC-743\)"
            check: {|| scan-lines (markup-files) $DANGER_OUTLINE }
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
            id: "no-dead-lc-css-selectors"
            pending: null
            fix: "a `.lc-*` class defined in main.css with no literal hit in the templates, browser scripts or Rust markup sources has no caller left; delete the selector, or its consumer if the deletion was the miss (LC-891)"
            check: {|| dead-lc-css-selectors }
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
            id: "focus-visible-rings"
            pending: null
            fix: "a `:focus-visible` rule that sets `outline: none` must also set `box-shadow` (the shared `0 0 0 2px var(--ring)` ring); otherwise a keyboard user cannot tell focus from hover or from nothing at all (LC-881)"
            check: {|| focus-visible-rings }
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
            id: "no-ellipsis-outside-locales"
            pending: null
            fix: "write three periods, not U+2026 or `&#8230;`; the catalogs spell it that way and `check-locale-ellipsis.nu` guards them, this rule is the same spelling over the templates and browser scripts. `sw.js` is exempt: its one U+2026 is a truncation marker, not prose (LC-891)"
            check: {|| ellipsis-outside-locales }
        }
        {
            id: "i18n-keys-are-paired"
            pending: null
            fix: "every `window.__lcI18n` entry in base.html needs a caller in the templates or server/assets, and every `__lcS`-family call site needs a matching base.html entry; delete whichever side of the pair is now the leftover (LC-891)"
            check: {|| i18n-key-pairing }
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
            fix: "U+2014 (em dash) is banned repo-wide: use a hyphen, a colon, parentheses, or a period and a new sentence (internal/CLAUDE.md style rules, folded into this job by LC-749)"
            check: {|| scan-lines (tracked-text-files) $EM_DASH }
        }
        {
            id: "cbtn-label-text-keeps-aria-label"
            pending: null
            fix: "a function that writes `.lc-cbtn-label` text must also write `aria-label` (and `data-lc-tip`) in the same function, or the tooltip and the accessible name go stale the moment the visible label flips; use the shared `setLabel` on `window.LetsChatRtc` (rtc_common.js) instead of a local copy (LC-875)"
            check: {|| cbtn-label-missing-aria }
        }
        {
            id: "env-var-template-parity"
            pending: null
            fix: "add a commented entry mirroring docs/configuration.md's wording to whichever of .env.standalone / .env.saas is missing it, or add it to docs/configuration.md if the code changed first; a name deliberately absent from one of the three surfaces goes on the ENV_VAR_ALLOWLIST above with a one-line reason (LC-937)"
            check: {|| undocumented-env-vars }
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
