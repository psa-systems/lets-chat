# LC-188: Root-level page chrome (base/layout shell, error pages,
# maintenance, scheduled messages, modals) plus the remaining Settings page
# strings the language picker did not cover. Message ids are kebab-case,
# area-prefixed; Fluent ids cannot contain ".". Keep in sync with es/common.ftl
# (CI checks coverage). Core action keys (action-save/cancel/...) live in
# main.ftl and are reused here.

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
admin-nav-analytics = Analytics
admin-nav-commands = Commands
admin-nav-bots = Bots
admin-nav-webhooks = Webhooks
admin-nav-version-release = Release
admin-nav-version-built = built

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

## Scheduled message modal
scheduled-modal-title = Schedule message
scheduled-modal-deliver-at = Deliver at (your local time)
scheduled-modal-submit = Schedule

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
settings-saved = Saved.
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
settings-avatar-formats = PNG, JPEG, or WebP. Max 1 MiB.
settings-display-name = Display name
settings-email = Email
settings-email-help = Used only for password reset. Leave blank to remove.
settings-email-verified = Verified
settings-email-unverified = Unverified
settings-email-resend = Resend verification email
settings-email-verify-sent = If your account has an unverified email on file, we sent a fresh verification link.
settings-bio = Bio
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
settings-pref-push-available = A browser permission prompt will appear on the next mention.
settings-pref-email-digest = Email me a digest of missed mentions and DMs
settings-pref-email-unavailable = Unavailable: the server administrator has not configured SMTP.
settings-pref-email-digest-help = Sent at most once per offline session (after at least an hour of inactivity). Set your email address above to receive them.
settings-pref-login-alerts = Email me when a new device signs in to my account
settings-pref-login-alerts-help = Sent the first time a browser or IP signs in. No alert is sent for devices already on file.
settings-pref-email-activity = Email me for each mention and direct message
settings-pref-email-activity-help = One email per event (not batched like the digest). If the operator has email-ingress configured, replying to the email posts your response back to the chat as you. Capped at 20 emails per minute per recipient.
settings-save-preferences = Save preferences
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
