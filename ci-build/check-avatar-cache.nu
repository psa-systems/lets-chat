#!/usr/bin/env nu

# Guard versioned avatar URLs in templates and client JS (LC-781 F11, LC-784).
#
# `/avatars/{id}` answers `Cache-Control: no-cache` + ETag, so a bare URL forces
# a conditional request (plus a `find_user_by_id` and a `stat`) on every page
# load for every distinct author shown - about 20 revalidations and 60 queries
# on a timeline of 20 authors, all to discover nothing changed. The fix is to
# render `/avatars/{id}?v={token}`: the route then answers `immutable` and the
# browser skips revalidation, while a re-upload moves the token (preserving
# LC-348's freshness guarantee).
#
# Server-rendered surfaces build the URL through the `avatar_url` Askama filter
# (`{{ user_id|avatar_url }}`). The call/voice surfaces build their URL in JS
# from a WS-delivered id, so LC-784 carries the version token alongside the id
# (a `data-*` attribute / a roster tuple element) and the JS appends `?v=`.
#
# This guard fails on a bare avatar URL so a new surface cannot silently
# reintroduce the per-navigation revalidation:
#   - templates: a `src=`/`href=` avatar URL attribute with no `?v=`.
#   - client JS: a `/avatars/` string literal with no `?v=` on the same line
#     (comment lines are prose, not requests, so they are skipped).
# A literal `/avatars/...?v=...` (e.g. the LC-432 settings preview element,
# which its own JS re-stamps) is versioned and therefore allowed.

def main [] {
    let templates = (glob server/templates/**/*.html | sort)
    let scripts = (glob server/assets/**/*.js | sort)
    if ($templates | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }
    if ($scripts | is-empty) {
        print --stderr "No scripts found under server/assets/"
        exit 1
    }

    let template_problems = (
        $templates | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row| ($row.item =~ '(src|href)="/avatars/') and ($row.item !~ '\?v=') }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } | flatten
    )

    let script_problems = (
        $scripts | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row|
                let line = ($row.item | str trim)
                # Skip comment lines: they mention the route as prose, not as a
                # request. `//` line comments and ` * ` block-comment bodies.
                let is_comment = (($line | str starts-with "//") or ($line | str starts-with "*"))
                (not $is_comment) and ($row.item =~ '/avatars/') and ($row.item !~ '\?v=')
            }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } | flatten
    )

    let problems = ($template_problems | append $script_problems)

    if ($problems | is-not-empty) {
        print --stderr "Bare /avatars/ URL. Version it so the route can answer immutable and the browser skips revalidation (templates: the avatar_url filter {{ user_id|avatar_url }}; JS: append ?v= from the WS-delivered token, LC-781 F11 / LC-784):"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"Avatar cache tokens OK across ($templates | length) templates and ($scripts | length) scripts."
}
