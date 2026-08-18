#!/usr/bin/env nu

# Guard the one tablist controller (LC-747).
#
# The settings, enclave-settings and room-info pages share one markup contract
# ([data-lc-tab] triggers, [data-lc-tabpanel] panels) and used to carry three
# byte-for-byte copies of the controller that drives it. They had already
# drifted: `settings.js` reset the visible panel on ANY hashchange, so following
# an in-page anchor whose hash was not a tab name silently jumped the user to a
# different panel, while the other two left the panel alone.
#
# So: only assets/tabs.js may touch the contract, every consumer goes through
# window.lcInitTabs, and the shared file is both loaded by base.html and in the
# service-worker precache list.

const CONTROLLER = "server/assets/tabs.js"
const CONSUMERS = [
    "server/assets/settings.js"
    "server/assets/enclave_settings.js"
    "server/assets/roominfo.js"
]

def main [] {
    if not ($CONTROLLER | path exists) {
        print --stderr $"Missing ($CONTROLLER): the shared tablist controller."
        exit 1
    }

    let strays = (
        glob server/assets/**/*.js
        | where {|f| ($f | path basename) != "tabs.js" and ($f !~ '/vendor/') }
        | sort
        | each {|file|
            open --raw $file
            | lines
            | enumerate
            | where {|row| $row.item =~ 'data-lc-tab' }
            | each {|row| $"($file):($row.index + 1): ($row.item | str trim)" }
        }
        | flatten
    )
    if ($strays | is-not-empty) {
        print --stderr $"Tablist markup touched outside ($CONTROLLER); call window.lcInitTabs instead:"
        for s in $strays { print --stderr $"  ($s)" }
        exit 1
    }

    # The drift that motivated the extraction: one copy re-ran its whole
    # fallback chain on every hashchange, so an in-page anchor whose hash is not
    # a tab key jumped the user to the remembered or first panel.
    if not ((open --raw $CONTROLLER) =~ "(?s)addEventListener\\('hashchange'.{0,400}if \\(valid\\(h\\)\\) select\\(") {
        print --stderr $"($CONTROLLER): a hashchange that does not name a tab must leave the panel alone."
        print --stderr "  Guard the select with an 'if (valid(h))' test."
        exit 1
    }

    for consumer in $CONSUMERS {
        if not ((open --raw $consumer) =~ 'window\.lcInitTabs\(') {
            print --stderr $"($consumer) has tabs but never calls the shared window.lcInitTabs."
            exit 1
        }
    }

    if not ((open --raw server/templates/base.html) =~ '/assets/tabs\.js') {
        print --stderr "base.html does not load /assets/tabs.js; its consumers would find no window.lcInitTabs."
        exit 1
    }
    if not ((open --raw server/assets/sw.js) =~ '/assets/tabs\.js') {
        print --stderr "server/assets/sw.js does not precache /assets/tabs.js."
        exit 1
    }

    print $"Single tab controller OK; ($CONSUMERS | length) consumers call window.lcInitTabs."
}
