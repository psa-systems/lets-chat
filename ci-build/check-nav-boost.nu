#!/usr/bin/env nu

# LC-837: boosted navigation is per anchor, and every anchor in the persistent
# shell says which side it is on.
#
# Links in the nav panel (enclave switcher, sidebar, account menu) carry the
# `partials/nav_boost.html` include, which boosts them targeting #main so a page
# move keeps the socket, the sidebar and every running script. A link that must
# stay a real navigation carries `hx-boost="false"`. An anchor with neither is
# the bug this guard exists for: it silently reloads the page, which drops a
# live call (LC-832) and cycles the socket, and nothing in the UI says so.
#
# Boost is never allowed on a container: `hx-select` and `hx-swap` inherit, and
# the sidebar's own hx-post / hx-get controls (mark read, star, category moves,
# search) would inherit them and blank themselves. So `hx-boost="true"` may
# appear in exactly one template, the include itself.
#
# Carries a self-test over its own rule so a hollowed-out rule fails before a
# single template is read (the LC-835 pattern).

const NAV_PARTIALS = [
    "server/templates/partials/enclave_switcher.html"
    "server/templates/partials/sidebar.html"
    "server/templates/partials/sidebar_self.html"
    "server/templates/partials/sidebar_nav.html"
    "server/templates/partials/sidebar_room_row.html"
    "server/templates/partials/sidebar_peer_row.html"
]
const PARTIAL = "server/templates/partials/nav_boost.html"
const LAYOUT = "server/templates/layout.html"
const BOOST_INCLUDE = '{% include "partials/nav_boost.html" %}'
const OPT_OUT = 'hx-boost="false"'
const BOOST_ON = 'hx-boost="true"'
const REQUIRED_ATTRS = ['hx-boost="true"' 'hx-target="#main"' 'hx-select="#main"' 'hx-swap="outerHTML"']

# Every `<a ...>` opening tag in `text` (tags may span lines) that has an href
# and carries neither the include nor the opt-out.
def unboosted-anchors [text: string]: nothing -> list<string> {
    $text
    | parse --regex '(?s)(?<tag><a\b[^>]*>)'
    | get tag
    | where {|tag|
        # One line: nushell reads a line-leading `and` as a command name.
        ($tag | str contains 'href=') and (not ($tag | str contains $BOOST_INCLUDE)) and (not ($tag | str contains $OPT_OUT))
    }
}

def self-test [] {
    let cases = [
        [text expect];
        ['<a href="/x" {% include "partials/nav_boost.html" %} class="y">go</a>' 0]
        ['<a href="/logout" hx-boost="false" class="y">out</a>' 0]
        ['<a href="/x" class="y">go</a>' 1]
        ["<a href=\"/x\"\n   class=\"y\"\n   data-z>go</a>" 1]
        ['<abbr title="x">a</abbr> <a href="/y" hx-boost="false">b</a>' 0]
        ['<a href="/{{ room.id }}">a</a><a href="/b" {% include "partials/nav_boost.html" %}>b</a>' 1]
        ['<a id="anchor-without-href">a</a>' 0]
    ]
    for case in $cases {
        let got = (unboosted-anchors $case.text | length)
        if $got != $case.expect {
            print --stderr $"self-test failed: expected ($case.expect) unboosted anchor\(s\), got ($got) for: ($case.text)"
            exit 1
        }
    }
}

def main [] {
    self-test

    mut failures: list<string> = []

    # 1. Every nav-panel anchor declares itself.
    mut boosted_total = 0
    for file in $NAV_PARTIALS {
        if not ($file | path exists) {
            $failures = ($failures | append $"($file): listed as a nav partial but missing; update NAV_PARTIALS")
            continue
        }
        let text = (open --raw $file | decode utf-8)
        for tag in (unboosted-anchors $text) {
            let first = ($tag | lines | first | str trim)
            $failures = ($failures | append $"($file): anchor without the nav_boost include or hx-boost=\"false\": ($first)")
        }
        $boosted_total = $boosted_total + ($text | str replace --all $BOOST_INCLUDE "\u{1}" | split chars | where {|c| $c == "\u{1}" } | length)
    }
    if $boosted_total == 0 {
        $failures = ($failures | append "no nav-panel anchor carries the nav_boost include; the guard is scanning nothing")
    }

    # 2. The include is what it says it is.
    let partial = (open --raw $PARTIAL | decode utf-8)
    for attr in $REQUIRED_ATTRS {
        if not ($partial | str contains $attr) {
            $failures = ($failures | append $"($PARTIAL): missing ($attr)")
        }
    }

    # 3. Boost lives in the include only, never on a container or a page.
    let partial_abs = ($PARTIAL | path expand)
    for file in (glob server/templates/**/*.html | where {|f| $f != $partial_abs } | sort) {
        let text = (open --raw $file | decode utf-8)
        if ($text | str contains $BOOST_ON) {
            let rel = ($file | path relative-to (pwd))
            $failures = ($failures | append $"($rel): hx-boost=\"true\" outside partials/nav_boost.html; boost is per anchor via the include, never on a container")
        }
    }

    # 4. #main is the history element, so back/forward restore only #main and
    #    leave the ws-connect wrapper alone.
    let layout = (open --raw $LAYOUT | decode utf-8)
    if not ($layout =~ '<main id="main" hx-history-elt\b') {
        $failures = ($failures | append $"($LAYOUT): <main id=\"main\"> must carry hx-history-elt")
    }

    if ($failures | is-empty) {
        print $"nav boost OK: ($boosted_total) boosted anchors across ($NAV_PARTIALS | length) nav partials, boost confined to ($PARTIAL)."
    } else {
        for f in $failures { print --stderr $"FAIL ($f)" }
        print --stderr $"($failures | length) nav boost failure\(s\)."
        exit 1
    }
}
