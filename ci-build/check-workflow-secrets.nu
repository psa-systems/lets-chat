#!/usr/bin/env nu

# LC-987: workflow secrets stay out of argv and `${{ }}` stays out of script
# source. A `--password <value>` argument is visible in the process list, and an
# expression substituted into a `run:` body turns event-controlled text into
# code. Values reach a script through `env:` and are read as `$env.NAME`.
#
# Scope: the publish workflows named in LC-987. Fails on a `--password` flag
# not followed by stdin form, and on any `${{` inside a `run: |` body.

const FILES = [
    ".forgejo/workflows/publish-release.yml"
    ".forgejo/workflows/build-agent-image.yml"
    ".forgejo/workflows/build-oci-image.yml"
]

def violations [lines: list<string>]: nothing -> list<string> {
    mut in_run = false
    mut out = []
    for l in ($lines | enumerate) {
        let t = ($l.item | str trim)
        if ($t | str starts-with "- name:") or ($t | str starts-with "env:") { $in_run = false }
        if $t == "run: |" { $in_run = true; continue }
        if $in_run and ($t | str contains '${{') { $out = ($out | append $"line ($l.index + 1): expression in run body: ($t)") }
        if ($t | str contains "--password") and not ($t | str contains "--password-stdin") { $out = ($out | append $"line ($l.index + 1): --password argument: ($t)") }
    }
    $out
}

# Self-test: a hollowed-out rule must fail before any workflow is read.
if ((violations ['run: |' '  let a = "${{ x }}"']) | is-empty) or ((violations ['  --password $x']) | is-empty) {
    error make {msg: "check-workflow-secrets self-test failed"}
}

let bad = ($FILES | each {|f| violations (open --raw $f | lines) | each {|v| $"($f): ($v)" } } | flatten)
if ($bad | is-not-empty) {
    $bad | each {|b| print -e $b }
    exit 1
}
print "check-workflow-secrets: ok"
