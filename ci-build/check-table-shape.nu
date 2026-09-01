#!/usr/bin/env nu

# Guard the shared data-table component (LC-745).
#
# A record list is `<div class="card lc-table-wrap"><table class="lc-table">`
# with bare `<th>` / `<td>`; `.lc-table` (main.css) owns the cell padding, the
# header color and the row rules. The three room integration tables hand-rolled
# that shape with `w-full text-sm` plus per-cell `py-1` / `py-2`, so a room
# integration list and a server one rendered at different row heights.
#
# Two rules, both mechanical:
#   1. Every table is `.lc-table`, inside a `.card` wrapper.
#   2. No `<th>` / `<td>` inside a `.lc-table` carries a padding utility.
#
# Email templates are excluded (inline-styled layout tables in a mail client).
# Nothing else is exempt: LC-756 moved the last two one-offs (the cohort
# retention grid and the IMAP drop log) onto the component, so every table
# under server/templates/ is checked.

# Tailwind padding utilities: p-4, px-2, py-1.5, pt-1, sm:pb-2, p-px, ...
const PADDING = '(^|["\s:])p[trblxy]?-(\d|px)'

def main [] {
    let files = (glob server/templates/**/*.html --exclude ["**/email/**"] | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }

    let problems = (
        $files | each {|file|
            let rel = ($file | path relative-to $env.PWD)
            let lines = (open --raw $file | lines)
            check-file $rel $lines
        } | flatten
    )

    if ($problems | is-not-empty) {
        print --stderr "Tables that do not use the shared `.lc-table` component (see docs/ui-conventions.md):"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }
    print $"Table shape OK across ($files | length) templates."
}

# Walk one file, tracking whether the current line sits inside a `.lc-table`.
def check-file [rel: string, lines: list<string>]: nothing -> list<string> {
    (
        $lines | enumerate
        | reduce --fold {in_lc: false, problems: []} {|row, acc|
            let line = $row.item
            let at = $"($rel):($row.index + 1)"

            let opens = ($line =~ '<table')
            let is_lc = ($opens and ($line =~ '<table[^>]*\blc-table\b'))
            let in_lc = if $opens { $is_lc } else if ($line =~ '</table>') { false } else { $acc.in_lc }

            # A non-`.lc-table` table, or an `.lc-table` with no `.card` wrapper.
            let table_problem = if not $opens { null } else if not $is_lc {
                $"($at): table is not `lc-table`"
            } else if not (wrapped-in-card $lines $row.index) {
                $"($at): `lc-table` is not inside a `.card` wrapper"
            } else { null }

            # A padding utility on a cell inside an `.lc-table`.
            let cell_problems = if not $in_lc { [] } else {
                $line | parse --regex '<t[hd](?<attrs>[^>]*)>' | get attrs
                | where {|a| $a =~ $PADDING }
                | each {|a| $"($at): cell carries a padding utility:($a)" }
            }

            let found = ([$table_problem] | compact | append $cell_problems)
            {in_lc: $in_lc, problems: ($acc.problems | append $found)}
        }
        | get problems
    )
}

# The wrapper is either on the same line ahead of the tag, or on the nearest
# non-blank line above it (same rule as check-table-scroll.nu).
def wrapped-in-card [lines: list<string>, index: int]: nothing -> bool {
    let ahead = ($lines | get $index | split row '<table' | first)
    let above = (
        $lines | first $index | reverse
        | where {|line| ($line | str trim) != "" }
    )
    let prev = (if ($above | is-empty) { "" } else { $above | first })
    ($ahead =~ '\bcard\b') or ($prev =~ '\bcard\b')
}
