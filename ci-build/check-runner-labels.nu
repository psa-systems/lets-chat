#!/usr/bin/env nu

# Guard the Forgejo runner-label split (LC-642 / LC-647, DEV-769).
# Jobs that run cargo natively or `docker build` need RUNS_ON_OPENSUSE_BASE_HEAVY
# (the dev image, with a C toolchain); everything else runs on
# RUNS_ON_OPENSUSE_BASE_MEDIUM. Installing a toolchain at job time is the
# workaround this guard rejects.

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

        # A bare `cargo ...` compiles on the runner; `docker build` compiles in
        # the image. Both are HEAVY work.
        let heavy_work = ($lines | where {|l| ($l =~ '^\s*(- )?(run:\s*)?\^?cargo\s') or ($l =~ '\bdocker\s+(buildx\s+)?build\s') })
        let on_medium = ($labels | any {|l| $l =~ 'RUNS_ON_OPENSUSE_BASE_MEDIUM' })
        if (($heavy_work | is-not-empty) and $on_medium) {
            $problems = ($problems | append $"($file): compiles \(cargo or docker build\) but requests MEDIUM; use vars.RUNS_ON_OPENSUSE_BASE_HEAVY")
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
