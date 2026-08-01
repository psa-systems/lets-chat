#!/usr/bin/env nu

# Guard the Forgejo runner-label split (LC-642 / LC-647).
# Jobs that compile natively on the runner need the dev image's C toolchain
# (RUNS_ON_OPENSUSE_DEV_LATEST); everything else stays on base. Installing a
# toolchain at job time is the workaround this guard rejects.

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

        let unknown = ($labels | where {|l| not ($l =~ 'vars\.RUNS_ON_OPENSUSE_(BASE|DEV)_LATEST') })
        if ($unknown | is-not-empty) {
            $problems = ($problems | append $"($file): runs-on must use vars.RUNS_ON_OPENSUSE_BASE_LATEST or vars.RUNS_ON_OPENSUSE_DEV_LATEST")
        }

        # A bare `cargo ...` command compiles on the runner; a cargo build inside
        # `docker buildx` lives in the image and never matches this shape.
        let native_cargo = ($lines | where {|l| $l =~ '^\s*(- )?(run:\s*)?\^?cargo\s' })
        let on_base = ($labels | any {|l| $l =~ 'RUNS_ON_OPENSUSE_BASE_LATEST' })
        if (($native_cargo | is-not-empty) and $on_base) {
            $problems = ($problems | append $"($file): compiles natively \(cargo\) but requests the base runner; use vars.RUNS_ON_OPENSUSE_DEV_LATEST")
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
