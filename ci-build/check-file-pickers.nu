#!/usr/bin/env nu

# Guard the one file-picker shape in templates (LC-740).
#
# Every user-facing file input renders through templates/partials/file_picker.html,
# which keeps the native input `sr-only` behind a styled label so the filename
# echo, the type/size rejection and the inline role="alert" slot are always there.
# A raw `<input type="file">` dropped straight into a template silently opts out
# of all three: the user picks a file, nothing on the page changes, and an
# oversized or wrong-type file is only rejected after the POST re-renders the
# page and drops their other unsaved edits. That is exactly the state four admin
# and settings forms were in before this guard existed.
#
# The one exemption is `class="hidden"` (room/composer.html), a
# programmatically-driven attachment input with no visible picker chrome at all.

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
            | where {|row| ($row.item =~ 'type="file"') and ($row.item !~ 'sr-only|class="hidden"') }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } | flatten
    )

    if ($problems | is-not-empty) {
        print --stderr "Raw file inputs found in templates; render them through partials/file_picker.html (styled label + sr-only input + filename echo + inline error):"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"File-picker shape OK across ($files | length) templates."
}
