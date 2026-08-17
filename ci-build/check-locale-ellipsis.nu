#!/usr/bin/env nu

# Guard one ellipsis spelling in the locale catalogs (LC-750, F23).
#
# Both catalogs used to mix three periods with U+2026 on the same kind of
# string, so the sidebar showed "Search..." beside "Search…" and the outbox
# banner "Sending..." beside "Sending %n% queued message(s)…". Three periods is
# the 35-key majority and the only spelling a plain grep of the catalogs finds,
# so U+2026 is rejected here rather than left to review.

def main [] {
    let files = (glob server/locales/**/*.ftl | sort)
    if ($files | is-empty) {
        print --stderr "No catalogs found under server/locales/"
        exit 1
    }

    let problems = (
        $files | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row| $row.item =~ "\u{2026}" }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } | flatten
    )

    if ($problems | is-not-empty) {
        print --stderr "U+2026 (horizontal ellipsis) found in locale catalogs; write three periods instead:"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"Locale ellipsis spelling OK across ($files | length) catalogs."
}
