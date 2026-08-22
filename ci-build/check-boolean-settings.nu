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
#
# LC-751 added the second half: the `.lc-toggle` row markup itself lives once, in
# templates/partials/toggle_row.html, so a new call site cannot omit the track or
# nest the text spans differently. Only the two rows named in EXEMPT_ROWS may
# still spell it out.

const PARTIAL = "server/templates/partials/settings_toggle.html"
const ROW_PARTIAL = "server/templates/partials/toggle_row.html"

# The rows that stay hand-written, and why. Their descriptions carry inline
# markup, which askama escapes when it comes through a `{% let %}` binding, so
# routing them through the partial would need a `|safe` desc and would put
# translated content on an unescaped path for the sake of two links. `rows` is
# the number of hand-written `.lc-toggle` labels the file may contain; a stale or
# extra one fails this check.
const EXEMPT_ROWS = [
    # The push row's unavailable reason names LETS_CHAT_SECRET_KEY in a <code>.
    { file: "server/templates/settings/page.html", rows: 1 }
    # The link-filter note links to /admin/link-filter.
    { file: "server/templates/admin/anti_spam.html", rows: 1 }
]

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

    # Every `.lc-toggle` row renders through ROW_PARTIAL. A file outside it and
    # outside EXEMPT_ROWS may not spell out the row's markup at all.
    let exempt_files = ($EXEMPT_ROWS | get file)
    let rows = (
        $files
        | where {|f| not ($f | str ends-with $ROW_PARTIAL) }
        | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row| $row.item =~ 'class="lc-toggle"|lc-toggle-track|lc-toggle-text|lc-toggle-title|lc-toggle-desc' }
            | each {|row| { file: $file, line: ($row.index + 1), text: ($row.item | str trim) } }
        }
        | flatten
    )

    let stray = ($rows | where {|r| ($exempt_files | any {|e| $r.file | str ends-with $e }) == false })
    if ($stray | is-not-empty) {
        print --stderr $"Hand-written `.lc-toggle` row markup outside ($ROW_PARTIAL); include the partial with its `name` / `checked` / `disabled` / `title` / `desc` bindings instead:"
        for r in $stray { print --stderr $"  ($r.file):($r.line): ($r.text)" }
        exit 1
    }

    for e in $EXEMPT_ROWS {
        let found = (
            $rows
            | where {|r| ($r.file | str ends-with $e.file) and ($r.text =~ 'class="lc-toggle"') }
            | length
        )
        if $found != $e.rows {
            print --stderr $"($e.file) has ($found) hand-written `.lc-toggle` rows, expected ($e.rows). Route a new row through ($ROW_PARTIAL), or drop the stale exemption from EXEMPT_ROWS in this script."
            exit 1
        }
    }

    print $"Boolean-setting control OK across ($files | length) templates."
}
