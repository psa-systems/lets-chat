#!/usr/bin/env nu

# Guard the desktop self-updater's release contract (LC-831).
#
# The updater resolves a registry coordinate and a per-platform tag that only
# the release workflow creates, and it compiles those coordinates in at build
# time. Nothing tied the two halves together, so they drifted in silence: the
# workflow injected LETS_CHAT_UPDATE_BASE_URL, a name no Rust source has read
# since LC-733, while the names update.rs does read were injected by nothing and
# every release shipped the in-source fallback. The client then resolved
# `latest-{os}-x86_64` tags that no step had ever pushed, and a 404 on every
# check looked like a transient outage.
#
# The desktop crate's own tests assert the same thing, but check.yml cannot run
# them (the ./dev/cargo-desktop bind mount does not survive this runner), so
# this guard is what actually fails a pull request.
#
# Four rules, all derived from the sources rather than a hardcoded list:
#   A. Every desktop image the RELEASE workflow builds declares, exports and is
#      passed every `option_env!("LETS_CHAT_*")` name the Rust sources read.
#   B. Every LETS_CHAT_* build arg or Dockerfile ARG anywhere is read by some
#      Rust source. Dead plumbing still reads as a live safeguard.
#   C. Any build that uses a Dockerfile declaring a LETS_CHAT_* ARG passes it.
#   D. Every tag PUBLISHED_PLATFORM_TAGS lets the client resolve is pushed by
#      the release workflow, and the workflow pushes no tag no client resolves.

const RELEASE_WORKFLOW: string = ".forgejo/workflows/publish-release.yml"
const UPDATE_SOURCE: string = "desktop/src/update.rs"

# Names the Rust sources read at compile time, which is the set the build must
# inject. `option_env!` needs a literal, so the source text is the only place
# these exist.
def rust_option_env_names []: nothing -> list<string> {
    (glob desktop/src/**/*.rs) ++ (glob server/src/**/*.rs)
    | each {|f|
        open --raw $f
        | parse --regex 'option_env!\("(?<name>[A-Za-z_][A-Za-z0-9_]*)"\)'
        | get name
    }
    | flatten
    | uniq
    | sort
}

# `ARG NAME` declarations in a Dockerfile.
def dockerfile_args [file: string]: nothing -> list<string> {
    open --raw $file
    | parse --regex '(?m)^ARG\s+(?<name>[A-Za-z_][A-Za-z0-9_]*)'
    | get name
    | uniq
}

# One record per `docker buildx build` invocation in a workflow: the Dockerfile
# it names and the build args passed before that `-f`, which is the order every
# invocation in this repo is written in.
def buildx_invocations [file: string]: nothing -> table {
    open --raw $file
    | split row "docker buildx build"
    | skip 1
    | each {|chunk|
        let dockerfile = ($chunk | parse --regex '-f\s+(?<f>\S+)' | get f)
        if ($dockerfile | is-empty) {
            null
        } else {
            {
                dockerfile: ($dockerfile | first)
                build_args: (
                    $chunk
                    | split row "-f "
                    | first
                    | parse --regex '--build-arg\s+\$?"?(?<name>[A-Za-z_][A-Za-z0-9_]*)='
                    | get name
                )
            }
        }
    }
    | compact
}

def main [] {
    let read_by_rust = (rust_option_env_names)
    let update_vars = ($read_by_rust | where {|n| $n | str starts-with "LETS_CHAT_" })
    if ($update_vars | is-empty) {
        print --stderr $"No option_env! LETS_CHAT_* names found; ($UPDATE_SOURCE) no longer compiles in its update source."
        exit 1
    }

    mut problems = []

    # Rule A: the release build injects every name the client reads, and both
    # halves of the Dockerfile plumbing are present. An ARG the image never
    # exports is accepted by docker and never reaches rustc.
    for inv in (buildx_invocations $RELEASE_WORKFLOW) {
        if not ($inv.dockerfile | path exists) {
            $problems = ($problems | append $"($RELEASE_WORKFLOW): builds ($inv.dockerfile), which does not exist")
            continue
        }
        let text = (open --raw $inv.dockerfile)
        for name in $update_vars {
            if $name not-in $inv.build_args {
                $problems = ($problems | append $"($RELEASE_WORKFLOW): the ($inv.dockerfile) build passes no --build-arg ($name); the release would ship the in-source fallback")
            }
            if not ($text | str contains $"ARG ($name)") {
                $problems = ($problems | append $"($inv.dockerfile): no `ARG ($name)`, so the release's build arg is discarded")
            }
            # Literal `${NAME}`: the ENV line that re-exports the ARG.
            if not ($text | str contains $"($name)=${($name)}") {
                $problems = ($problems | append $"($inv.dockerfile): `($name)` is never exported into the build environment, so rustc cannot see it")
            }
        }
    }

    # Rules B and C, over every workflow and every Dockerfile. Paths are made
    # repo-relative so a failure reads like the file you would open.
    for wf in (glob .forgejo/workflows/*.yml | sort | each {|p| $p | path relative-to (pwd) }) {
        for inv in (buildx_invocations $wf) {
            let declared = (
                if ($inv.dockerfile | path exists) { dockerfile_args $inv.dockerfile } else { [] }
                | where {|a| $a | str starts-with "LETS_CHAT_" }
            )
            for name in ($inv.build_args | where {|a| $a | str starts-with "LETS_CHAT_" }) {
                if $name not-in $read_by_rust {
                    $problems = ($problems | append $"($wf): injects ($name), which no Rust source reads through option_env!")
                }
            }
            for name in $declared {
                if $name not-in $inv.build_args {
                    $problems = ($problems | append $"($wf): builds ($inv.dockerfile), which declares ARG ($name), but passes no value for it")
                }
            }
        }
    }
    for df in (glob ci-build/Dockerfile.* | sort | each {|p| $p | path relative-to (pwd) }) {
        for name in (dockerfile_args $df | where {|a| $a | str starts-with "LETS_CHAT_" }) {
            if $name not-in $read_by_rust {
                $problems = ($problems | append $"($df): declares ARG ($name), which no Rust source reads through option_env!")
            }
        }
    }

    # Rule D: the tags the client resolves are the tags the release pushes.
    let block = (
        open --raw $UPDATE_SOURCE
        | split row "const PUBLISHED_PLATFORM_TAGS"
        | get 1
        | split row "];"
        | first
    )
    let client_tags = (
        $block
        | parse --regex '"(?<os>[^"]+)",\s*"(?<arch>[^"]+)",\s*"(?<tag>[^"]+)"'
        | get tag
    )
    if ($client_tags | is-empty) {
        $problems = ($problems | append $"($UPDATE_SOURCE): PUBLISHED_PLATFORM_TAGS parsed empty; the client resolves nothing")
    }
    let pushed_tags = (
        open --raw $RELEASE_WORKFLOW
        | parse --regex 'tag: "(?<tag>[^"]+)"'
        | get tag
        | uniq
    )
    for tag in $client_tags {
        if $tag not-in $pushed_tags {
            $problems = ($problems | append $"($RELEASE_WORKFLOW): never pushes ($tag), which the client resolves; every update check would 404")
        }
    }
    for tag in $pushed_tags {
        if $tag not-in $client_tags {
            $problems = ($problems | append $"($RELEASE_WORKFLOW): pushes ($tag), which no client resolves")
        }
    }

    if ($problems | is-not-empty) {
        print --stderr "Desktop update injection guard failed:"
        for p in $problems { print --stderr $"  ($p)" }
        print --stderr ""
        print --stderr $"The client half lives in ($UPDATE_SOURCE); the publisher half in ($RELEASE_WORKFLOW). See LC-831."
        exit 1
    }

    print $"Update injection OK: ($update_vars | str join ', ') injected by the release build; ($client_tags | str join ', ') published."
}
