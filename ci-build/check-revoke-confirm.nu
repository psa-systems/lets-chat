#!/usr/bin/env nu

# Guard the confirmation in front of every revoke control (LC-738).
#
# Revoking a credential is irreversible: the token or secret is gone and every
# integration using it breaks on the next request. Five of the six revoke
# surfaces fired on a single click of a small text link sitting in a table row,
# with no confirmation and nothing separating it from the row's other cells.
# Only admin/invites.html asked first.
#
# The rule: a form or button that targets a `/revoke` endpoint carries a
# confirmation inside it, either `hx-confirm` (htmx-driven controls) or
# `onclick="return confirm(...)"` (native POST forms, which htmx does not see).
# Email templates are excluded: they render in a mail client with no scripting.

const ENDPOINT = '(?:action|href|hx-post|hx-put|hx-patch|hx-delete)="[^"]*/revoke"'

# The source of an element, from its opening tag to its first closing tag.
def block-text [lines: list<string>, start: int, close: string] {
    let rest = ($lines | skip $start)
    let ends = ($rest | enumerate | where {|r| $r.item =~ $close } | get index)
    let n = (if ($ends | is-empty) { $rest | length } else { ($ends | first) + 1 })
    $rest | first $n | str join " "
}

def elements [lines: list<string>, open: string, close: string] {
    $lines
    | enumerate
    | where {|row| $row.item =~ $open }
    | each {|row| {line: ($row.index + 1), text: (block-text $lines $row.index $close)} }
}

def main [] {
    let files = (glob server/templates/**/*.html --exclude ["**/email/**"] | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }

    let problems = (
        $files | each {|file|
            let lines = (open --raw $file | decode utf-8 | lines)
            (elements $lines '<form' '</form>')
            | append (elements $lines '<button' '</button>')
            | where {|el| ($el.text =~ $ENDPOINT) and ($el.text !~ 'confirm') }
            | each {|el| $"($file):($el.line)" }
        } | flatten
    )

    if ($problems | is-not-empty) {
        print --stderr "Revoke controls with no confirmation; add `hx-confirm` (htmx) or `onclick=\"return confirm(...)\"` (native POST form) with a translated string:"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"Revoke confirmations OK across ($files | length) templates."
}
