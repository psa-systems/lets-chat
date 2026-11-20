#!/usr/bin/env nu

# Guard docs/public/configuration.md against fabricated env vars.
#
# The doc presents itself as the single source of truth for every server env
# var. A parity audit found several `LETS_CHAT_*` names and one behavioral
# claim in it that did not match `server/src`: the doc named a variable the
# server never reads, so an operator who set it got no effect and no error. Every
# `LETS_CHAT_*` name the doc claims must appear as a Rust string literal
# somewhere in `server/src` (the key every env-reading helper in this codebase
# takes as an argument), or this fails the build instead of waiting for the
# next parity audit.

const DOC = "docs/public/configuration.md"
const SRC_GLOB = "server/src/**/*.rs"

def main [] {
    if not ($DOC | path exists) {
        print --stderr $"($DOC) not found"
        exit 1
    }

    let doc_vars = (
        open --raw $DOC
        | parse --regex '(?<name>LETS_CHAT_[A-Z0-9_]+)'
        | get name
        | uniq
    )

    let src_text = (glob $SRC_GLOB | each {|f| open --raw $f } | str join "\n")

    let read_vars = (
        $src_text
        | parse --regex '"(?<name>LETS_CHAT_[A-Z0-9_]+)"'
        | get name
        | uniq
    )

    let missing = ($doc_vars | where {|v| not ($v in $read_vars) })

    if ($missing | is-not-empty) {
        print --stderr $"($DOC) names LETS_CHAT_* variables server/src never reads:"
        for v in $missing { print --stderr $"  ($v)" }
        exit 1
    }
    print $"($DOC) OK: ($doc_vars | length) LETS_CHAT_* names all read in server/src."
}
