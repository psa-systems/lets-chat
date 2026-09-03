# LC-188: Root-level page chrome (base/layout shell, error pages,
# maintenance, scheduled messages, modals) plus the remaining Settings page
# strings the language picker did not cover. Message ids are kebab-case,
# area-prefixed; Fluent ids cannot contain ".". Keep in sync with es/common.ftl
# (CI checks coverage). Core action keys (action-save/cancel/...) live in
# main.ftl and are reused here.

## A11y (LC-197): screen-reader / keyboard helpers used by base.html, the
## room message list, the composer, and the status picker. These are
## label strings - keep them short, no punctuation that an SR will read aloud.
a11y-skip-to-content = Skip to main content
a11y-messages-region-label = Chat messages
a11y-composer-message-input = Type a message
a11y-status-picker-dialog-label = Set status

## Admin nav
admin-nav-settings = Settings
admin-nav-users = Users
admin-nav-invites = Invites
admin-nav-rooms = Rooms
admin-nav-enclaves = Enclaves
admin-nav-anti-spam = Anti-spam
admin-nav-link-filter = Link filter
admin-nav-quarantine = Quarantine
admin-nav-branding = Branding
admin-nav-backup = Backup
admin-nav-modlog = Mod log
admin-nav-remote-control = Remote control
admin-nav-voice-log = Voice log
admin-nav-reports = Reports
admin-nav-support = Support
admin-nav-analytics = Analytics
admin-nav-commands = Commands
admin-nav-bots = Bots
admin-nav-webhooks = Webhooks
admin-nav-bridges = Bridges
admin-nav-version-release = Release
admin-nav-version-built = built
admin-nav-label = Admin navigation
# LC-510: sidebar group headings
admin-nav-group-overview = Overview
admin-nav-group-people = People
admin-nav-group-spaces = Spaces
admin-nav-group-safety = Safety
admin-nav-group-integrations = Integrations
admin-nav-group-system = System

## Layout shell
layout-menu = Menu
layout-toggle-nav = Toggle navigation

## Call UI (layout shell)
call-incoming = Incoming call
call-accept = Accept
call-decline = Decline
call-active = Active call
call-control-request = Remote control request
call-control-request-explain = They will be able to move your mouse and type on your keyboard.
call-control-grant = Grant control
call-control-deny = Deny
call-control-uipi = Admin-elevated windows on their machine can't be controlled.
call-control-web-peer = They're on the web app, so your clicks and keystrokes can't reach their machine. Remote control needs the desktop app on their end.
call-mute = Mute
call-start-video = Start video
call-share-screen = Share screen
call-request-control = Request control
call-devices = Call devices
call-choose-devices = Choose call devices
call-hang-up = Hang up
call-controlling-suffix = is controlling your screen.
call-stop-control = Stop control
call-stop-control-hint = or press Ctrl+Alt+F9

## Maintenance page
maintenance-page-title = Maintenance in progress
maintenance-heading = We will be right back
maintenance-body = The site is undergoing maintenance.
maintenance-retry = Try refreshing in a few minutes.

## Not found page
not-found-page-title = Not found
not-found-heading = Page not found
not-found-exists-prefix = Nothing exists at
not-found-exists-suffix = .
not-found-generic = The page you were looking for does not exist.
not-found-back-home = Back to home

## LC-220: standalone error page (themed, no sidebar chrome). The router-
## level 404 still uses not_found.html with sidebar context; these strings
## cover the handler-returned AppError variants.
error-status-not-found = Not found
error-status-forbidden = Forbidden
error-status-unauthorized = Unauthorized
error-status-conflict = Conflict
error-status-bad-request = Bad request
error-status-payload-too-large = Payload too large
error-status-too-many-requests = Too many requests
error-status-internal = Server error
## (Back-to-home label is shared with the router-level 404 via the
## existing `not-found-back-home` key; LC-220 does not duplicate it.)

## LC-552: friendly, human descriptions shown under the status heading, so even
## a bare 404 / 403 reads as real copy. A caller's curated reason (e.g. "Pin cap
## reached (max 50)") still renders as a secondary line; only the truly-internal
## variant hides its detail. One description per AppError variant.
error-desc-not-found = We could not find that page. It may have been moved or removed, or the link might be wrong.
error-desc-forbidden = You do not have access to that. If you think this is a mistake, make sure you are signed in to the right account.
error-desc-unauthorized = You need to be signed in to view this. Sign in and try again.
error-desc-conflict = That could not be saved because it clashes with something that already exists. Try a different value.
error-desc-bad-request = We could not process that request. Double-check what you entered and give it another try.
error-desc-payload-too-large = That upload is too large. Try again with a smaller file.
error-desc-too-many-requests = You are doing that a little too quickly. Wait a moment, then try again.
error-desc-internal = Something went wrong on our end. The problem has been logged; please try again in a little while.

## Poll modal
poll-create-title = Create poll
poll-close-dialog = Close dialog
poll-question = Question
poll-options = Options (one per line, 2-10)
poll-allow-multi = Allow multiple choices
poll-anonymous = Anonymous (hide who voted)
poll-close-after = Close after
poll-close-after-suffix = minutes (0 = never)
poll-post = Post poll

## Event modal (LC-491)
event-create-title = Create event
event-title-label = Event title
event-when-label = When (your local time)
event-location-label = Location (optional)
event-post = Post event
event-rsvp-going = Going
event-rsvp-maybe = Maybe
event-rsvp-no = Can't go

## GIF picker (LC-488)
gif-modal-title = Pick a GIF
gif-search-placeholder = Search GIFs...
gif-no-results = No GIFs found.
gif-powered-by = Powered by GIPHY

## Keyboard shortcuts overlay (LC-252)
shortcuts-title = Keyboard shortcuts
shortcuts-close-dialog = Close dialog
shortcuts-section-messaging = Messaging
shortcuts-section-navigation = Navigation
shortcuts-section-general = General
shortcuts-send = Send message
shortcuts-newline = New line
shortcuts-edit-last = Edit your last message
shortcuts-bold = Bold
shortcuts-italic = Italic
shortcuts-cancel = Cancel edit / close dialog
shortcuts-mention = Mention someone
shortcuts-slash = Slash command
shortcuts-next-unread = Next unread room
shortcuts-prev-unread = Previous unread room
shortcuts-switcher = Quick switcher
shortcuts-help = Show this help

## Quick switcher (LC-260)
# LC-750 (F27): the dialog's accessible name. Separate from the input's
# placeholder, which is a hint and ends in an ellipsis.
switcher-dialog-label = Quick switcher
switcher-placeholder = Jump to a room, DM, or person...
switcher-no-results = No matches

## Image lightbox (LC-262)
lightbox-label = Image viewer
lightbox-close = Close
lightbox-prev = Previous image
lightbox-next = Next image

## Code block copy (LC-272)
code-copy = Copy
code-copied = Copied

## Scheduled message modal
scheduled-modal-title = Schedule message
scheduled-modal-deliver-at = Deliver at (your local time)
scheduled-modal-repeat = Repeat
scheduled-modal-submit = Schedule
# LC-485: recurrence options + pending-row badge
scheduled-repeat-none = Does not repeat
scheduled-repeat-daily = Repeats daily
scheduled-repeat-weekdays = Repeats every weekday
scheduled-repeat-weekly = Repeats weekly

## Scheduled messages page
scheduled-page-title = Scheduled
scheduled-heading = Scheduled messages
scheduled-delivery-note = Delivered within 30 seconds of the scheduled time.
scheduled-empty = You have no pending scheduled messages. Use the schedule button in any composer to queue one.
scheduled-dropped-heading = Recently dropped
scheduled-for-prefix = for
scheduled-scheduled-for = scheduled for
scheduled-dropped-at = dropped at
scheduled-dropped-label = Dropped:
scheduled-row-delivers = delivers
scheduled-cancel-confirm = Cancel this scheduled message?

## Settings page
settings-page-title = Settings
settings-heading = Settings
settings-sections-nav = Settings sections
settings-section-profile = Profile
settings-section-appearance = Appearance
settings-section-notifications = Notifications & Activity
settings-section-privacy = Privacy & Security
settings-section-account = Data & Account
# LC-482: personal custom emoji panel.
settings-section-emoji = Custom emoji
settings-emoji-heading = Your custom emoji
settings-emoji-help-1 = Personal emoji you can use anywhere with
settings-emoji-help-2 = . Only you see them.
settings-emoji-empty = You have no personal emoji yet.
settings-emoji-shortcode-label = Shortcode
settings-emoji-image-label = Image
settings-emoji-formats = PNG, GIF, or WebP, up to
# LC-740: client-side rejection copy for the personal-emoji picker.
settings-emoji-err-type = Use a PNG, GIF, or WebP image.
settings-emoji-err-size = Image must be under 256 KiB.
settings-emoji-add = Add emoji
settings-emoji-delete = Delete
settings-emoji-delete-confirm = Delete this emoji?
# LC-487: canned responses / saved replies panel.
settings-section-canned = Saved replies
settings-canned-heading = Your saved replies
settings-canned-help-1 = Reusable snippets you post from the composer by typing
settings-canned-help-2 = . Use
settings-canned-help-3 = to drop in whatever you type after the name.
settings-canned-empty = You have no saved replies yet.
settings-canned-name-label = Shortcut name
settings-canned-desc-label = Description (optional)
settings-canned-body-label = Reply text
settings-canned-add = Add saved reply
settings-canned-delete = Delete
settings-canned-delete-confirm = Delete this saved reply?
settings-saved = Saved.
# LC-426: per-action feedback (inline status + toast)
settings-fb-saved = Saved
settings-fb-profile = Profile saved
settings-fb-avatar = Profile picture updated
settings-fb-avatar-removed = Profile picture removed
settings-fb-session-revoked = Session revoked
settings-fb-keyword-added = Highlight word added
settings-avatar-pending = Not applied yet - click Save profile
settings-remove-avatar-confirm = Remove your profile picture?
settings-data-preparing = Preparing your data...
settings-deleting = Deleting...
settings-account = Account
settings-username = Username
settings-role = Role
settings-profile = Profile
settings-status-label = Status:
settings-presence-online = Online
settings-presence-away = Away
settings-presence-dnd = Do not disturb
settings-presence-offline = Offline
settings-profile-picture = Profile picture
settings-choose-image = Choose image
settings-no-file = No file selected
settings-avatar-formats = PNG, JPEG, or WebP. Max 1 MiB.
settings-avatar-err-size = Image must be under 1 MiB.
settings-avatar-err-type = Unsupported format - use PNG, JPEG, or WebP.
settings-display-name = Display name
# LC-809: name the scope so this reads as the everywhere name, distinct from a
# per-room nickname.
settings-display-name-help = Shown everywhere, in every room. A per-room nickname (set from a room's Preferences) overrides it in that one room.
# LC-766: editable chat handle
settings-handle = Handle
settings-handle-help = Letters, numbers, and _ - . only. You can change it once every 30 days.
settings-handle-locked = You can change your handle again on
# LC-766: first-entry handle prompt
welcome-handle-title = Choose your handle
welcome-handle-heading = Choose your handle
welcome-handle-intro = This is the name others see and mention you by. We have suggested one from your account; keep it or pick your own.
welcome-handle-label = Handle
welcome-handle-help = Letters, numbers, and _ - . only.
welcome-handle-submit = Continue
settings-email = Email
settings-email-help = Used only for password reset. Leave blank to remove.
settings-email-verified = Verified
settings-email-unverified = Unverified
settings-email-resend = Resend verification email
settings-email-verify-sent = If your account has an unverified email on file, we sent a fresh verification link.
settings-bio = Bio
settings-pronouns = Pronouns
settings-pronouns-placeholder = e.g. she/her, they/them
settings-links = Links
settings-links-placeholder = https://example.com
settings-links-help = One http(s) URL per line, up to 5. Shown on your profile card.
settings-profile-timezone = Timezone
settings-profile-timezone-none = Not set
settings-profile-timezone-help = Shows your current local time on your profile card.
settings-save-profile = Save profile
settings-remove-avatar = Remove avatar
settings-preferences = Preferences
settings-pref-read-receipts = Send and receive read receipts
settings-pref-public-profile = Public profile
settings-pref-public-profile-help = When off, your profile is hidden from people search and others cannot start a new DM with you.
settings-pref-browser-notify = Show browser notifications when I am @mentioned or DM'd
settings-pref-sound = Play a sound on new mentions and DMs
settings-pref-push = Enable push notifications (works when tab is closed)
settings-pref-push-unavailable-prefix = Unavailable: set the
settings-pref-push-unavailable-suffix = environment variable on the server and restart to enable push notifications.
settings-pref-push-available = Turn this on, then use the button below to subscribe this device.
settings-push-enable = Enable desktop notifications
settings-push-enable-help = Grants permission and subscribes this browser right away. Keep the toggle above on and save so mentions, DMs, and reminders are delivered even when the tab is closed.
settings-push-status-ok = Subscribed. This device will receive push notifications.
settings-push-status-ok-toggle-off = Subscribed, but turn on the toggle above and save. Until you do, the server will not send notifications to this device.
settings-push-status-prompting = Waiting for the browser permission prompt...
settings-push-status-denied = Permission was not granted, so this device is not subscribed.
settings-push-status-blocked = Notifications are blocked for this site. Allow them in your browser settings, then try again.
settings-push-status-unsupported = This browser does not support push notifications.
settings-push-status-unavailable = Push is not available on the server right now.
settings-push-status-failed = Could not subscribe. Check the browser console for details and try again.
settings-pref-email-digest = Email me a digest of missed mentions and DMs
settings-pref-email-unavailable = Unavailable: the server administrator has not configured SMTP.
settings-pref-email-digest-help = Sent at most once per offline session (after at least an hour of inactivity). Set your email address above to receive them.
settings-pref-login-alerts = Email me when a new device signs in to my account
settings-pref-login-alerts-help = Sent the first time a browser or IP signs in. No alert is sent for devices already on file.
settings-pref-email-activity = Email me for each mention and direct message
settings-pref-email-activity-help = One email per event (not batched like the digest). If the operator has email-ingress configured, replying to the email posts your response back to the chat as you. Capped at 20 emails per minute per recipient.
settings-pref-kudos-optout = Hide me from the kudos leaderboard
settings-pref-kudos-optout-help = You can still give and receive kudos; you just won't appear on the /kudos leaderboard.
settings-save-preferences = Save preferences
# LC-304: highlight words
settings-keyword-heading = Highlight words
settings-keyword-help = Get notified and have the message highlighted when one of these words appears, just like an @mention. Case-insensitive, whole-word match.
settings-keyword-placeholder = Add a word
settings-keyword-add = Add
settings-keyword-remove = Remove
settings-keyword-empty = No highlight words yet.
settings-keyword-cap = Limit reached
settings-dnd-heading = Do Not Disturb
settings-dnd-active = Active now
settings-dnd-explain = During Do Not Disturb, push notifications are suppressed and the email digest holds your missed mentions for the next send. In-app activity is still recorded. Others see a "do not disturb" badge on your avatar.
settings-dnd-pause-heading = Pause notifications
settings-dnd-paused-until-prefix = Paused until
settings-dnd-paused-until-suffix = (UTC).
settings-dnd-resume = Resume now
settings-dnd-30-minutes = 30 minutes
settings-dnd-2-hours = 2 hours
settings-dnd-8-hours = 8 hours
settings-dnd-pause-minutes = Pause minutes
settings-dnd-min = min
settings-dnd-schedule-heading = Quiet hours schedule
settings-dnd-timezone = Timezone (leave as Off to disable the schedule)
settings-dnd-off = Off
settings-dnd-weekday-start = Weekdays start
settings-dnd-weekday-end = Weekdays end
settings-dnd-weekend-start = Weekend start
settings-dnd-weekend-end = Weekend end
settings-dnd-schedule-help = Leave a start/end pair blank to skip that group. A window whose end is earlier than its start spans midnight (e.g. 22:00 to 07:00).
settings-dnd-save-schedule = Save schedule
settings-change-password = Change password
settings-password-updated = Password updated.
settings-current-password = Current password
settings-new-password = New password
settings-confirm-new-password = Confirm new password
settings-update-password = Update password
settings-active-sessions = Active sessions
settings-sessions-help = Every device signed in to this account. Revoke any session you do not recognize. Logging out ends the current one.
settings-session-revoked = Session revoked.
settings-session-revoke-confirm = Revoke this session? The device is signed out immediately and has to sign in again.
settings-this-device = This device
settings-last-seen = Last seen
settings-from = from
settings-signed-in = Signed in
settings-privacy = Privacy
settings-manage-blocked = Manage blocked users
settings-api-tokens-heading = API tokens
settings-manage-api-tokens = Manage API tokens
settings-api-tokens-help = for the HTTP API (bots, scripts, integrations).
settings-storage-heading = Storage usage
settings-storage-using = You are using
settings-storage-of-quota = of your
settings-storage-quota-suffix = upload quota.
settings-storage-no-quota = of uploads. No quota is currently set on your account.
settings-your-data-heading = Your data
settings-your-data-help = Download a JSON file containing your profile, messages, reactions, bookmarks, sessions, blocks, room and enclave memberships, mentions, file-upload metadata, and notification preferences.
settings-download-data = Download my data
settings-delete-account-heading = Delete account
settings-delete-account-explain = Permanently remove your account, profile, messages, reactions, bookmarks, mentions, file uploads, blocks, sessions and push subscriptions. This cannot be undone. If you own an enclave with other members, transfer ownership or delete the enclave first.
settings-delete-confirm-prefix = Type
settings-delete-confirm-suffix = to confirm
settings-delete-account-button = Delete my account
settings-delete-account-confirm-js = Delete your account? This cannot be undone.
settings-about-heading = About
settings-about-version = Version
settings-about-release = Release
settings-about-commit = Commit
settings-about-built = Built

## API tokens page
api-tokens-page-title = API tokens
api-tokens-heading = API tokens
api-tokens-back = Back to settings
api-tokens-intro-prefix = Personal bearer tokens for the HTTP API. A token can be scoped narrower than your account and is shown only once. Send it as
api-tokens-intro-suffix = . See the API reference for routes and required scopes.
api-tokens-created-note = Token created. Copy it now - it won't be shown again.
api-tokens-disabled-prefix = API tokens are disabled because the server has no secret key configured
api-tokens-disabled-suffix = .
api-tokens-create-heading = Create a token
api-tokens-name = Name
api-tokens-scopes = Scopes
api-tokens-expires = Expires after (days, 0 = never)
api-tokens-create-button = Create token
api-tokens-your-tokens = Your tokens
api-tokens-none = No tokens yet.
api-tokens-created-at = created
api-tokens-last-used = last used
api-tokens-never-used = never used
api-tokens-expires-at = expires
api-tokens-revoked = revoked
api-tokens-revoke-confirm = Revoke this token? Everything using it stops working immediately and it cannot be restored.

## Blocked users page
blocked-page-title = Blocked users
blocked-heading = Blocked users
blocked-back = Settings
blocked-explain = Blocking a user hides them from people search, prevents them from starting or sending DMs to you, and hides their messages from you.
blocked-by-username = Block by username
blocked-by-username-help = Enter the exact username, including for users with private profiles.
blocked-username-placeholder = username
blocked-block-button = Block
blocked-empty = You haven't blocked anyone.
blocked-unblock = Unblock

## LC-718: support chat bubble (epic LC-717)
support-bubble-open = Need help? Ask our assistant
support-bubble-title = Support
support-bubble-subtitle = Answers from our docs
support-bubble-close = Close support
support-bubble-placeholder = Ask a question...
support-bubble-send = Send
support-bubble-human = Talk to a human
support-bubble-empty = Ask a question to get started. Answers come from our documentation; you can reach a person anytime.
support-stage-stuck = Didn't find what you needed?
support-stage-filed-title = We've filed your request
support-stage-filed-body = An admin will follow up. Reference
support-sources-label = Sources:
# LC-732: welcome + starter chips shown when the panel first opens (empty thread).
support-welcome-title = Hi! How can we help?
support-welcome-sub = Ask the assistant a question, or reach a person.
support-starter-start = How do I get started?
support-starter-docs = Where can I find the docs?
# LC-730: staged wording for the "checking the docs" loader (rotated client-side).
support-thinking-search = Searching the docs...
support-thinking-read = Reading the relevant pages...
support-thinking-write = Writing your answer...
# LC-724: waiting-for-a-human stage + add-details form.
support-waiting-live = An admin has been notified. Waiting for a reply...
support-waiting-timeout = No one has picked this up yet. You can keep waiting, or add details below.
support-add-details = Add details
support-detail-need = What do you need help with?
support-detail-tried = What have you tried?
support-detail-urgency = Urgency
support-detail-email = Contact email (optional)
support-detail-submit = Send details
support-urgency-low = Low
support-urgency-normal = Normal
support-urgency-high = High
