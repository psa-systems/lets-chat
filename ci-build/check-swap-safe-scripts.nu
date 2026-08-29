#!/usr/bin/env nu

# Guard re-entrant inline page scripts (LC-835).
#
# Today every in-app navigation is a full page load, so each inline <script>
# runs exactly once and none of them has to be re-entrant. LC-837 swaps
# <main id="main"> instead, and from then on every inline script inside the
# swapped region re-runs on every navigation. A script that registers on a host
# the swap does not replace - document, document.body, window, a repeating
# timer, an observer - stacks a second registration each time. Nothing throws:
# the handler just fires twice, the interval just ticks twice as often. An
# unaudited script is therefore indistinguishable from a safe one, which is why
# the declaration below is mandatory rather than inferred.
#
# Every inline script in a template that can render more than once into a live
# document declares how it stays safe:
#
#   data-lc-guard="none"      registers nothing on a host that outlives the
#                             swap; it only binds nodes inside the swapped
#                             region, which the swap replaces wholesale.
#   data-lc-guard="flag"      a window.__lc<Name> test-and-set makes the
#                             surviving-host registration happen at most once.
#   data-lc-guard="teardown"  the registration is paired with an explicit
#                             removal (removeEventListener / clearInterval /
#                             observer.disconnect), usually on
#                             htmx:beforeCleanupElement.
#
# The teeth are in the mismatch: "none" must contain zero surviving-host
# registrations, so mislabelling a script that touches document fails here.
#
# SHELL below is the boundary, stated rather than implied: those templates
# render exactly once per page load and are never swapped or re-rendered into a
# live document, so their scripts cannot re-run and need no declaration.
# See docs/ui-conventions.md, "Inline scripts must survive a swap".

# Templates whose inline scripts run exactly once per page load.
#   base.html / layout.html   the shell itself; LC-837 targets #main, so
#                             everything outside <main> is never swapped.
#   partials/theme_bootstrap  a <head> before-paint block, included by the shell.
#   *_modal.html              included by layout.html AFTER </main> (line 270+),
#                             so they sit outside the swap target and are
#                             rendered once with the shell.
const SHELL = [
    "server/templates/base.html"
    "server/templates/layout.html"
    "server/templates/partials/theme_bootstrap.html"
    "server/templates/add_room_modal.html"
    "server/templates/event_modal.html"
    "server/templates/gif_modal.html"
    "server/templates/lightbox_modal.html"
    "server/templates/poll_modal.html"
    "server/templates/scheduled_modal.html"
    "server/templates/shortcuts_modal.html"
    "server/templates/switcher_modal.html"
]

const VALUES = ["none" "flag" "teardown"]

# A registration whose host outlives the swap. setTimeout is deliberately absent:
# it fires once and cannot stack.
const REG_SHAPES = '(?<call>document\.addEventListener|document\.body\.addEventListener|document\.documentElement\.addEventListener|window\.addEventListener|setInterval|new MutationObserver|new IntersectionObserver|new ResizeObserver|new PerformanceObserver|customElements\.define|htmx\.onLoad)\s*\('
const FLAG_TEST = 'if\s*\(\s*!?\s*window\.(?<flag>__lc\w+)\s*\)'
const FLAG_SET = 'window\.(?<flag>__lc\w+)\s*=\s*true'
const REMOVE_SHAPES = '(removeEventListener\s*\(|clearInterval\s*\(|\.disconnect\s*\()'

# The one rule engine, shared by the repo scan and the self-test. Returns the
# failure reason, or "" when the block is compliant.
def verdict [guard: string, body: string]: nothing -> string {
    if $guard == "" {
        return 'no data-lc-guard on an inline <script>; declare none|flag|teardown'
    }
    if not ($guard in $VALUES) {
        return $'data-lc-guard="($guard)" is not one of none|flag|teardown'
    }
    let regs = ($body | parse --regex $REG_SHAPES | length)
    if $guard == "none" {
        if $regs > 0 {
            return 'declared none but registers on a host that outlives the swap; use flag or teardown'
        }
        return ""
    }
    if $regs == 0 {
        return $'declared ($guard) but registers nothing on a surviving host; declare none'
    }
    if $guard == "flag" {
        let tested = ($body | parse --regex $FLAG_TEST | each {|r| $r.flag })
        let assigned = ($body | parse --regex $FLAG_SET | each {|r| $r.flag })
        let paired = ($tested | where {|f| $f in $assigned })
        if ($paired | is-empty) {
            return 'declared flag but carries no window.__lc<Name> test-and-set pair'
        }
        return ""
    }
    if not ($body =~ $REMOVE_SHAPES) {
        return 'declared teardown but never removes what it registered'
    }
    ""
}

# Every inline <script> block in one file: its line, its data-lc-guard value
# (empty when absent) and its body. External <script src=...> is reported
# separately.
def blocks [file: string]: nothing -> table {
    mut out = []
    mut open_at = 0
    mut guard = ""
    mut body = ""
    mut inside = false
    for row in (open --raw $file | lines | enumerate) {
        let l = $row.item
        let n = $row.index + 1
        if $inside {
            if ($l =~ '</script>') {
                $out = ($out | append {file: $file, line: $open_at, guard: $guard, body: $body})
                $inside = false
                $body = ""
            } else {
                $body = $"($body)\n($l)"
            }
            continue
        }
        if not ($l =~ '<script') { continue }
        if ($l =~ '<script[^>]*\bsrc=') { continue }
        if not ($l =~ '<script[^>]*>') {
            print --stderr $"($file):($n): the opening <script> tag must fit on one line so this guard can read its data-lc-guard."
            exit 1
        }
        if ($l =~ '</script>') {
            print --stderr $"($file):($n): put the inline script body on its own lines; a one-line <script>...</script> is not readable by this guard."
            exit 1
        }
        let attrs = ($l | parse --regex '<script(?<attrs>[^>]*)>' | get attrs.0)
        let declared = ($attrs | parse --regex 'data-lc-guard="(?<v>[^"]*)"' | each {|r| $r.v })
        $open_at = $n
        $guard = (if ($declared | is-empty) { "" } else { $declared | first })
        $inside = true
    }
    if $inside {
        print --stderr $"($file):($open_at): unterminated inline <script>."
        exit 1
    }
    $out
}

def run-self-test []: nothing -> nothing {
    # Known-bad and known-good fixtures for the same rule engine the repo scan
    # uses, so a guard that has stopped asserting anything fails here first.
    let cases = [
        [name, guard, body, must_fail];
        ["undeclared" "" "var a = 1;" true]
        ["bogus value" "always" "var a = 1;" true]
        ["none but binds document" "none" "document.addEventListener('click', f);" true]
        ["none but binds document.body" "none" "document.body.addEventListener('htmx:afterSettle', f);" true]
        ["none but sets an interval" "none" "setInterval(tick, 250);" true]
        ["none but observes" "none" "var o = new MutationObserver(f);" true]
        ["flag with no pair" "flag" "document.addEventListener('click', f);" true]
        ["flag tested but never set" "flag" "if (window.__lcA) return;\ndocument.addEventListener('click', f);" true]
        ["flag set but never tested" "flag" "window.__lcA = true;\ndocument.addEventListener('click', f);" true]
        ["flag names do not match" "flag" "if (window.__lcA) return;\nwindow.__lcB = true;\ndocument.addEventListener('click', f);" true]
        ["teardown with no removal" "teardown" "document.addEventListener('click', f);" true]
        ["flag guarding nothing" "flag" "if (window.__lcA) return;\nwindow.__lcA = true;\nel.addEventListener('click', f);" true]
        ["teardown guarding nothing" "teardown" "el.removeEventListener('click', f);" true]
        ["none, local binding only" "none" "el.addEventListener('click', f);\nsetTimeout(f, 10);" false]
        ["none, no binding at all" "none" "document.getElementById('x').textContent = 'hi';" false]
        ["flag, early return form" "flag" "if (window.__lcA) return;\nwindow.__lcA = true;\ndocument.addEventListener('click', f);" false]
        ["flag, negated block form" "flag" "if (!window.__lcA) {\n  window.__lcA = true;\n  document.body.addEventListener('htmx:afterSettle', f);\n}" false]
        ["teardown, removeEventListener" "teardown" "document.addEventListener('click', f);\nroot.addEventListener('htmx:beforeCleanupElement', function(){ document.removeEventListener('click', f); });" false]
        ["teardown, clearInterval" "teardown" "if (window.__lcHb) clearInterval(window.__lcHb);\nwindow.__lcHb = setInterval(ping, 3000);" false]
        ["teardown, observer disconnect" "teardown" "var o = new MutationObserver(function(){ o.disconnect(); });" false]
    ]
    mut bad = []
    for c in $cases {
        let got = (verdict $c.guard $c.body)
        let failed = ($got != "")
        if $failed != $c.must_fail {
            let want = (if $c.must_fail { "reject" } else { "accept" })
            let got_desc = (if $failed { $got } else { "no failure" })
            $bad = ($bad | append $"  ($c.name): expected the rule engine to ($want) it, got: ($got_desc)")
        }
    }
    if ($bad | is-not-empty) {
        print --stderr "Self-test failed; the inline-script rule engine does not assert what it claims:"
        for b in $bad { print --stderr $b }
        exit 1
    }
    print $"Self-test OK: ($cases | length) fixtures, ($cases | where must_fail | length) rejected, ($cases | where not must_fail | length) accepted."
}

def "main self-test" [] {
    run-self-test
}

def main [] {
    run-self-test

    for shell in $SHELL {
        if not ($shell | path exists) {
            print --stderr $"($shell) is on the shell allowlist but does not exist; re-audit the swap boundary."
            exit 1
        }
    }

    let files = (glob server/templates/**/*.html | sort)
    if ($files | is-empty) {
        print --stderr "No templates found under server/templates/"
        exit 1
    }
    # Email templates render in a mail client with no scripting, so they are out
    # of scope here as they are for every other template rule.
    let swappable = (
        $files
        | each {|f| $f | path relative-to (pwd) }
        | where {|f| ($f not-in $SHELL) and ($f !~ '^server/templates/email/') }
    )

    mut problems = []
    mut checked = 0
    for rel in $swappable {
        let file = $rel
        # An external bundle re-fetched and re-executed on every swap double-registers
        # everything it defines; the shell is the only place a <script src> belongs.
        for row in (open --raw $file | lines | enumerate) {
            if ($row.item =~ '<script[^>]*\bsrc=') {
                $problems = ($problems | append $"($rel):($row.index + 1): <script src=...> in a swappable template; load it from base.html instead.")
            }
        }
        for b in (blocks $file) {
            $checked = $checked + 1
            let why = (verdict $b.guard $b.body)
            if $why != "" {
                $problems = ($problems | append $"($rel):($b.line): ($why)")
            }
        }
    }

    if ($checked == 0) {
        print --stderr "No inline scripts found outside the shell allowlist. Either the allowlist swallowed the tree or the scanner stopped matching; either way this guard is asserting nothing."
        exit 1
    }

    if ($problems | is-not-empty) {
        print --stderr "Inline scripts in swappable templates must declare how they survive a re-run (LC-835). See docs/ui-conventions.md, \"Inline scripts must survive a swap\":"
        for p in $problems { print --stderr $"  ($p)" }
        exit 1
    }

    print $"Swap-safe inline scripts OK: ($checked) declared blocks across ($swappable | length) templates, ($SHELL | length) shell templates exempt."
}
