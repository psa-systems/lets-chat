# Changelog

Operator-facing record of changes that affect how you **run, configure, secure, or upgrade** lets-chat. If you operate a deployment, read the Security and Changed entries before upgrading: they call out default-on behavior changes and "set this env var / upgrade promptly" actions.

This file records **tagged releases**. The project's release flow (`just create-release`, see `docs/releasing.md`) bumps the version, tags, and publishes; this file is curated at that point by the release-cutter, who reads the PR titles and descriptions merged since the last tag and picks out the operator-visible ones. Between releases, the operator-visible delta is always reconstructable from `git log --oneline --first-parent <last-tag>..HEAD` and never lives only here.

Format loosely follows [Keep a Changelog](https://keepachangelog.com). Sections: **Security** (must-act), **Changed** (behavior/default/config changes), **Added**, **Fixed**, **Deprecated**. Internal-only work (refactors, test hygiene, decoder hardening with no operator impact) is intentionally omitted; the git history is the complete record.

## [Unreleased]

### Added

- **You can now expand any call tile to focus it, and pick which screen share to watch (LC-1174).** During a call, each participant tile has an expand button (shown on hover or keyboard focus): click it to blow that tile up to fill the call area on your screen and shrink the rest into a filmstrip, and minimize it to go back to the automatic layout. This also lets you choose which share to watch when more than one person is sharing at once - previously every share was enlarged with no way to pick one. The choice is yours alone and is not sent to anyone else, so each person can focus on whatever they want. Works in the room huddle, the voice-channel page, and the pop-out / floating call window.

### Fixed

- **A stopped screen share no longer leaves a frozen frame in the sharer's tile (LC-1172).** When someone stopped sharing their screen, their call tile could keep showing the last frozen frame instead of reverting to their avatar or camera, because some browsers leave the stopped video track live-but-frozen on the receiver. The tile now clears that stale frame as soon as the "stopped sharing" signal arrives (and re-shows their camera if they switch to one).
- **The live-transcription audio upload now backs off instead of hammering the server (LC-1172).** When the server rejected a transcription audio clip - rate-limited (HTTP 429) or the transcript was gone (404) - the client kept uploading the next clip anyway, filling the console with repeated errors and adding load. It now pauses uploads for a cooldown after a rate-limit, and stops uploading to a transcript that has gone away.

## [v0.12.0] - 2026-11-11

### Added

- **The desktop app now raises native OS notifications for mentions, DMs and incoming calls.** Running inside the Let's Chat desktop shell, a mention, direct message or reminder that would have shown a browser notification now fires a real OS notification instead (and only one - the browser notification is not also shown). Incoming calls, which previously only rang in-page, now also notify natively so a huddle is not missed while the window is in the background. Notifications honour the same notification preference and per-room mute / do-not-disturb gating as everywhere else (the desktop bridge reuses the server-gated signal, so a muted room stays silent). Clicking a notification to jump straight to the room arrives with the deep-link work.
- **The desktop app registers `lets-chat://` links (Windows & Linux).** Opening a `lets-chat://room/15` (or `/dm/...`, or a `#msg-...` anchor) link - or launching the app with one - opens and focuses the desktop window and navigates straight to that room, DM or message, mapping onto the app's existing routes. Only one instance runs: a second launch with a link focuses and navigates the already-open window instead of opening another. On macOS the link is delivered through a different OS path, which arrives with the macOS build (below).
- **The desktop app shows your unread count on the dock / taskbar icon.** When mentions and direct messages arrive while the Let's Chat window is in the background, the running count now appears as a badge on the app icon (a number on the Linux dock and macOS dock, a red dot overlay on the Windows taskbar), and clears when you catch up. It mirrors the same unread total the window title and sidebar already show, so a muted or do-not-disturb room never bumps the badge (the desktop bridge reads the server-gated unread signal, nothing new server-side). macOS uses the same dock-badge path but is verified under the separate Mac-build ticket.
- **The desktop app now builds and runs on macOS (LC-1157).** The Mac shell reaches parity with the Windows and Linux ones: `lets-chat://` deep links open and navigate the app (delivered via the macOS on_open_url path rather than the command line), microphone and camera work in huddles (macOS shows its one-time permission prompt the first time and remembers the choice), and the dock badge and native notifications fire. Screen sharing additionally needs the macOS Screen Recording permission, granted once in System Settings on first use. This is currently a local, unsigned development bundle (`just build-desktop-mac`); Developer ID signing, notarization and a macOS CI runner are a later phase, so Gatekeeper must be bypassed to launch it for now (right-click the app and choose Open).
- **The desktop app now handles "open in new tab" links instead of dropping them (LC-1159).** A link that opens in a new window or tab (for example a portal "Community" button that points at Let's Chat) used to do nothing in the desktop shell, because a wrapped single-window webview has no second window to put it in. Such links now route by destination: a first-party one (the configured server's own domain and its sibling apps, plus the product domains) opens in the existing app window, and any other link opens in your default browser, so an external site a message links to never takes over the app window. The first-party set is derived from the configured server URL, so self-hosted and custom-domain deployments work too.
- **Desktop installers are published to the private package registry (LC-1163, LC-1171).** Each release builds the downloadable desktop installers - a Linux `.deb` and `.AppImage`, and for Windows an NSIS installer (`-setup.exe`) plus a portable `.exe` - and publishes them to the private org package registry (`dev.a8n.run/psa-systems-private`, under `generic/lets-chat/<version>/`), where members download them; a macOS `.dmg` is uploaded there too once built on a Mac. They are not attached to the public release page. Released installers open the production server by default on first launch (you can still point the app at a different server from the welcome screen, and that choice is remembered - useful for self-hosted or multiple instances). All builds are unsigned for now, so Windows SmartScreen and macOS Gatekeeper will warn until code signing lands. The existing auto-update path is unchanged.

- **A dropped voice connection now recovers on its own instead of staying silent until you rejoin (LC-1154).** In a mesh call, when a peer connection failed to establish (common for a late joiner whose network path can't be negotiated on the first try), it used to sit dead until the person left and rejoined the call a couple of times - so they could not hear or be heard by that peer. The connection now automatically restarts ICE when it fails and re-establishes the media path. Only one side of each pair restarts (so the two ends don't collide), it tries once per failure and waits a moment first (a transient blip when someone else leaves resolves on its own), and a successful recovery is noted in the per-call voice log.

### Changed

- **The per-call voice log now says why a call problem happened, not just that it did (LC-1154).** Connection and screen-share diagnostics used to be too thin to act on: a failure logged only "ice failed", a successful connection only its local network route ("via host"), and a black screen share "frames ?/?" with no further clue. Each row now carries the specifics an operator needs: a successful connection shows the full route and round-trip time (e.g. "via host/srflx rtt 42ms"); a failure shows whether a route was ever established and, if not, which candidate types were gathered (so a missing relay from a STUN/TURN gap is visible); and a black screen share shows the frames, bytes and resolution arriving, distinguishing "nothing is arriving" from "arriving but not decoding".

### Fixed

- **Pressing Enter sends a message again on older Safari and the macOS desktop app (LC-1162).** On Safari 15 and earlier (which includes the system webview the desktop app uses on macOS 12 and below), pressing Enter in the composer did nothing: no message sent and not even a newline, so you had to click Send. The composer relied on a browser feature (`requestSubmit`) that those versions lack, so the keypress silently failed. A small compatibility shim restores it, so Enter-to-send, thread replies, saving a message edit with Cmd/Ctrl+Enter, and in-call chat all work again on those browsers. Current browsers are unaffected.
- **Call reactions animate again on older Safari and the macOS desktop app (LC-1167).** The floating emoji reactions in a call rise and swell as they float up; on Safari 15 and earlier (the system webview on macOS 12 and below) they instead just appeared at the bottom and faded, because the animation measured its travel in a CSS unit those versions do not support. The animation now uses a plain pixel measurement, so reactions animate the same everywhere.
- **Turning your camera off then on again now works in the desktop app (LC-1165).** In the desktop app on macOS, turning video off and back on during a call failed after the first time (the camera would not come back), and joining a call could be intermittent - the system webview there cannot re-open a camera or microphone once it has been released, unlike a normal browser. The desktop app now keeps the camera held (but not transmitting) while video is off, so re-enabling it is instant and reliable, and a call join/rejoin retries the device briefly if it is momentarily busy. In the desktop app the camera's indicator light therefore stays on while video is off (nothing is sent); in a regular browser the camera is released as before. Screen sharing in the desktop app on macOS 12 is a separate, known limitation.

## [v0.11.0] - 2026-11-10

### Security

- **Bot creation now validates the bot's handle (LC-1010).** The admin "create bot" form wrote `users.username` with no validation beyond non-empty, so an admin POST could store an over-length or illegal handle that no other path would accept. Bot handles now go through the same validator as every other handle (length cap, allowed charset), closing the last unvalidated handle write path.
- **Admin "invite to room" is now gated to private rooms on the server (LC-1096).** The admin invite form was only hidden for non-private rooms in the template; a hand-crafted POST naming a public room still inserted a `room_members` row (which grants nothing, since public-room access comes from enclave membership), and a missing room fell through to the insert instead of a 404. The route now refuses a non-private room with 400 and a missing room with 404, matching the enclave member routes.

### Changed

- **Chat handles are now capped at 18 characters (LC-1011).** A newly chosen or newly changed handle must be at most 18 characters (letters, numbers and `_`), enforced on every write path (settings, SSO provisioning, and admin bot creation). Existing handles longer than 18 are grandfathered: they keep working, stay mentionable, and are only constrained when their owner next changes them. The settings, welcome and bot-creation fields and their help text now state the real limit (they previously said 64 while the server rejected at 33).
- **Speaking participants now show a live mic-level ring instead of a canned equalizer (LC-1134).** Each call tile pulses a green ring around the avatar (and the tile border) that grows and brightens with the speaker's actual microphone level, and sits flat when they are muted or silent, so you can see who is talking, how loud, and that your own mic works. This replaces the old three-bar equalizer in the name pill, which looked identical to the connection-quality signal glyph next to it. Honors `prefers-reduced-motion` (a steady ring, no pulsing).

### Added

- **Search the live transcript during a call (LC-1133).** The in-call transcript drawer now has a search box: typing filters the caption list to the lines that match and highlights the matched text, with a live match count, and new lines that arrive are filtered in step (a match appears, a non-match stays hidden). Clearing the box restores the full transcript. Entirely client-side over the captions already in the drawer.
- **Turn a call's action items into YouTrack issues (LC-1135).** After a call, a reviewer can open the transcript and generate proposed issues from it: each proposal is a one-line summary, a proposed assignee (the spoken name), and the transcript lines it came from. The reviewer edits and accepts the ones they want and files them as real YouTrack issues in one step; nothing is filed without that review. An admin configures one org-wide YouTrack in Settings (base URL, a sealed API token, a default project), and a feature flag plus the moderator/admin role gate who can file, so a stray participant cannot spray the tracker. Proposing uses the transcript's LLM summary, so it follows the same AI audience and per-room AI toggle as the other transcript AI features.

### Fixed

- **Hovering a sidebar row now shows a visible fill (LC-1013).** The room, DM, profile and tool-button hover states reused the sidebar's panel-elevation tint, which sat at only 1.1:1 to 1.4:1 against the sidebar surface in every palette and mode (far below the 3:1 that WCAG 2.2 SC 1.4.11 asks of a state indicator), so hover read as nothing changing. Each palette now defines a dedicated `--sidebar-hover` fill tuned to at least 3:1 against its sidebar surface while keeping the theme's hue, and the hover consumers point at it. Keyboard focus is unchanged (the focus ring already indicated state). The contrast verifier now also checks non-text state-fill pairs at a 3:1 floor and runs in `just pre-commit` and CI, so a future token edit that flattens the difference fails the build.
- **In-call chat shows the author's own message when they are not a room member (LC-1137).** A site admin can open and post in a call via god-mode in a room they never joined (e.g. dropping into a call late). The in-call chat delivered their message to every room member but not back to the author, so the sender saw nothing from their own end while everyone else saw it immediately. The author now always receives their own message, member or not.
- **Late-join screen blackouts now self-heal instead of needing a page reload (LC-1138).** When you join a call where someone is already sharing their screen and the share renders black for you (frames never arrive), the client now automatically recovers the affected peer connection: first an ICE restart, then a scoped rebuild of just that one connection, before falling back to a prompt to rejoin. The per-call voice log (`/admin/voice-log`) also records why a blackout happened (decoded/received frame counts and the ICE route: host/srflx/relay) and logs when a screen recovers, so the blackout entry is diagnostic rather than just a flag. Mesh path only; the SFU path keeps its own media handling.
- **Call captions are no longer mis-attributed to the wrong speaker from mic bleed (LC-1131).** The browser speech-to-text engine listens to a raw microphone that also picks up other participants through your speakers, so another person's words (especially a Firefox/Safari user, whose browser cannot transcribe their own voice) could be transcribed and attributed to you. A caption is now dropped when your own echo-cancelled microphone showed no speech while it was produced, so bleed-through no longer lands under the wrong name. This does not transcribe a non-Chrome participant's own voice (that needs server-side STT); it stops the wrong-speaker lines.
- **A 1:1 screen-control session can always be revoked, even after eligibility changes (LC-953).** In a direct-message call, if the two people became ineligible to share control mid-session (one blocked the other, or an admin unverified an email), a Revoke was silently dropped: the session stayed open and the other person was never told, until the connection-drop backstop eventually cleaned it up. Revoke and deny now always go through; only starting control (request/grant) is gated by eligibility.
- **Call tiles no longer hide behind the controls in a short or narrow window (LC-1132).** On the full voice page the participant tiles were centered in the stage, so when the tiles were taller than the available space (a short window, or a couple of large tiles) the top row was pushed up under the control bar and could not be scrolled back into view. The grid now anchors to the top once the tiles overflow (while still centering them when they fit), so every tile stays reachable at any window size down to phone width.

## [v0.10.0] - 2026-11-05

Minor release on top of v0.9.0. Headline: in-call text chat for voice rooms, plus a default-on change for call sound. Read the Changed entry before upgrading.

### Added

- **In-call text chat in voice rooms (LC-1123, LC-1124, LC-1127, LC-1136).** A Chat control in the voice call bar (shown once you join) opens a panel where anyone in the call can send and read messages live during the call, like Google Meet or Zoom. Messages post to the voice room's own message channel and arrive live over the WebSocket; the panel is live-only (it fills with messages sent during the call). The panel is draggable (its position is remembered per browser), hidden by default, and does not clutter the pre-join lobby.
- **In-call live admin diagnostics drawer (LC-1118).** A feature-flagged, admin-only panel inside a call that streams the per-call voice events (joins, drops, WebRTC errors) live, so an admin can trace a bad call without leaving it for `/admin/voice-log`. Off by default; turn it on under Admin > Feature flags.
- **ICE route diagnostic on the SFU/LiveKit path (LC-1117).** The selected ICE route (host / srflx / relay) is now reported into the per-call diagnostic log on the SFU path too, not only on the mesh path, so a TURN-carried SFU call is visible to admins.
- **Transcription and screen-share faults in the voice log (LC-1129).** The per-call diagnostic log (`/admin/voice-log`) now records transcription events (start/stop, speech-recognition errors, segment-upload failures, and a "no captions while the mic is live" signal) and screen-share events, including a per-viewer blackout entry when a shared screen renders black for some participants but not others. Lets an admin trace why transcription did not pick someone up, or why one viewer saw a black screen while others saw it fine.

### Changed

- **Call sound now defaults on (LC-1120).** New accounts default to notification sound on, and a migration flips existing accounts on, so the call sound cues (incoming-call ring, someone-joined cue) are audible by default. If you relied on sound being off by default, note this change; each user can still turn it off in Settings.

### Fixed

- **Call sound cues actually play, and are louder (LC-1120).** The incoming-call ring and the join cue were synthesized on a WebAudio context created inside a WebSocket event with no user gesture, so the browser autoplay policy silently dropped them. The shared audio context is now primed on the first user interaction, and the cues are brighter and louder, so a ring or a peer joining is noticeable.
- **An expired session on a live call page redirects to sign-in (LC-1119).** When a session expired while a call page was open, the page froze on "Connection lost. Reconnecting..." with a stale roster and transcript and a sign-in panel swapped into it. The server now answers an expired htmx request with a full redirect, and the reconnect loop detects a dead session and navigates to sign-in.
- **Room-member removal is gated to private rooms (LC-1095).** Removing a member from a room is restricted to private rooms, matching the intended room-type permissions.

## [v0.9.0] - 2026-11-03

Minor release on top of v0.8.2. Headline: an admin per-call diagnostic log to trace why a call misbehaved. Plus call-UI fixes and one operator-facing container-startup change.

### Added

- **Per-call admin diagnostic log (LC-1113, LC-1114).** `/admin/voice-log` now groups the voice-event stream into calls: a "recent calls" list (room, duration, people, event + error counts, a Live badge) drills into one call's whole timeline at `/admin/voice-log/call/{id}`. The browser also reports WebRTC diagnostics into that log on the mesh path: the selected ICE route (host / srflx / **relay**, where `relay` means a TURN server carried it) on connect, and screen-share (`getDisplayMedia`) failures. Admin-only; technical events only, no call content; kept 24 hours. Lets an admin trace a blank screen-share or a failed join from the panel instead of needing a user's browser console.

### Changed

- **Runtime images start as root, then drop privileges via gosu (LC-1081).** The web and transcription-agent containers now start as root and drop to the unprivileged runtime user through a shared `gosu-entrypoint.sh`, so a bind-mounted data directory's ownership is fixed at startup. No action needed unless you pinned the container to run as a fixed non-root UID.

### Fixed

- **A popped-out call stays alive when you open Settings (LC-1106).** The settings deep-links in the rail, account menu, and your own hovercard were full-page reloads that tore down the live call; they are boosted navigations now, so the pop-out call and socket survive, while the right settings tab still opens.
- **The in-call tile grid fits the stage and screen-share re-binds (LC-1109, LC-1110).** Participant tiles scale to fit instead of overflowing into a scrolling single column with clashing names; a peer's screen-share now re-binds on the screen signal (a track that arrived on a drained renegotiation no longer leaves a blank tile), and a pop-out with a shared screen keeps a filmstrip row so names do not crowd one tile.
- **The live transcript is cleared per call (PMS-1632).** The caption panel is a persistent element that was never cleared, so a new call (even the next day) stacked its captions under the previous call's. It now clears when a different call's session starts and when a call ends, while a reconnect to the same call keeps its captions.
- **The post-call summary dialog traps focus (LC-1090).** Keyboard focus stays within the dialog, as expected for a modal.
- **The invite modal targets the enclave you are viewing (LC-1084).** A follow-up to the stale-enclave race: the invite search now reads the current enclave synchronously from the page context.

## [v0.8.2] - 2026-10-21

Patch release on top of v0.8.1: five UI and call-history fixes, no operator action.

### Changed

- **Invite people moved next to the enclave gear (LC-1080).** The Invite control was always an enclave-wide invite, so it now opens from a modal beside the enclave settings gear (reachable from any page, not just a room), and the per-room Invite button is gone. Private rooms instead get a room-specific picker in their header to add existing enclave members to that room.
- **Support entries hide when the AI help desk is off (LC-1079).** The support rail link and help bubble no longer appear for admins when the AI help desk is disabled (no LLM/embeddings or the AI gate is closed), so there is no dead support entry that leads nowhere.

### Fixed

- **Voice room Manage is discoverable and icon-only (LC-1078, LC-1080).** The Manage control now renders for managers on a boosted navigation (the gate is computed on the voice page itself, like text rooms), stays beside the room name, and is icon-only to match the app's other header actions.
- **Add room creates the room in the enclave you are viewing (LC-1077).** A follow-up to LC-1067: the add-room dialog could still target a stale enclave in the window between a navigation landing and the sidebar refresh resolving. It now reads the current enclave synchronously from the page context.
- **A busy auto-decline reads as "missed a call: busy" (LC-1075).** When an invite arrived while you were already in another call, the auto-decline was logged with the same "declined the call." text as a deliberate decline. It now persists a distinct "missed a call: busy." message, so the caller's history no longer reads as an intentional decline.

## [v0.8.1] - 2026-10-20

Patch release on top of v0.8.0: two UI fixes, no operator action. (Backfilled: the release PR merged before this section landed on the release branch.)

### Fixed

- **A voice room can now be deleted (LC-1070).** The voice page had no Manage link, so there was no path to the room's delete option. Managers now get a Manage link in the voice header, reaching the same manage / Danger-zone delete flow text rooms already had.

### Changed

- **Enclave settings is a discoverable gear (LC-1071).** The current-enclave header replaces its chevron menu with a labelled Settings gear (manager-only) and drops the "Discover or create enclaves" item, which duplicated the enclave rail's + button. A non-manager now sees just the enclave name.

## [v0.8.0] - 2026-10-20

Eighth tagged release, on top of v0.7.0. All three changes are user-facing; none needs operator action.

### Added

- **"Wave to say hi" plays a big wave animation (LC-1066).** Clicking the enclave-welcome wave now floats a large waving 👋 over the room for everyone viewing (the waver included), on top of the existing 👋 reaction. Respects reduced-motion.
- **Active voice rooms show a live participant count in the sidebar (LC-1068).** A voice channel with people on the call now carries an in-call badge, Discord-style, updated live as people join and leave, so you can see which call is active without opening it.

### Fixed

- **Add room creates the room in the enclave you are viewing (LC-1067).** After navigating between enclaves, the add-room dialog could post to a stale enclave, creating the room in the wrong place (e.g. General) and jumping you there. It now targets the current enclave.

## [v0.7.0] - 2026-10-20

Seventh tagged release, cut from `main` on top of v0.6.0. One entry is operator-facing: the GIF picker can now be configured from the admin panel instead of only a startup env var (LC-1065).

### Changed

- **An incoming 1-1 call auto-declines while you are already in a voice call (LC-1064).** In an enclave voice channel or huddle, a direct call no longer interrupts with the full-screen ring: the caller is told you are in a call and you get a quiet missed-call toast instead. An idle recipient still rings normally.

### Added

- **Configure the Giphy API key from Admin > Settings (LC-1065).** A GIF picker card sets or rotates the key at runtime, sealed at rest under `LETS_CHAT_SECRET_KEY`, taking effect with no redeploy. A stored key overrides the `LETS_CHAT_GIPHY_API_KEY` env var; with no key set the GIF button stays hidden.
- **1-1 call reactions, sounds, and a call-duration summary (LC-1063).** Floating emoji reactions in a direct call, a ringtone on an incoming call, join/leave cues in voice rooms, and a short duration panel when a call ends. Every sound follows the per-user notification-sound preference.
- **"Add chat" on the Rooms header (LC-1060).**

### Fixed

- **The sidebar no longer vanishes on enclave category changes (LC-1060).**
- **Seen-receipt timestamps show the full time on hover (LC-1062).**

## [v0.6.0] - 2026-10-20

Sixth tagged release, cut from `main` after 4 merged PRs on top of v0.5.0 (`git log --first-parent v0.5.0..v0.6.0`). One entry needs operator attention: transcription is now off after upgrade until a site admin enables it (LC-1056).

### Changed

- **Transcription is OFF after upgrade until a site admin enables it (LC-1056).** Live call transcription and voice-message / clip auto-transcripts are now behind a feature flag. **Action:** to keep transcription, a site admin sets it to Admins or Everyone on Admin > Feature flags. The AI and remote-control switches moved from Admin > Settings to that tab; their stored values are unchanged.
- **Presence is Discord-style (LC-1057).** Users go idle after 10 minutes without input (was 30), a closed or silent tab shows offline within about 90 seconds, and a reconnect no longer counts as activity. Status "Clear after" now resets the whole status (Away / Do not disturb back to Online, custom text cleared) and a running timer is kept when the status is saved again.

### Added

- **Admin > Feature flags tab (LC-1056).** Off / Admins only / Everyone rollouts for AI features, remote control and transcription.
- **Welcome message when a user joins an enclave (LC-1059).** A friendly system line posts in the enclave's welcome channel, with a "Wave to say hi" button for other members. On by default; enclave managers pick the channel or turn it off on enclave settings > General. Re-joins, startup General backfill and bot creation do not post.
- **Floating call controls (LC-1058).** The popped-out call panel has Back to call (navigates to the call's room without dropping it), Expand and Minimize buttons, on huddles and voice channels.

### Fixed

- **"Seen" times show in the viewer's local time instead of UTC (LC-1057).**
- **The status picker's Clear after dropdown works without custom text (LC-1057).**
- **"Bring back" on the floating call works from any page (LC-1058).**

## [v0.5.0] - 2026-10-19

Fifth tagged release, cut from `main` after 11 merged PRs on top of v0.4.1 (`git log --first-parent v0.4.1..v0.5.0`). No operator action is required; the entries below are the user-visible changes. Internal work (broadcast render performance LC-780 / LC-783, release CI LC-1045 / LC-1047 / LC-1049, `common` submodule bumps) is omitted and lives in the git history.

### Added

- **Settings shows the linked Bunyip account (LC-1050).** A "Bunyip account" card lists the account's SSO subject and whether its email is verified, so users and admins can tell which identity a local account is tied to. No action required.

### Changed

- **Avatar initials come from the display name, not the handle (LC-1050).** Default avatars and actor badges now use the first letter of the user's display label. Handle collisions on SSO provisioning and admin setup now get a `{handle}_{n}` suffix (for example `admin_2`). Existing handles are unchanged.
- **The sidebar Navigation block shows only on Home and global pages (LC-1051).** Inside an enclave the sidebar shows that enclave's rooms only, and the room list keeps its scroll position across live sidebar refreshes.
- **The sidebar room-list controls are one labeled toolbar (LC-1054).** "Unread only" is a labeled pill toggle beside a mark-all-read button, every sidebar block shares one gutter, the active-room highlight spans the full row, and the duplicate ROOMS-header add button is gone. On desktop a starred room's star now shows on hover with the other row controls.

### Fixed

- **Mention autocomplete no longer suggests blocked users or users with a private profile (LC-1050).**
- **The settings, enclave settings and room info pages no longer collapse to a narrow column after in-app navigation (LC-1051).**

## [v0.4.1] - 2026-10-04

No operator-visible changes; this release contains only internal CI tooling (LC-1039).

## [v0.4.0] - 2026-10-03

Fourth tagged release, cut from `main` after ~76 commits (about two weeks) of work on top of the v0.3.0 baseline. The entries below are curated from the PR titles and descriptions merged since v0.3.0 (`git log --first-parent v0.3.0..v0.4.0`); internal work (refactors, test hygiene, CI changes with no operator impact) is intentionally omitted and lives in the git history.

### Changed

- **The desktop self-updater now pulls its binary from an OCI registry, authenticated as the signed-in user (LC-733).** Let's Chat binaries are membership-gated, so the previous anonymous fetch of the Generic Packages URL could only ever answer 401. The updater now resolves `{registry}/v2/{repository}/manifests/latest-{platform}` and downloads the single artifact blob it names, using a registry credential the server hands the app after a Bunyip sign-in (`GET /desktop/registry-token`); there is no second sign-in and nothing to paste. The artifact's SHA-256 is still verified before the in-place replace, and the bearer is dropped on any cross-origin redirect. **Action:** the release still uploads binaries and `latest.json` to Forgejo Generic Packages for hand downloads, and since LC-831 it also pushes each binary to the container registry as an OCI artifact tagged `latest-{os}-x86_64` (plus a `{version}-{os}-x86_64` rollback tag), which is what the updater resolves; the registry must serve those artifacts over the OCI distribution API and accept a signed-in user's token. Operators mirroring releases replace `LETS_CHAT_UPDATE_URL` with `LETS_CHAT_UPDATE_REGISTRY_URL` (plus optional `LETS_CHAT_UPDATE_REPOSITORY` / `_TAG` / `_TOKEN`); the old variable is no longer read.

- **Desktop update manifests are no longer Ed25519-signed (LC-709).** The updater's manifest signature, the detached signature artifact published beside `latest.json`, and the public key embedded in the desktop binary are all removed. Desktop distribution is membership-gated and authenticated rather than public, so the signature is not what makes a download trustworthy; each artifact's SHA-256 is still recorded in `latest.json` and still checked before the in-place replace, which covers a corrupt or truncated download and a manifest that has drifted from the binaries it names, but not an attacker who controls the source. **Action:** the update-signing secret and public-key variable that the v0.1.0 Security entry below told you to provision are no longer read by anything; delete them from the org if you set them. Cutting a desktop release now needs no key material beyond the packages PAT.

## [v0.3.0] - 2026-08-14

Third tagged release, cut from `main` after ~198 commits (about three weeks) of work on top of the v0.2.0 baseline. The entries below are curated from the PR titles and descriptions merged since v0.2.0 (`git log --first-parent v0.2.0..v0.3.0`); internal work (UI polish, refactors, test hygiene, CI changes with no operator impact) is intentionally omitted and lives in the git history.

### Security

- **`GET /messages/{id}` now checks room membership before returning the message (LC-636).** The single-message fetch looked the message up by id alone, so any authenticated user who knew or guessed a message id could read it regardless of room membership. It now runs the same room-access check as the room message list. **Upgrade promptly.**
- **Public-room WebSocket fan-out and subscribe are now scoped to the enclave, not the whole server (LC-637).** Public-room chat events broadcast to every connected socket, and the `Subscribe` frame accepted any room id, so a signed-in user could subscribe to a public room outside their enclave and receive its message bodies. Both paths now check enclave membership. **Upgrade promptly** if you run more than one enclave.
- **SSO account adoption can no longer hijack an already-claimed identity (LC-618, tightened by LC-698).** LC-618 let a Bunyip sign-in re-adopt a local account whose `bunyip_sub` had rotated (an OP reseed), but in doing so it broadened the match past the original "unlinked row only" guard. LC-698 restores that guard and adds an explicit identity-conflict path (a bot row, an unverified-email match, or a row already linked to a different subject) that surfaces an actionable "contact an administrator" error instead of adopting the row or 500ing. No action required; a rotated-sub sign-in after an OP reseed still resolves correctly.

### Changed

- **The whole AI/LLM surface is now gated behind a runtime, role-scoped flag (LC-679).** Setting `LETS_CHAT_LLM_URL` (or the embeddings/vision/STT endpoints) only makes the backing service available; the AI surface itself stays off until a site admin flips it on in Admin > Settings > "AI features" (default off, no restart to toggle). Once on, a "Who can use AI" audience setting (LC-702, default **Everyone**) controls whether it's exposed to every user or scoped down to site admins, enclave owners/admins, and room moderators. **Action:** if you already had an LLM endpoint configured, AI features are OFF by default after this upgrade until an admin enables the flag.
- **Every boot backfills pre-existing users into the "General" default enclave (LC-621).** lets-chat now consolidates on General as the default enclave; the SSO signup path auto-joins new users, and an idempotent startup backfill (`INSERT OR IGNORE`) joins every account that existed before this upgrade. No action required.
- **Liveness/readiness probes replace the old container healthcheck (LC-581).** `GET /healthz` (dependency-free liveness) and `GET /readyz` (pings the auth/chat/settings SQLite pools, reports SSO-client presence, `503` when a store is down) are new, unauthenticated, and exempt from maintenance mode. **Action:** the container `HEALTHCHECK` now targets `/healthz` instead of the old check; point any external liveness gate at `/healthz` and readiness/load-balancer draining at `/readyz`, never the other way around, or a transient dependency blip restarts an otherwise-healthy process.

### Added

- **Image alt-text auto-draft via an operator vision model (LC-667).** Set `LETS_CHAT_VISION_URL` (an OpenAI-vision-compatible endpoint, e.g. llava/llama3.2-vision) plus optional `LETS_CHAT_VISION_API_KEY` / `LETS_CHAT_VISION_MODEL` (default `llava`) to add an "Auto-draft" button to the image alt-text editor. Separate endpoint from the chat LLM; the image is sent only to this operator endpoint. Unset = the button stays hidden.
- **Local AI moderation triage (LC-670).** With `LETS_CHAT_AI_MODERATION` set truthy and an LLM configured, posted messages are classified in the background and clear spam/harassment/inappropriate cases are flagged to the admin report queue for human review; nothing is auto-deleted or hidden. Content only reaches your own LLM.
- **Personal weekly recap DM (LC-671).** With `LETS_CHAT_WEEKLY_RECAP` set truthy and an LLM configured, each active user is DMed a short AI-written weekly recap from the assistant bot once a week; quiet weeks are skipped.
- **Background embeddings backfill for pre-existing messages (LC-673).** A new dispatcher embeds a small batch of un-embedded messages every 60s so semantic search and "Find related" cover history, not just messages posted after the embeddings endpoint was configured. No-op with no embeddings endpoint set; no action required.
- A cluster of AI-assistant features layered on the existing `LETS_CHAT_LLM_URL` / `LETS_CHAT_STT_URL` endpoints: per-bot AI persona (LC-695), admin-picked assistant-bot identity (LC-693), a translation target-language picker (LC-688), an AI writing assistant (LC-655), auto thread titles (LC-668), a scheduled per-room AI digest (LC-665), an AI gist subject for the email digest (LC-672), and call-transcript recaps (LC-662, LC-663, LC-664, LC-629). No action required beyond the LC-679 flag above; each is additional load/cost on whichever LLM/STT endpoint you have configured.

### Fixed

- **Bunyip-verified users are now marked email-verified (LC-627).** SSO provisioning never stamped `email_verified_at`, which silently blocked call remote-control (gated on a verified email) for every SSO-only user. No action required.

## [v0.2.0] - 2026-07-22

Second tagged release, cut from `main` after ~508 commits (roughly seven weeks) of work on top of the v0.1.0 seed. Server deployments track the `latest` OCI image built off `main`, so a running server does not pick this up until its image is repulled and the container is restarted; the tag is the pinning + desktop-updater baseline. The entries below are curated from the `[operator-action]` markers in `git log v0.1.0..HEAD`; internal work (refactors, test hygiene, dependency hardening with no operator impact) is intentionally omitted and lives in the git history.

### Security

- **Session tokens are now hashed at rest (LC-514).** The `sessions` table stored bearer cookies in plaintext; they are now stored hashed, and the first startup after this deploy runs a one-shot in-place re-hash of every existing row. Existing user sessions stay valid because the cookie value is the input to the new hashed lookup, so no one is logged out. **Action:** a database backup taken BEFORE this deploy still contains the plaintext cookies it captured; if that backup's cookie window has not expired (default cookie TTL is 30 days), rotate or invalidate it after deploying.
- **Security response headers are now emitted on every response (LC-504).** lets-chat now sends HSTS (`max-age=31536000; includeSubDomains; preload`), Content-Security-Policy, `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, Referrer-Policy and Permissions-Policy on every response. **Action:** if you terminate TLS at a fronting proxy that also injects these headers, reconcile them to avoid duplicates, and be aware the HSTS `includeSubDomains; preload` directive now applies to the whole domain for a year.

### Changed

- **GIF picker migrated from Tenor to Giphy (LC-505).** Google's Tenor v2 API shut down, so the composer GIF picker now uses Giphy. **Action:** set `LETS_CHAT_GIPHY_API_KEY` in place of the old `LETS_CHAT_TENOR_API_KEY`; optional `LETS_CHAT_GIPHY_RATING` (`g`/`pg`/`pg-13`/`r`, default `pg-13`). If you never set the Tenor key the picker was hidden and stays hidden until you set the Giphy key.
- **Recorded video clips are now transcribed via the STT endpoint (LC-496).** When `LETS_CHAT_STT_URL` is configured, video clips are sent to it in addition to call audio. No action required; this is a load/cost consideration on a metered or third-party STT engine, and an engine that rejects video containers simply yields no clip transcript (the clip still posts and plays).
- **Per-message translation sends message text to the LLM endpoint (LC-486).** With `LETS_CHAT_LLM_URL` set, a per-message Translate action now sends that message's text to the endpoint (cached). No action required; load/cost/content consideration on metered or third-party engines.
- **Catch-me-up summaries send chat text to the LLM endpoint (LC-484).** With `LETS_CHAT_LLM_URL` set, room and thread message text is now sent to that endpoint for on-demand summaries (cached). No action required; same consideration as above.
- **Voice messages are now transcribed via the STT endpoint (LC-483).** With `LETS_CHAT_STT_URL` set, voice-message audio is sent to it in addition to call audio. No action required; load/cost consideration on a metered STT engine.

### Added

- **LiveKit SFU stage audio (LC-512).** Optional large-audience stage audio backed by a self-hosted LiveKit server. **Action to enable:** run LiveKit and set `LETS_CHAT_LIVEKIT_URL` (`wss://...`), `LETS_CHAT_LIVEKIT_API_KEY`, `LETS_CHAT_LIVEKIT_API_SECRET`; the build must run `just vendor-js` to fetch the browser SDK (already wired into `just build` / `build-saas`). Left unset, stage roles and request-to-speak still work with no audio.
- **GIF picker in the composer (LC-488, superseded by LC-505).** Introduced against Tenor and migrated to Giphy within this release window; see the Giphy entry under Changed for the current env var.

### Deprecated

- **`LETS_CHAT_TENOR_*` env vars (LC-505).** Replaced by `LETS_CHAT_GIPHY_*` after the Tenor API shutdown. The old vars are ignored.

## [v0.1.0] - 2026-06-24

First tagged release. lets-chat previously shipped only as the `latest` OCI image built off `main`; v0.1.0 is the seed version cut to a tag so deployments can pin a release (e.g. `lets-chat:v0.1.0` in your OCI registry) and the desktop self-updater has a baseline. This entry is the operator-visible snapshot of everything in `main` at the tag, folding the prior **Pre-release** seed (snapshot 2026-05-30) together with the operator-action changes that landed after it.

### Security

- **Authentication is Bunyip SSO only; local auth retired (LC-22).** The standalone build refuses to start without all four `LETS_CHAT_BUNYIP_SSO_ISSUER` / `_CLIENT_ID` / `_CLIENT_SECRET` / `_REDIRECT_URI` vars set and the OP reachable for discovery + JWKS; there is no local username/password, registration, password reset, or 2FA. **Action:** land the bunyip-api `oauth_clients` seed migration first, set the four vars on every deployment, snapshot `auth.db` before deploy (the cutover migration is irreversible), then deploy. Existing local users cannot sign in post-cutover; re-graft a returning user's authored content manually once they come back via Bunyip.
- **Unified outbound SSRF guard; closed an unguarded Web Push SSRF (LC-152).** All server-initiated outbound HTTP (outgoing webhooks, Web Push, bridge-avatar fetch) now routes through a single guarded client that refuses connections resolving to private / non-public addresses. The audit that motivated this found Web Push (`push/mod.rs`) had **no SSRF guard at all** in shipped versions, so a crafted push endpoint could reach internal-network addresses and exfiltrate response metadata. **Upgrade promptly** if your deployment has Web Push or outgoing webhooks enabled.
- **Bounded image decode closes an unbounded GIF decompression-bomb DoS (LC-206-IMAGE-LIMITS).** Shipped versions decoded GIFs through an unbounded decoder; an attacker-supplied GIF bomb (small file, huge decoded size) via the foreign-avatar fetch or upload pipeline could exhaust server memory and crash the process (remote DoS). No configuration change is required; the fix is the explicit decode `Limits`. **Upgrade promptly.**
- **Signed desktop update manifest + binary-hash verification (LC-210-BINARY-INTEGRITY).** The desktop updater now verifies an Ed25519-signed `latest.json` and each binary's SHA-256 before the in-place replace, and fails closed if unverified. **Action (before cutting desktop releases):** provision update signing - generate an Ed25519 keypair, set the `DESKTOP_UPDATE_SIGNING_KEY` secret (private PEM) and `DESKTOP_UPDATE_PUBLIC_KEY` variable (public key hex) per `docs/desktop-update-signing.md`. Vector: an unverified binary served via update-mirror compromise or a redirect to a public attacker host.

### Changed

- **Foreign bridge-avatar proxy fetching is default-ON (LC-78-AVATAR-PROXY).** When a protocol bridge submits a foreign avatar URL, the server now fetches it server-side (capped, SSRF-guarded, re-encoded) and serves it same-origin, instead of rejecting it. This means **outbound fetches to foreign homeservers happen by default** on bridge traffic. To restore the v1 reject-non-null behavior, set `LETS_CHAT_BRIDGE_AVATAR_PROXY_ENABLED=false` (or `0`).
- **Destructive message-retention sweep, gated default-OFF (`LETS_CHAT_RETENTION_SWEEP_ENABLED`).** A background sweep that **hard-deletes** messages past a room's `retention_days` is shipped but disabled unless you set `LETS_CHAT_RETENTION_SWEEP_ENABLED=1` (or `true`) and restart. It is irreversible; enable deliberately. Off by default while the thread-retention semantics question is open.
- **Email-ingress messages now fire `message.posted` outgoing webhooks (LC-205).** If you run LC-75 outgoing-webhook subscribers or LC-78 bridge daemons, they now receive `message.posted` for messages that arrived via email ingress (previously these were silently not delivered). No action required unless your subscriber assumed email-ingress messages never fired webhooks.

### Added

- **Email ingress (LC-77).** Optional IMAP-polled mailbox posts mail addressed to `<token>@<ingress-domain>` into rooms. Gated on `LETS_CHAT_SECRET_KEY` set + `imap_inbox_config.enabled` + an `ingress_domain`; enabling requires a restart. See `docs/email-ingress.md`.
- **Per-message notification emails + reply-by-email (LC-77-REPLY).** Per-user opt-in (`notify_email_activity_enabled`, default off); replies via a `reply-<token>@<ingress-domain>` address post as the real user. Requires SMTP + ingress configured.
- **Bridge-avatar cache admin diagnostic page (LC-207).** Read-only `/admin/bridges/avatars` shows cache stats and recent failed avatar fetches with reasons, so "why is this bridged user showing initials" is answerable without SQL. No action required.
- **Optional self-hosted server-side call transcription (LC-393).** Set `LETS_CHAT_STT_URL` (+ optional `_API_KEY` / `_MODEL`) to an OpenAI-compatible `/v1/audio/transcriptions` endpoint to switch call transcription from the in-browser Web Speech engine to self-hosted STT (browser-agnostic, keeps audio off third-party clouds). Leave unset to keep the browser engine.
- **Optional AI transcript summaries (LC-396).** Set `LETS_CHAT_LLM_URL` (+ optional `_API_KEY` / `_MODEL`) to an OpenAI-compatible `/v1/chat/completions` endpoint to enable the transcript "Summarize" action (markdown summary + action items, cached). Leave unset to hide the action.
