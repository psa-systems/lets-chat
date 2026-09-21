#!/usr/bin/env nu

# Guard the Forgejo runner-label split (LC-642 / LC-647, DEV-769).
# Jobs that run cargo natively or `docker build` need RUNS_ON_OPENSUSE_BASE_HEAVY
# (the dev image, with a C toolchain); everything else runs on
# RUNS_ON_OPENSUSE_BASE_MEDIUM. Installing a toolchain at job time is the
# workaround this guard rejects.
#
# The compile-vs-label rule is judged PER JOB, not per file (LC-977). Judging a
# whole file meant one compiling job anywhere in it condemned every MEDIUM label
# beside it, so a secret scan that compiles nothing could not sit in `check.yml`
# next to the cargo job without being told to claim a HEAVY runner it does not
# need. That is backwards: the labels exist to keep cheap jobs off the expensive
# runner. The other two rules stay file-scoped, since both are true of a file
# regardless of which job carries the offending line.

# Split a workflow into its jobs. A job header is a two-space key directly under
# `jobs:`; everything more deeply indented belongs to the job above it. Keys
# under `on:` sit at the same depth, so collection starts only once `jobs:` is
# seen.
def job-chunks [lines: list<string>] {
    mut chunks = []
    mut name = ""
    mut body = []
    mut in_jobs = false
    for l in $lines {
        if ($l =~ '^jobs:') {
            $in_jobs = true
            continue
        }
        if not $in_jobs { continue }
        if ($l =~ '^  [A-Za-z0-9_.-]+:\s*$') {
            if $name != "" {
                $chunks = ($chunks | append {name: $name, lines: $body})
            }
            $name = ($l | str trim | str replace --regex ':$' '')
            $body = []
            continue
        }
        if $name != "" {
            $body = ($body | append $l)
        }
    }
    if $name != "" {
        $chunks = ($chunks | append {name: $name, lines: $body})
    }
    $chunks
}

# A bare `cargo ...` compiles on the runner; `docker build` compiles in the
# image. Both are HEAVY work.
def compiles [lines: list<string>] {
    ($lines | where {|l| ($l =~ '^\s*(- )?(run:\s*)?\^?cargo\s') or ($l =~ '\bdocker\s+(buildx\s+)?build\s') } | is-not-empty)
}

def main [] {
    let files = (glob .forgejo/workflows/*.yml | sort)
    if ($files | is-empty) {
        print --stderr "No workflows found under .forgejo/workflows/"
        exit 1
    }

    mut problems = []
    for file in $files {
        # Comments describe the rule; only real YAML is checked against it.
        let lines = (open --raw $file | lines | where {|l| not (($l | str trim) | str starts-with "#") })
        let labels = ($lines | where {|l| $l =~ 'runs-on:' })

        let unknown = ($labels | where {|l| not ($l =~ 'vars\.RUNS_ON_OPENSUSE_BASE_(HEAVY|MEDIUM)\b') })
        if ($unknown | is-not-empty) {
            $problems = ($problems | append $"($file): runs-on must use vars.RUNS_ON_OPENSUSE_BASE_HEAVY or vars.RUNS_ON_OPENSUSE_BASE_MEDIUM")
        }

        # Per job: only the job that actually compiles has to be HEAVY.
        let jobs = (job-chunks $lines)
        if ($jobs | is-empty) {
            # No parsable jobs: fall back to the file-wide rule rather than
            # passing silently, so a shape this parser does not understand is
            # still judged instead of skipped.
            let on_medium = ($labels | any {|l| $l =~ 'RUNS_ON_OPENSUSE_BASE_MEDIUM' })
            if ((compiles $lines) and $on_medium) {
                $problems = ($problems | append $"($file): compiles \(cargo or docker build\) but requests MEDIUM; use vars.RUNS_ON_OPENSUSE_BASE_HEAVY")
            }
        } else {
            for job in $jobs {
                let job_labels = ($job.lines | where {|l| $l =~ 'runs-on:' })
                let on_medium = ($job_labels | any {|l| $l =~ 'RUNS_ON_OPENSUSE_BASE_MEDIUM' })
                if ((compiles $job.lines) and $on_medium) {
                    $problems = ($problems | append $"($file): job '($job.name)' compiles \(cargo or docker build\) but requests MEDIUM; use vars.RUNS_ON_OPENSUSE_BASE_HEAVY")
                }
            }
        }

        let installs = ($lines | where {|l| $l =~ '(zypper|apt-get|dnf install|apk add)' })
        if ($installs | is-not-empty) {
            $problems = ($problems | append $"($file): installs packages at job time; the runner image owns that dependency")
        }
    }

    if ($problems | is-not-empty) {
        print --stderr "Runner-label guard failed:"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"Runner labels OK across ($files | length) workflows."
}
