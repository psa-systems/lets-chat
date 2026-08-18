#!/usr/bin/env nu

# Guard the one boolean-setting control (LC-747).
#
# A "turn this on" setting renders one of two ways, and both must announce as a
# switch:
#
#   - templates/partials/settings_toggle.html, an immediately-applied
#     role="switch" button that re-renders itself with the new state. It is the
#     ONLY place that may hand-roll `.lc-switch` markup; a copy elsewhere is the
#     four-times-duplicated enclave block this ticket removed, and every copy
#     drifts away from the shared feedback contract.
#   - a `.lc-toggle` label wrapping a native checkbox, for settings that batch
#     into one form with a Save button. The checkbox needs an explicit
#     role="switch": without it the control looks like a switch and announces as
#     a checkbox, so the same setting is described differently depending on which
#     page it lives on.
#
# Checkboxes that are one option among several (a scopes or events list) are NOT
# switches and correctly stay checkboxes inside a fieldset+legend; they carry no
# `.lc-toggle` class, so this guard leaves them alone.

const PARTIAL = "server/templates/partials/settings_toggle.html"

# The `.lc-toggle` rows in one template, paired with the input line that follows.
def toggle-rows [file: string] {
    let rows = (open --raw $file | lines)
    $rows
    | enumerate
    | where {|row| $row.item =~ 'class="lc-toggle"' }
    | each {|row|
        let rest = ($rows | skip ($row.index + 1) | where {|l| ($l | str trim) != "" })
        let next = (if ($rest | is-empty) { "" } else { $rest | first })
        { file: $file, line: ($row.index + 1), input: ($next | str trim) }
    }
}

def main [] {
    let files = (glob server/templates/**/*.html | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }

    let unswitched = (
        $files | each {|file| toggle-rows $file } | flatten
        | where {|r| $r.input !~ 'role="switch"' }
    )
    if ($unswitched | is-not-empty) {
        print --stderr "`.lc-toggle` checkboxes with no role=\"switch\"; a boolean setting must announce as a switch:"
        for r in $unswitched { print --stderr $"  ($r.file):($r.line): ($r.input)" }
        exit 1
    }

    let copies = (
        $files
        | where {|f| $f !~ 'settings_toggle.html' }
        | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row| $row.item =~ 'lc-switch-thumb|class="lc-switch"' }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        }
        | flatten
    )
    if ($copies | is-not-empty) {
        print --stderr $"Hand-rolled switch markup outside ($PARTIAL); include the partial instead:"
        for c in $copies { print --stderr $"  ($c)" }
        exit 1
    }

    print $"Boolean-setting control OK across ($files | length) templates."
}
