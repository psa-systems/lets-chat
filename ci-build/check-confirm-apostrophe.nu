#!/usr/bin/env nu

# Guard: confirmation text is attribute text, never a JavaScript string literal
# (LC-753, LC-771).
#
# Askama escapes every `{{ }}` through askama_escape, which maps `'` to
# `&#x27;`. The HTML parser decodes that back to a bare `'` before an event
# handler attribute is compiled as JavaScript, so any apostrophe interpolated
# into `onclick="return confirm('...')"` ends the JS literal early and the
# handler never compiles. The dialog then never appears and the destructive
# action runs on the first click, silently. LC-753 saw it from catalog strings
# (`admin-webhooks-rotate-confirm`); LC-771 from runtime data (a member display
# name such as O'Brien in the enclave transfer and kick confirmations).
#
# The rule: a template never builds a JS string out of text. The text lives in
# a `data-lc-confirm` attribute on the element that carries the handler, and
# the handler reads it back: `confirm(this.getAttribute('data-lc-confirm'))`.
# Attribute text is decoded by the HTML parser and handed to JavaScript as a
# value, so it never meets the JS parser and any character round-trips. With
# no template building a literal, no catalog or runtime value can end one, so
# this rule subsumes the LC-753 catalog scan it replaces.

# The one accepted call shape.
const CANON = "confirm(this.getAttribute('data-lc-confirm'))"

# A `confirm(` whose argument opens a JS string literal, anywhere in a template
# (handler attributes and inline scripts alike).
const LITERAL = "confirm\\(\\s*[\u{27}\"`]"

# An inline event handler that calls `confirm(` at all.
const HANDLER = "\\bon[a-z]+=\"[^\"]*confirm\\("

# The attribute the canonical call reads.
const ATTR = "data-lc-confirm=\""

# The line an element's opening tag starts on: the nearest line at or above
# `i` that begins with `<tag`. Attributes of a multi-line tag sit on the lines
# between that one and `i`.
def element-start [lines: list<string>, i: int] {
    let starts = (
        $lines | first ($i + 1) | enumerate
        | where {|r| $r.item =~ "^\\s*<[a-zA-Z]" }
        | get index
    )
    if ($starts | is-empty) { 0 } else { $starts | last }
}

def scan [file: string] {
    let lines = (open --raw $file | decode utf-8 | lines)
    let rows = ($lines | enumerate)

    let literals = (
        $rows | where {|r| $r.item =~ $LITERAL }
        | each {|r| $"($file):($r.index + 1)" }
    )
    let handlers = (
        $rows
        | where {|r| ($r.item =~ $HANDLER) and (not ($r.item | str contains $CANON)) }
        | each {|r| $"($file):($r.index + 1)" }
    )
    let canon = ($rows | where {|r| $r.item | str contains $CANON })
    let orphans = (
        $canon | each {|r|
            let start = (element-start $lines $r.index)
            let element = ($lines | skip $start | first ($r.index - $start + 1) | str join " ")
            if ($element | str contains $ATTR) { null } else { $"($file):($r.index + 1)" }
        } | compact
    )
    {
        literals: $literals,
        handlers: $handlers,
        orphans: $orphans,
        canon: ($canon | length),
    }
}

def main [] {
    let files = (glob server/templates/**/*.html --exclude ["**/email/**"] | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }

    let results = ($files | each {|f| scan $f })
    let literals = ($results | get literals | flatten)
    let handlers = ($results | get handlers | flatten)
    let orphans = ($results | get orphans | flatten)
    let canon = ($results | get canon | math sum)

    if ($literals | is-not-empty) {
        print --stderr "confirm() called with a JS string literal; askama escapes an apostrophe in the text, the HTML parser hands the bare character to the JS compiler, and the confirmation silently never runs. Put the text in data-lc-confirm on the same element and call confirm(this.getAttribute('data-lc-confirm')):"
        for p in $literals { print --stderr $"  ($p)" }
        exit 1
    }
    if ($handlers | is-not-empty) {
        print --stderr "inline handler calls confirm() with something other than this.getAttribute('data-lc-confirm'); the confirmation text must be attribute text on the same element:"
        for p in $handlers { print --stderr $"  ($p)" }
        exit 1
    }
    if ($orphans | is-not-empty) {
        print --stderr "confirm(this.getAttribute('data-lc-confirm')) on an element with no data-lc-confirm attribute; the dialog would show 'null'. Put the attribute on the same element as the handler:"
        for p in $orphans { print --stderr $"  ($p)" }
        exit 1
    }
    if $canon == 0 {
        print --stderr "No confirm(this.getAttribute('data-lc-confirm')) call sites found under server/templates/; the guard would pass vacuously."
        exit 1
    }

    print $"confirm\(\) call sites OK: ($canon) attribute-backed sites across ($files | length) templates, no JS string literals."
}
