#!/usr/bin/env nu

# Guard the catalog strings interpolated into an inline confirm() (LC-753).
#
# Askama escapes every `{{ }}` through askama_escape, which maps `'` to
# `&#x27;`. The HTML parser decodes that back to a bare `'` before the handler
# attribute is compiled as JavaScript, so an apostrophe in the string ends the
# JS literal early and the handler never compiles. The dialog then never
# appears and the destructive action runs on the first click, silently:
# `admin-webhooks-rotate-confirm` rotated a live webhook secret that way.
#
# The rule: a catalog value interpolated into an inline `confirm('...')`
# carries neither `'` (ends the literal) nor `\` (escapes the next character),
# in any locale. The guard reads both inputs the bug needs (the template site
# and the catalog value), so a bad string added on either side fails here. A
# `"` is safe: the HTML parser decodes `&quot;` after the attribute is
# tokenized, and a double quote inside a single-quoted JS literal is just text.

# `onclick="return confirm('...')"` / `onsubmit=...`; capture the JS literal.
const CALL = "\\bon(?:click|submit)=\"[^\"]*?confirm\\(\u{27}(?<arg>[^\u{27}]*)\u{27}\\)"

# The unterminated shape: a confirm('...) whose literal does not close on the
# line. `confirm(this.getAttribute('data-lc-confirm'))` is not this shape and is
# apostrophe-safe anyway: attribute text never reaches the JS parser.
const CALL_OPEN = "\\bon(?:click|submit)=\"[^\"]*?confirm\\(\u{27}"

# `{{ "some-key"|t }}` inside that literal.
const KEY = "\\{\\{\\s*\"(?<key>[a-zA-Z0-9_-]+)\"\\s*\\|\\s*t\\s*\\}\\}"

# `key = value` in a Fluent catalog; comments and blank lines do not match.
const ENTRY = "^(?<key>[a-zA-Z][a-zA-Z0-9_-]*)\\s*=\\s*(?<value>.*)$"

# The characters that change how the JS literal parses.
const BREAKERS = "[\u{27}\\\\]"

def catalog-entries [] {
    let files = (glob server/locales/**/*.ftl | sort)
    if ($files | is-empty) {
        print --stderr "No catalogs found under server/locales/"
        exit 1
    }
    $files | each {|file|
        open --raw $file
        | decode utf-8
        | lines
        | enumerate
        | each {|row|
            $row.item | parse --regex $ENTRY | each {|e|
                {key: $e.key, value: $e.value, file: $file, line: ($row.index + 1)}
            }
        }
        | flatten
    } | flatten
}

# Every catalog key interpolated into an inline confirm(), with its site.
def confirm-sites [files: list<string>] {
    $files | each {|file|
        open --raw $file
        | decode utf-8
        | lines
        | enumerate
        | each {|row|
            let calls = ($row.item | parse --regex $CALL)
            if (($calls | is-empty) and ($row.item =~ $CALL_OPEN)) {
                {file: $file, line: ($row.index + 1), key: null}
            } else {
                $calls | each {|call|
                    $call.arg | parse --regex $KEY | each {|k|
                        {file: $file, line: ($row.index + 1), key: $k.key}
                    }
                } | flatten
            }
        }
        | flatten
    } | flatten
}

def main [] {
    let files = (glob server/templates/**/*.html --exclude ["**/email/**"] | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }

    let entries = (catalog-entries)
    let sites = (confirm-sites $files)

    let unreadable = (
        $sites | where {|s| $s.key == null } | each {|s| $"($s.file):($s.line)" }
    )
    if ($unreadable | is-not-empty) {
        print --stderr "confirm() call whose JS string literal does not close on the same line; keep the call on one line so this guard can read its keys:"
        for u in $unreadable { print --stderr $"  ($u)" }
        exit 1
    }

    let used = ($sites | where {|s| $s.key != null })
    if ($used | is-empty) {
        print --stderr "No confirm() call sites found under server/templates/; the guard would pass vacuously."
        exit 1
    }

    let missing = (
        $used
        | where {|s| ($entries | where key == $s.key | is-empty) }
        | each {|s| $"($s.file):($s.line): ($s.key)" }
        | uniq
    )
    if ($missing | is-not-empty) {
        print --stderr "confirm() interpolates a key that no catalog under server/locales/ defines:"
        for m in $missing { print --stderr $"  ($m)" }
        exit 1
    }

    let problems = (
        $used | each {|s|
            $entries
            | where {|e| ($e.key == $s.key) and ($e.value =~ $BREAKERS) }
            | each {|e| $"($e.file):($e.line): ($e.key) -- used at ($s.file):($s.line)" }
        } | flatten | uniq | sort
    )

    if ($problems | is-not-empty) {
        print --stderr "Apostrophe or backslash in a catalog value interpolated into an inline confirm('...'); askama escapes it, the HTML parser hands the bare character to the JS compiler, and the confirmation silently never runs. Reword the string without it:"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }

    print $"confirm\(\) strings OK: ($used | get key | uniq | length) keys across ($used | length) call sites, checked against ($entries | length) catalog entries."
}
