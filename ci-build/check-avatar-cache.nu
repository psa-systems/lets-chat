#!/usr/bin/env nu

# Guard versioned avatar URLs in templates (LC-781, finding F11).
#
# `/avatars/{id}` answers `Cache-Control: no-cache` + ETag, so a bare URL forces
# a conditional request (plus a `find_user_by_id` and a `stat`) on every page
# load for every distinct author shown - about 20 revalidations and 60 queries
# on a timeline of 20 authors, all to discover nothing changed. The fix is to
# render `/avatars/{id}?v={token}` via the `avatar_url` Askama filter
# (`{{ user_id|avatar_url }}`): the route then answers `immutable` and the
# browser skips revalidation, while a re-upload moves the token (preserving
# LC-348's freshness guarantee). This guard fails on a literal bare avatar URL
# in a template attribute so a new surface cannot silently reintroduce the
# per-navigation revalidation.
#
# It scans TEMPLATES only. The three client-side call/voice avatar URLs built in
# `server/assets/*.js` build their URL from a WS-delivered id and are the tracked
# exclusion (LC-784 carries the token into those frames and extends this guard to
# the JS). A literal `/avatars/...?v=...` (the LC-432 settings preview element,
# which its own JS re-stamps) is versioned and therefore allowed.

def main [] {
    let files = (glob server/templates/**/*.html | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }

    let problems = (
        $files | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row| ($row.item =~ '(src|href)="/avatars/') and ($row.item !~ '\?v=') }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } | flatten
    )

    if ($problems | is-not-empty) {
        print --stderr "Bare /avatars/ URL in a template. Render it versioned via the avatar_url filter ({{ user_id|avatar_url }}) so the route can answer immutable and the browser skips revalidation (LC-781 F11):"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"Avatar cache tokens OK across ($files | length) templates."
}
