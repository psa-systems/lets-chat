#!/usr/bin/env nu

# Guard the horizontal scroll container around every table (LC-737).
#
# The 13 admin list pages wrapped `.lc-table` in `.card.overflow-hidden`, which
# clips an overflowing table instead of scrolling it: nothing upstream provides
# horizontal scroll, so at 375px the trailing columns (including the nowrap
# actions cell) were unreachable, with no scrollbar and no drag. The wrapper is
# now `.lc-table-wrap` (`overflow-x: auto`, main.css), which is what the two
# one-off admin tables already did with `overflow-x-auto`.
#
# Email templates are excluded: they are layout tables in a mail client, with
# inline styles and no stylesheet.

const WRAPPERS = 'lc-table-wrap|overflow-x-auto'

def main [] {
    let files = (glob server/templates/**/*.html --exclude ["**/email/**"] | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }

    let problems = (
        $files | each {|file|
            let lines = (open --raw $file | lines)
            $lines
            | enumerate
            | where {|row| $row.item =~ '<table' }
            | where {|row|
                # The wrapper is either on the same line ahead of the tag, or on
                # the nearest non-blank line above it.
                let ahead = ($row.item | split row '<table' | first)
                let above = (
                    $lines | first $row.index | reverse
                    | where {|line| ($line | str trim) != "" }
                )
                let prev = (if ($above | is-empty) { "" } else { $above | first })
                not (($ahead =~ $WRAPPERS) or ($prev =~ $WRAPPERS))
            }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        } | flatten
    )

    if ($problems | is-not-empty) {
        print --stderr "Tables without a horizontal scroll wrapper; wrap them in a `.lc-table-wrap` div (admin list pages: `<div class=\"card lc-table-wrap\">`):"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"Table scroll wrappers OK across ($files | length) templates."
}
