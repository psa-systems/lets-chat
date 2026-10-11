#!/usr/bin/env nu

# Guard the release changelog contract (LC-1047).
#
# `just create-release` bumps the version at `[workspace.package].version` in
# Cargo.toml and opens a `release/vX.Y.Z` PR, but nothing ties that version to
# CHANGELOG.md actually growing a matching section: three consecutive tagged
# releases (v0.3.0 / LC-968, v0.4.0 / LC-1038, v0.4.1 / LC-1046) each skipped
# the changelog-section checklist step at least once before an audit or a
# human caught it, days after the release had already shipped.
#
# The check is scoped to release branches: a normal feature branch has no
# reason to carry a changelog heading for the current workspace version yet,
# so this is a no-op everywhere except a `release/v*` branch (locally, the
# checked-out branch; in CI, the pull request's head ref or the pushed ref).

const CARGO_MANIFEST: string = "Cargo.toml"
const CHANGELOG: string = "CHANGELOG.md"

# The branch this check gates on: the PR head ref in a pull_request run, the
# pushed ref in a push run, else the locally checked-out branch.
def current_branch []: nothing -> string {
    let head_ref = ($env.GITHUB_HEAD_REF? | default "")
    if $head_ref != "" {
        return $head_ref
    }
    let ref_name = ($env.GITHUB_REF_NAME? | default "")
    if $ref_name != "" {
        return $ref_name
    }
    (git rev-parse --abbrev-ref HEAD | str trim)
}

def workspace_version []: nothing -> string {
    open --raw $CARGO_MANIFEST
    | split row "[workspace.package]"
    | get 1
    | split row "\n["
    | first
    | parse --regex 'version\s*=\s*"(?<version>[^"]+)"'
    | get version
    | first
}

def main [] {
    let branch = (current_branch)
    if not ($branch | str starts-with "release/v") {
        print $"Changelog section guard skipped: ($branch) is not a release/v* branch."
        return
    }

    let version = (workspace_version)
    let heading = $"## [v($version)]"
    let changelog = (open --raw $CHANGELOG)

    if not ($changelog | str contains $heading) {
        print --stderr $"Changelog section guard failed: ($CHANGELOG) has no `($heading)` heading for the ($CARGO_MANIFEST) version \(($version)\) on ($branch)."
        print --stderr $"Add a `($heading)` section to ($CHANGELOG) before this release PR can merge. See LC-1047."
        exit 1
    }

    print $"Changelog section OK: ($CHANGELOG) has `($heading)` for ($branch)."
}
