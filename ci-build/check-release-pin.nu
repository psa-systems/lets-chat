#!/usr/bin/env nu

# Guard the create-release.yml / common submodule pin contract (LC-1049).
#
# `.forgejo/workflows/create-release.yml` pins its `uses:` reference to the
# `common` reusable workflow by SHA, with a comment stating the invariant:
# that SHA must equal `git ls-tree HEAD common`. LC-1044 bumped the submodule
# gitlink for an unrelated reason and its own stale-wording sweep missed this
# pin, so the two values silently disagreed from 2026-10-10 until this check
# was added (LC-1049). Without a CI signal, a future `common` bump can drift
# the pin again with nobody noticing until a parity audit catches it.

const WORKFLOW_FILE: string = ".forgejo/workflows/create-release.yml"

def parse_ls_tree [line: string]: nothing -> string {
    $line | split row "\t" | first | split row " " | last
}

def parse_pin [lines: list<string>]: nothing -> string {
    $lines
    | where {|l| $l | str contains "uses: psa-systems/common/" }
    | first
    | split row "@"
    | last
    | str trim
}

# Self-test: a hollowed-out rule must fail before the real files are read.
if (parse_ls_tree "160000 commit 690a469ec3d4194a9e9b7ebfba03df936ac6c517\tcommon") != "690a469ec3d4194a9e9b7ebfba03df936ac6c517" {
    error make {msg: "check-release-pin self-test failed: parse_ls_tree"}
}
if (parse_pin ["    uses: psa-systems/common/.forgejo/workflows/create-release.yml@deadbeef"]) != "deadbeef" {
    error make {msg: "check-release-pin self-test failed: parse_pin"}
}

let expected = (parse_ls_tree (git ls-tree HEAD common | str trim))
let actual = (parse_pin (open --raw $WORKFLOW_FILE | lines))

if $expected != $actual {
    print --stderr $"Release pin guard failed: ($WORKFLOW_FILE) pins common@($actual), but `git ls-tree HEAD common` is ($expected)."
    print --stderr $"Update the `uses:` SHA in ($WORKFLOW_FILE) to ($expected). See LC-1049."
    exit 1
}

print $"Release pin OK: ($WORKFLOW_FILE) matches common@($expected)."
