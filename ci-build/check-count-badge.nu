#!/usr/bin/env nu

# Guard the single count-badge component (LC-889).
#
# Six separate count-badge implementations used to exist (`.lc-count-pill`,
# `.lc-count-pill--alert`, `.lc-rail-badge`, and two hand-rolled danger-red
# spans), with an uneven accessibility story: only some carried an
# `aria-label`, so a screen reader announced a bare numeral on the rest.
# They are consolidated onto `partials/unread_badge.html`'s `badge` macro,
# which always renders `aria-label="{count} {noun}"`.
#
# This guard fails on a literal `lc-count-pill`, `lc-count-pill--alert`, or
# `lc-rail-badge` class outside `partials/unread_badge.html` and its
# OOB-swap counterpart `ws/unread_badge.html` (the only two files allowed to
# own that markup) and `server/assets/main.css` (which still defines the
# classes), so a new badge cannot silently bypass the macro and reintroduce
# the unlabeled-numeral gap.

def main [] {
    let templates = (glob server/templates/**/*.html | sort)
    if ($templates | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }

    let allowed = ["partials/unread_badge.html" "ws/unread_badge.html"]

    let problems = (
        $templates | where {|file| not ($allowed | any {|a| $file | str ends-with $a }) } | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row| $row.item =~ '"[^"]*\b(lc-count-pill|lc-count-pill--alert|lc-rail-badge)\b[^"]*"' }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } | flatten
    )

    if ($problems | is-not-empty) {
        print --stderr "Raw count-badge class outside the shared component. Render the badge through partials/unread_badge.html's `badge` macro instead of a literal lc-count-pill / lc-count-pill--alert / lc-rail-badge class, so every badge keeps its aria-label (LC-889):"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"Count-badge component OK across ($templates | length) templates."
}
