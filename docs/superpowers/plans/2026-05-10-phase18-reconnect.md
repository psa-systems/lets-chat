# Phase 18 - Connection-Lost UI / WebSocket Reconnect

## Goal

When the WS drops, the user currently has no signal: messages stop
arriving, mute/sidebar updates stall, the Push permission prompt timing
gets weird, and the user keeps typing into a dead pipe until they
notice. Phase 18 adds a visible reconnect indicator and an automatic
reconnect-with-backoff loop so every previous WS-dependent feature
(phases 14-17) recovers cleanly.

User-visible behaviour after this phase:

- Connected (default): no banner, no DOM at all.
- Reconnecting (yellow): "Connection lost. Reconnecting..." with a
  subtle spinner, fixed-position bar at the top of the viewport, does
  not push content.
- Failed-long (red): after ~5 minutes of continuous reconnecting, the
  banner escalates to "Still trying to reconnect. Check your internet
  connection." Same retry loop continues underneath.
- On successful reconnect, the banner briefly flashes green ("Connected")
  for ~500ms then fades out, and the current room/page is soft-refreshed
  via `htmx.ajax` so the user sees any messages they missed.

Detection is dual: the htmx ws extension's `htmx:wsClose` /
`htmx:wsError` events catch fast-path failures; an application-level
heartbeat (server emits a `<!-- ping -->` text frame every 30s; client
resets a 60s watchdog on any inbound message) catches half-open
connections that the browser hasn't noticed (sleep/wake, NAT timeout,
flaky mobile networks).

Out of scope (deferred):

- **Event replay / per-connection cursor.** When the WS is back, we
  soft-refresh the current page; we do not replay individual events the
  client missed during the outage. Significant new infrastructure
  (per-connection cursor table, replay query path); own future phase.
- **Offline message-queueing.** User typing during a disconnect: the
  composer still POSTs over HTTP (separate from WS), so a single
  message will still send while disconnected if HTTP works. We do not
  add a queue for the case where HTTP also fails.
- **Service-worker-coordinated reconnect.** The phase 16 SW handles
  Push only; we do not move WS lifecycle into the SW.
- **Per-tab WS deduplication.** Multi-tab users continue to get one WS
  per tab.
- **Persistent client-side state recovery across full page reloads.**
  The soft-refresh path covers in-place reconnect; full page reload is
  the user's escape hatch and unchanged.

## Architecture

- **Stack** (current truth): Axum 0.8 + Askama + HTMX with the
  `htmx-ext-ws` extension wired declaratively in `templates/layout.html`
  via `<div hx-ext="ws" ws-connect="/ws">`. WS payloads are
  pre-rendered HTML fragments tagged with `hx-swap-oob`; never JSON
  (heartbeat is an HTML comment fragment, see below).

- **Keep `htmx-ext-ws`, do not replace it.** A surprising amount of
  this phase's design is shaped by the extension's behaviour:

  1. Inbound message processing (parse the text frame as an HTML
     fragment, walk children, call `oobSwap` on each) uses extension-
     internal API (`api.oobSwap`, `api.makeFragment`,
     `api.makeSettleInfo`, `api.settleImmediately`) that is only
     reachable from inside an htmx extension. Replacing the extension
     would mean reimplementing this loop. Not worth it.
  2. Per-room JS in `templates/room/page.html` and
     `templates/dm/page.html` already hangs `subscribe` frames off the
     `htmx:wsOpen` event, so room subscriptions automatically re-fire
     on every reconnect. We get this for free if we keep the
     extension.
  3. The extension already auto-reconnects on close codes 1006
     (abnormal closure), 1012 (service restart) and 1013 (try again
     later) - the codes browsers emit for the disconnect cases users
     actually hit. We extend it to cover the remaining cases (clean
     closes, our own forced closes from the half-open watchdog) rather
     than fight it.

- **Reconnect-delay override.** The extension's default
  `htmx.config.wsReconnectDelay = 'full-jitter'` is unbounded:
  `1000 * Math.pow(2, exp) * Math.random()` with no cap before
  randomization, which crosses 60s after ~6 retries and exceeds 17min
  after ~10. The brief calls for `1, 2, 4, 8, 16, 30, 30, 30...`
  seconds with +/-20% jitter, capped at 30s forever. We override
  `htmx.config.wsReconnectDelay` with a custom function that
  implements the capped curve. One block in the layout-level IIFE.

- **Forced reconnect for non-1006/1012/1013 closes.** When the
  half-open watchdog fires, we force-close the underlying WebSocket.
  The browser does not allow user code to set close code 1006, so the
  resulting `onclose` carries a code (1000 or 4xxx) that the
  extension's `onclose` handler will NOT auto-reconnect on. We add a
  small dedicated htmx extension `lc-ws-reconnect` that:
  - Captures the extension API on `init` (gains access to
    `api.getInternalData(socketElt).webSocket`, the wrapper that
    exposes the private `init()` method).
  - Exposes `window.__lcReconnectWS()`, which calls
    `wrapper.init()` to re-establish the connection regardless of
    close code.
  This is the only way to access `wrapper.init()` from outside the
  extension (`wrapper.publicInterface` exposes only `send`,
  `sendImmediately` and `queue`). The extension is ~10 lines, lives
  in the same layout-level `<script>` block, and is mounted by adding
  `lc-ws-reconnect` to the body's `hx-ext`.

- **Half-open detection via inbound traffic.** Server emits a
  `<!-- ping -->` HTML comment fragment as a `Message::Text` frame
  every 30s on every connection (replaces the existing
  `Message::Ping` protocol-level ping at server/src/routes/ws.rs:189-
  193 - the protocol ping is invisible to JS and gives us nothing
  here that a text frame doesn't already give). The
  `htmx-ext-ws` extension's `oobSwap` loop iterates
  `fragment.children`; HTML comments are not children, so the
  fragment renders as a no-op. The client listens for
  `htmx:wsAfterMessage` (fires after every inbound text frame is
  processed) and resets a 60s watchdog timer. On timeout, the
  watchdog calls `window.__lcReconnectWS()`. This catches
  half-open cases the browser has not yet noticed.

  Decision over the brief's "do both protocol-level ping and a text
  ping": just one text ping. Cheaper, simpler, equivalent for keeping
  NAT alive. The protocol ping is removed at the same line in
  `routes/ws.rs`.

- **Soft refresh on reconnect.** When `htmx:wsOpen` fires AND the
  connection was previously in `reconnecting` (or `failed-long`)
  state, we trigger
  `htmx.ajax('GET', location.pathname, { target: '#main', swap: 'outerHTML', select: '#main' })`.
  Without `select`, the response is the full page (layout + main),
  and swapping it into `#main` would nest the layout. With
  `select: '#main'`, htmx extracts just the `<main id="main">`
  element from the response (per the htmx 1.x docs for `hx-select`,
  which `htmx.ajax` accepts via the config object). The result: the
  user sees a fresh render of the current room/page (any missed
  messages, mute changes, sidebar deltas) without nested layout.

  We do NOT include the `#lc-nav-panel` (sidebar) in the soft-refresh
  target. Sidebar updates are already driven by WS events and will
  arrive from now on; refreshing the sidebar separately would be an
  extra round-trip with little payoff. The same room subscription
  re-fires on `htmx:wsOpen` (per-page handlers in `room/page.html`
  and `dm/page.html` already do this), so live updates resume
  immediately.

  Listener-accumulation note: the per-page `<script>` that attaches
  the `htmx:wsOpen` `subscribe` handler runs again on every
  soft-refresh (htmx executes inline scripts on swap), accumulating
  one extra `document.body` listener per reconnect. Same pre-existing
  pattern as ordinary page navigation. The extra subscribes are
  idempotent server-side. Out of scope to fix here.

- **Banner partial.** New `templates/partials/connection_status.html`,
  included once from `templates/layout.html` near the existing
  `#lc-notify-bus` element. The element is `<div
  id="lc-conn-status" data-state="hidden">` with three states keyed
  off `data-state`: `hidden`, `reconnecting`, `failed-long`,
  `connected-flash`. The CSS lives inline in the partial (Tailwind
  classes via `data-state` selector + a tiny `<style>` block for the
  fade and pulse, since Tailwind's data-attribute variant config is
  not enabled in this codebase). Fixed-position at the top
  (`fixed top-0 inset-x-0 z-50 transition-opacity duration-500`) so
  it does not push content.

- **JS state machine.** All in one IIFE in `layout.html` (extends the
  existing notification-bus block from phases 14/16). State variables:
  `state` ('connected' | 'reconnecting' | 'failed-long'), `socket`
  (raw WebSocket reference, captured from
  `evt.detail.event.target` in `htmx:wsOpen`), `watchdog` (timeout
  handle, reset on every `htmx:wsAfterMessage`), `escalateTimer`
  (timeout handle, started when entering `reconnecting`, fires after
  5 minutes to switch to `failed-long`). On `htmx:wsOpen` if previous
  state was reconnecting: trigger soft refresh, flash banner to
  `connected-flash` for 500ms then back to `hidden`. On
  `htmx:wsClose` or `htmx:wsError`: transition to `reconnecting`,
  start escalate timer. If the underlying close code is not in
  [1006,1012,1013] (the htmx extension will not auto-reconnect),
  call `window.__lcReconnectWS()` to manually reconnect.

  Total JS budget for the new logic (override + extension definition
  + state machine + watchdog + soft refresh): ~80 lines. Existing
  notification-bus IIFE stays as-is; the new code is a sibling block.

## Tech Stack

- New crates: none.
- New static assets: none. Vendored htmx and `htmx-ext-ws` unchanged.
- New migrations: none.
- No new build steps; pure Askama + Tailwind classes already in the
  built stylesheet, plus a tiny `<style>` block for fade/pulse
  keyframes inline in the banner partial.

## File Map

| Action | Path | Responsibility |
|---|---|---|
| Edit | `server/src/routes/ws.rs` | Replace the 30s `Message::Ping` tick with a 30s `Message::Text("<!-- ping -->")` tick. One-line change inside `tokio::select!`. |
| Add  | `server/templates/partials/connection_status.html` | Fixed-position banner element with `data-state`-driven visual states (`hidden`, `reconnecting`, `failed-long`, `connected-flash`) and inline `<style>` for fade/spinner. |
| Edit | `server/templates/layout.html` | (1) Include the new banner partial just inside `<div hx-ext="ws" ...>`. (2) Add `lc-ws-reconnect` to a new `hx-ext` on the body (or alongside `response-targets` on `<body>` in `base.html`). (3) Append a new `<script>` IIFE for the reconnect/banner state machine, sibling to the existing notification-bus IIFE. |
| Edit | `server/templates/base.html` | Add `lc-ws-reconnect` to the body's `hx-ext` attribute (currently `response-targets` only). |
| Add  | `server/tests/routes_reconnect.rs` | (1) Banner partial renders correctly in each of its three "fixed" states (Askama struct call, assert HTML contents). (2) (Optional) Server-side: verify the `Message::Text("<!-- ping -->")` frame is emitted on schedule when an integration test harness for WS exists; if not, smoke-test by verifying the relevant tokio interval branch compiles + runs the right `tx.send` call. |

No changes to: `server/src/state.rs`, the hub, any DB migrations, the
desktop crate, or the Push/SW machinery.

## Tasks

### Task 1 - Server: replace protocol ping with HTML-comment text ping

- [ ] Edit `server/src/routes/ws.rs`. Inside `handle_socket`, the
      `tokio::select!` block at lines 80-194 has a 30s ping interval
      arm at lines 189-193:

      ```rust
      _ = ping.tick() => {
          if tx.send(Message::Ping(Vec::new().into())).await.is_err() {
              break;
          }
      }
      ```

      Change to:

      ```rust
      _ = ping.tick() => {
          // Send an HTML-comment text frame as the heartbeat. The
          // client uses any inbound `htmx:wsAfterMessage` to reset its
          // half-open watchdog. Comments are not `fragment.children`
          // for `htmx-ext-ws`, so the swap path renders a no-op.
          if tx.send(Message::Text("<!-- ping -->".into())).await.is_err() {
              break;
          }
      }
      ```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo check -p lets-chat-server --no-default-features --features saas`
- [ ] `./dev/cargo test -p lets-chat-server` (existing WS-related tests
      should still pass; protocol-level ping was not asserted on by any
      test).
- [ ] `git checkout -b feat/reconnect-ui`
- [ ] `git add server/src/routes/ws.rs`
- [ ] Commit:

      ```
      refactor(ws): emit text-frame heartbeat in place of protocol ping

      Browsers handle protocol-level WebSocket pings transparently and
      give the JS no signal. A 30s `<!-- ping -->` text frame keeps
      NAT/proxies alive and gives the client a visible "still alive"
      event via `htmx:wsAfterMessage`, which the upcoming reconnect UI
      uses to detect half-open connections.
      ```

### Task 2 - Banner partial

- [ ] Create `server/templates/partials/connection_status.html`:

      ```html
      <div id="lc-conn-status" data-state="hidden"
           class="fixed top-0 inset-x-0 z-50 pointer-events-none
                  transition-opacity duration-500">
        <div class="mx-auto max-w-2xl mt-2 rounded-md px-3 py-1.5
                    text-sm text-center shadow-md
                    flex items-center justify-center gap-2"
             id="lc-conn-status-pill">
          <span id="lc-conn-status-spinner" class="hidden">
            <svg class="h-3.5 w-3.5 animate-spin" viewBox="0 0 24 24"
                 fill="none" xmlns="http://www.w3.org/2000/svg"
                 aria-hidden="true">
              <circle cx="12" cy="12" r="10" stroke="currentColor"
                      stroke-width="3" stroke-opacity="0.25"></circle>
              <path d="M22 12a10 10 0 0 1-10 10" stroke="currentColor"
                    stroke-width="3" stroke-linecap="round"></path>
            </svg>
          </span>
          <span id="lc-conn-status-text"></span>
        </div>
      </div>
      <style>
      /* data-state-driven visuals. Tailwind's data-attribute variant is
         not configured in this project; a small <style> block keeps the
         partial self-contained without touching tailwind.config.js. */
      #lc-conn-status[data-state="hidden"]            { opacity: 0; }
      #lc-conn-status[data-state="reconnecting"]      { opacity: 1; }
      #lc-conn-status[data-state="failed-long"]       { opacity: 1; }
      #lc-conn-status[data-state="connected-flash"]   { opacity: 1; }
      #lc-conn-status[data-state="reconnecting"]    #lc-conn-status-pill { background-color: #fef3c7; color: #78350f; }
      #lc-conn-status[data-state="failed-long"]     #lc-conn-status-pill { background-color: #fee2e2; color: #7f1d1d; }
      #lc-conn-status[data-state="connected-flash"] #lc-conn-status-pill { background-color: #dcfce7; color: #14532d; }
      #lc-conn-status[data-state="reconnecting"]    #lc-conn-status-spinner,
      #lc-conn-status[data-state="failed-long"]     #lc-conn-status-spinner { display: inline-flex; }
      </style>
      ```

      The element starts in `data-state="hidden"` so a connected client
      sees nothing. The text content is driven by the JS state
      machine (Task 4) since the three messages differ. The
      `pointer-events-none` on the wrapper means the banner never
      captures clicks even when visible.

- [ ] `./dev/cargo check -p lets-chat-server` (Askama template
      compilation only).
- [ ] `git add server/templates/partials/connection_status.html`
- [ ] Commit:

      ```
      feat(reconnect): add connection-status banner partial
      ```

### Task 3 - Mount the banner + register the lc-ws-reconnect extension

- [ ] Edit `server/templates/layout.html`. Just inside the
      `<div hx-ext="ws" ws-connect="/ws" class="flex h-screen">`
      opening tag, BEFORE the `#lc-nav-panel` line (so the banner is
      rendered as the first child of the WS-root element), add:

      ```html
      {% include "partials/connection_status.html" %}
      ```

- [ ] Edit `server/templates/base.html`. Change:

      ```html
      <body hx-ext="response-targets" class="h-screen overflow-hidden bg-slate-50 text-slate-900">
      ```

      to:

      ```html
      <body hx-ext="response-targets,lc-ws-reconnect" class="h-screen overflow-hidden bg-slate-50 text-slate-900">
      ```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `git add server/templates/layout.html server/templates/base.html`
- [ ] Commit:

      ```
      feat(reconnect): mount connection-status banner and reserve hx-ext slot
      ```

### Task 4 - Reconnect/banner state-machine IIFE

- [ ] Edit `server/templates/layout.html`. Append a new `<script>`
      block at the end of the existing `{% block body %}` (after the
      existing `lcOpenNav`/`lcCloseNav` script). It defines the
      `lc-ws-reconnect` htmx extension AND the state machine in a
      single IIFE. ~80 lines, target inline:

      ```html
      <script>
      (function(){
        // ----- 1. Reconnect-delay override -----------------------------
        // Capped jittered exponential backoff: 1s, 2s, 4s, 8s, 16s, 30s,
        // 30s, 30s... +/-20% jitter. Replaces the extension's default
        // `full-jitter` which is uncapped (exceeds 30s after ~6 retries
        // and 17min after ~10).
        if (window.htmx) {
          htmx.config.wsReconnectDelay = function(retryCount) {
            var base = Math.min(1000 * Math.pow(2, retryCount), 30000);
            var jitter = 0.8 + Math.random() * 0.4; // [0.8, 1.2)
            return Math.round(base * jitter);
          };
        }

        // ----- 2. Tiny extension to expose wrapper.init() --------------
        // wrapper.publicInterface exposes only send/sendImmediately/
        // queue. To force a reconnect from the half-open watchdog we
        // need wrapper.init(), which is reachable only via the extension
        // API's getInternalData on the [ws-connect] element.
        // FUTURE: if htmx-ext-ws changes wrapper API (specifically
        // wrapper.init), this extension breaks. Failure mode: watchdog
        // disabled, falls back to onclose-only detection.
        if (window.htmx && htmx.defineExtension) {
          htmx.defineExtension('lc-ws-reconnect', {
            init: function(api) {
              window.__lcReconnectWS = function() {
                var elt = document.querySelector('[ws-connect]');
                if (!elt) return;
                var data = api.getInternalData(elt);
                if (data && data.webSocket && data.webSocket.init) {
                  data.webSocket.init();
                }
              };
            }
          });
        }

        // ----- 3. Banner state machine ---------------------------------
        var bannerEl = document.getElementById('lc-conn-status');
        var textEl   = document.getElementById('lc-conn-status-text');
        if (!bannerEl || !textEl) return;

        var state = 'connected';      // 'connected' | 'reconnecting' | 'failed-long'
        var rawSocket = null;          // captured from htmx:wsOpen
        var watchdog = null;           // 60s half-open detector
        var escalateTimer = null;      // 5min "still trying" escalation
        var WATCHDOG_MS  = 60 * 1000;
        var ESCALATE_MS  = 5 * 60 * 1000;

        function setBanner(next, msg) {
          bannerEl.setAttribute('data-state', next);
          textEl.textContent = msg || '';
        }

        function startWatchdog() {
          clearTimeout(watchdog);
          watchdog = setTimeout(function(){
            // Half-open: no inbound traffic in WATCHDOG_MS. Force the
            // socket closed and let our htmx:wsClose handler trigger
            // the reconnect.
            try { if (rawSocket) rawSocket.close(4000, 'lc-half-open'); } catch (e) {}
          }, WATCHDOG_MS);
        }

        function enterReconnecting() {
          if (state === 'reconnecting' || state === 'failed-long') return;
          state = 'reconnecting';
          setBanner('reconnecting', 'Connection lost. Reconnecting...');
          clearTimeout(escalateTimer);
          escalateTimer = setTimeout(function(){
            if (state === 'reconnecting') {
              state = 'failed-long';
              setBanner('failed-long', 'Still trying to reconnect. Check your internet connection.');
            }
          }, ESCALATE_MS);
        }

        function enterConnected(wasReconnecting) {
          var prev = state;
          state = 'connected';
          clearTimeout(escalateTimer);
          if (wasReconnecting || prev !== 'connected') {
            setBanner('connected-flash', 'Connected');
            setTimeout(function(){
              if (state === 'connected') setBanner('hidden', '');
            }, 500);
            // Soft-refresh the current page so the user sees missed
            // content. select:'#main' extracts just the <main> from
            // the full-page response; without it we would nest the
            // layout into #main.
            if (window.htmx && document.getElementById('main')) {
              try {
                htmx.ajax('GET', location.pathname + location.search, {
                  target: '#main',
                  swap: 'outerHTML',
                  select: '#main'
                });
              } catch (e) { /* navigation away is fine */ }
            }
          } else {
            setBanner('hidden', '');
          }
        }

        document.body.addEventListener('htmx:wsConnecting', function(){
          // First-load `wsConnecting` fires before any wsOpen. Treat as
          // silent until we have a baseline; subsequent ones (during a
          // retry loop) are already covered by the wsClose handler.
        });

        document.body.addEventListener('htmx:wsOpen', function(evt){
          rawSocket = (evt.detail && evt.detail.event) ? evt.detail.event.target : null;
          var wasReconnecting = (state !== 'connected');
          enterConnected(wasReconnecting);
          startWatchdog();
        });

        document.body.addEventListener('htmx:wsAfterMessage', function(){
          // Any inbound text frame (real OOB swap or `<!-- ping -->`)
          // counts as proof of life.
          if (state !== 'connected') enterConnected(true);
          startWatchdog();
        });

        function handleDrop(evt){
          enterReconnecting();
          clearTimeout(watchdog);
          // The extension auto-reconnects only for codes
          // [1006, 1012, 1013]. For any other close code (clean close,
          // server 1011, our own 4000 from the watchdog), force a
          // manual reconnect via the extension we registered above.
          var code = evt && evt.detail && evt.detail.event && evt.detail.event.code;
          var autoReconnects = (code === 1006 || code === 1012 || code === 1013);
          if (!autoReconnects && window.__lcReconnectWS) {
            // Schedule on the next tick so the extension's own onclose
            // bookkeeping has finished first.
            setTimeout(window.__lcReconnectWS, 0);
          }
        }
        document.body.addEventListener('htmx:wsClose', handleDrop);
        document.body.addEventListener('htmx:wsError', handleDrop);
      })();
      </script>
      ```

- [ ] `./dev/cargo check -p lets-chat-server`
- [ ] `./dev/cargo check -p lets-chat-server --no-default-features --features saas`
- [ ] `git add server/templates/layout.html`
- [ ] Commit:

      ```
      feat(reconnect): add WS reconnect/banner state machine

      Drives the connection-status banner from htmx:wsOpen / wsClose /
      wsError events, with a 60s half-open watchdog reset by
      htmx:wsAfterMessage. Caps the extension's reconnect-delay at
      30s with jitter, and adds a tiny lc-ws-reconnect htmx extension
      so the watchdog can force a reconnect for close codes the
      extension would otherwise ignore. On successful reconnect after
      a drop, soft-refreshes the current page via htmx.ajax so the
      user sees content they missed.
      ```

### Task 5 - Tests

The honest testable surface here is small. Most of the behaviour is
client-side JS lifecycle that requires a browser harness we do not
have. Be explicit about that.

- [ ] Add `server/tests/routes_reconnect.rs`. Two tests:

      1. **Banner partial renders** in its baseline (`hidden`) state.
         The partial is included from `templates/layout.html`, so we
         exercise it indirectly via any authenticated GET that renders
         the layout (e.g., `GET /`). Assert the response body contains
         `id="lc-conn-status"` and `data-state="hidden"`. This catches
         template-syntax regressions in the partial.

      2. **Soft-refresh route returns the expected `<main id="main">`
         wrapper** in its response body for an authenticated user.
         Sanity-check that `htmx.ajax(... select: '#main' ...)`
         against (e.g.) `GET /` will find a `#main` to extract. We
         assert the response contains `<main id="main"`. This catches
         a future refactor that renames or removes the `#main` id and
         silently breaks the soft-refresh path.

      Skip a server-side heartbeat-cadence test. The WS test harness
      in this repo (none of `server/tests/*` opens a real WebSocket)
      would have to be built from scratch, which is out of scope. The
      cadence is verified by the manual smoke test in Task 6 below.

      Use `axum::body::to_bytes` and the helper-style request
      construction from `server/tests/routes_dm_mute.rs` as the
      reference for the test setup.

- [ ] `./dev/cargo test -p lets-chat-server`
- [ ] `git add server/tests/routes_reconnect.rs`
- [ ] Commit:

      ```
      test(reconnect): cover banner partial rendering and soft-refresh target
      ```

### Task 6 - Final verification

- [ ] `just check`        # both modes + clippy + fmt
- [ ] `just test`         # standalone tests
- [ ] `just test-saas`    # saas tests (the changed code is mode-agnostic)
- [ ] `just verify`       # release build + GET /login smoke

- [ ] **Manual smoke list** (run against `just dev-web-local`,
      with the browser DevTools Network tab open). For each, note
      down what you saw; the goal is to flag any case where the
      banner UX is wrong or the reconnect does not resume:

      1. **Hard server restart.** `just dev-web-down` to stop the
         backend, wait ~2s, restart with `just dev-web-local`. Banner
         should appear within ~1s of the WS close, retry with
         backoff, then briefly flash green and fade out. The current
         room should soft-refresh so any messages sent from another
         tab during the outage appear.
      2. **DevTools Offline toggle on -> off.** Banner appears
         immediately on Offline, escalates to "Still trying..." after
         5 minutes if you wait that long, recovers when toggled back.
      3. **Heartbeat-only failure (half-open).** Easiest to test by
         temporarily commenting out the heartbeat tick in
         `routes/ws.rs` and connecting fresh. Within 60s the watchdog
         should fire, force-close, and reconnect. Restore the line
         after.
      4. **Multi-tab cross-check.** Two tabs of the same user, drop
         one tab's WS; only that tab's banner should appear; the
         other tab is unaffected. After reconnect, sending a message
         from tab B should appear in tab A through the normal WS
         path.
      5. **Subscribe re-fires on reconnect.** Open a room, drop the
         WS, reconnect, then send a message from another user. The
         message should arrive live (proves `subscribe` re-fired on
         the per-page `htmx:wsOpen` handler).
      6. **No banner on a healthy load.** Fresh page load with the
         server up: the banner element exists but
         `data-state="hidden"` and is invisible. No console errors.
      7. **Soft refresh - load-bearing or redundant?** This is the
         decision point. The per-page `htmx:wsOpen` handler in
         `room/page.html` and `dm/page.html` already re-fires the
         `subscribe` frame on every reconnect, which means future
         events for the room arrive normally. The open question is
         whether `htmx.ajax(GET pathname, select '#main')` adds
         anything beyond that. Concrete check: from a second user,
         send a message into the open room WHILE the first user's WS
         is dropped. After reconnect:
         (a) without soft-refresh, does the missed message appear?
             It will not - subscribe re-fires deliver events from
             reconnect onwards, not retroactively.
         (b) with soft-refresh, does it appear? Yes - the page is
             re-rendered from the DB, including the missed message.
         If (a) is acceptable for the product (users accept that
         disconnect-window content needs an explicit refresh) then
         the soft-refresh is decoration and should be removed. If (b)
         is the desired UX then the soft-refresh stays.
         Run the test both ways: with the soft-refresh code in place
         AND with it commented out. Record what each path actually
         shows. Write the conclusion in the PR description.
         If soft-refresh turns out to be redundant, follow-up commit
         `refactor(reconnect): drop redundant soft-refresh on reconnect`
         removes the `htmx.ajax` call and the `select: '#main'`
         dependency, simplifying the IIFE by ~10 lines.

      In the PR description, list any deviations actually observed.

- [ ] `git push -u origin feat/reconnect-ui`
- [ ] Open a PR titled `feat(reconnect): visible reconnect indicator
      with backoff and soft-refresh`. The PR body should:
      - Summarise the user-visible behaviour (3 banner states, soft
        refresh on recovery).
      - Call out the architectural decisions: keeping
        `htmx-ext-ws`, the `lc-ws-reconnect` companion extension, the
        delay-override, the heartbeat-as-text-frame swap.
      - Note the smoke results from the list above.

## Resolved decisions

These were the open questions during planning. Each resolved by
inspection of the current codebase before any implementation work
began; documented here so reviewers can see the basis without
re-deriving it.

1. **Removing the protocol-level `Message::Ping` is safe.** Resolved
   in favour of replacement. Self-hosted deployment uses Traefik in
   the dev environment (`compose.dev.yml`) with no infrastructure
   that special-cases WS Ping/Pong frames over text frames at the
   proxy layer. Production deployments are user-controlled
   self-hosting; the standalone binary makes no assumptions either
   way. Text-frame keep-alive is equivalent for NAT/proxy idle
   timeouts (any direction of traffic resets the timer). The
   30-byte text frame replaces a 0-byte Ping frame; cost negligible.

2. **`select: '#main'` is supported by the vendored htmx.** Resolved
   by grep on `server/assets/vendor/htmx.min.js`: `select:` appears
   as a config key consumed by the ajax path. `htmx.ajax(method,
   url, { target, swap, select })` will extract just the `#main`
   element from the full-page response, avoiding nested-layout
   issues. No server-side fallback (HX-Reconnect header or
   ?fragment=main query param) is needed.

3. **Login/register pages do not run the layout IIFE.** Resolved by
   inspecting `server/templates/auth/login.html` and
   `server/templates/auth/register.html`: both `{% extends "base.html" %}`
   and provide their own `{% block body %}`, bypassing
   `layout.html` entirely. The reconnect IIFE lives inside
   `layout.html`'s body block, so it never runs on login/register.
   The banner element does not exist on those pages either; no risk
   of dead state-machine code reaching for a missing
   `#lc-conn-status` element. (The IIFE has a defensive
   `if (!bannerEl || !textEl) return;` guard regardless.)

4. **Extension definition ordering is safe with `defer` scripts.**
   Resolved by inspecting `templates/base.html`: vendored htmx + ws
   extension are loaded with `defer`, meaning they execute after
   document parsing but before `DOMContentLoaded`. The IIFE inside
   `layout.html`'s `{% block body %}` runs as part of body parsing,
   strictly before any deferred script and well before
   `htmx.process(document.body)` runs at `DOMContentLoaded`. The
   `htmx.defineExtension('lc-ws-reconnect', ...)` call therefore
   beats the body-process step, and `hx-ext="response-targets,
   lc-ws-reconnect"` will resolve cleanly. The existing
   notification-bus IIFE follows the same pattern and has shipped
   without ordering issues since phase 14.
