# LC-188: Admin UI strings (source locale). Keys are kebab-case, prefixed
# "admin-", no dots. Grouped by template area.

## Analytics
admin-analytics-title = Analytics
admin-analytics-heading = Analytics
admin-analytics-date-range = Date range
admin-analytics-recompute = Recompute today
admin-analytics-recompute-title-prefix = Recompute today's metrics
admin-analytics-counts-note = Counts only; no per-user activity is shown. Metrics are pre-aggregated daily.
admin-analytics-over = over
admin-analytics-retention-heading = Retention by signup cohort
admin-analytics-retention-note = Percent of each weekly signup cohort that sent a message in the Nth week after joining.
admin-analytics-no-cohorts = No signup cohorts yet.
admin-analytics-th-cohort = Cohort
admin-analytics-th-users = Users

## Anti-spam
admin-antispam-title = Anti-spam
admin-antispam-heading = Anti-spam
admin-antispam-saved = Settings saved.
admin-antispam-rate-limits-heading = Rate limits (per minute)
admin-antispam-rate-limits-note-1 = A cap of
admin-antispam-rate-limits-note-2 = disables the limit. Per-IP limits do nothing on deployments without a reverse proxy that forwards
admin-antispam-messages-per-user = Messages per user
admin-antispam-registrations-per-ip = Registrations per IP
admin-antispam-login-attempts-per-ip = Login attempts per IP
admin-antispam-login-attempts-note = Covers the password form and the 2FA / recovery challenge (shared budget). Per IP, so it requires a trusted reverse proxy. 0 disables.
admin-antispam-password-resets-per-ip = Password reset requests per IP
admin-antispam-defenses-heading = Defenses
admin-antispam-link-filter = Link filter
admin-antispam-link-filter-note-1 = Runs message bodies through the rules at
admin-antispam-link-filter-note-2 = Rules can block, quarantine, or warn on matched domains.
admin-antispam-honeypot = Honeypot on register
admin-antispam-honeypot-note = Hidden form field that legit users never fill but lazy bots do. No false-positive impact.
admin-antispam-save = Save

## Backup / restore
admin-backup-title = Backup / Restore
admin-backup-heading = Backup / restore
admin-backup-staged-title = A restore is staged.
admin-backup-staged-1 = Restart the server to apply it. On startup the current data directory is renamed aside (suffix
admin-backup-staged-2 = ) and the staged copy takes its place. If you change your mind, delete the
admin-backup-staged-3 = file and the
admin-backup-staged-4 = sibling directory inside the data directory before restarting.
admin-backup-headsup-title = Heads up:
admin-backup-headsup-1 = every restore leaves the previous data directory on disk as a sibling named
admin-backup-headsup-2 = . These accumulate over time; once you have confirmed the restore is healthy, remove old
admin-backup-headsup-3 = directories manually to reclaim space. There is no automatic cleanup.
admin-backup-create-heading = Create backup
admin-backup-create-1 = Downloads a single
admin-backup-create-2 = with consistent SQLite snapshots (via
admin-backup-create-3 = ) of all three databases plus the
admin-backup-create-4 = and
admin-backup-create-5 = trees. Each entry is sha256'd into a
admin-backup-create-6 = so a later restore can verify integrity. The server keeps serving while the snapshot runs. Large deployments may take a minute or two; the response begins streaming as soon as the archive finishes building on disk.
admin-backup-download = Download backup
admin-backup-restore-heading = Restore from archive
admin-backup-restore-note = Upload a previously-downloaded backup. The server validates the manifest + per-file sha256, refuses archives built on a different Let's Chat version, then stages the contents in a sibling directory. A restart finalizes the swap.
admin-backup-stage-confirm = Stage this archive for restore? On the next server restart the current data will be replaced.
admin-backup-stage = Stage restore
admin-backup-stage-note = The restart step is the confirmation gate; staging alone does not touch the live data directory.

## Bots
admin-bots-title = Bots
admin-bots-heading = Bots
admin-bots-intro = Bot accounts are machine identities. They authenticate only via their API token (the cookie login refuses them), post with a "bot" badge, and never receive notifications. Disabling a bot bans it and revokes all its tokens.
admin-bots-created-prefix = Bot
admin-bots-created-suffix = created. Copy its API token now - it won't be shown again.
admin-bots-no-secret-1 = Bots require a server secret key
admin-bots-no-secret-2 = to mint their API token. Set it and restart to create bots.
admin-bots-create-heading = Create a bot
admin-bots-username = Username
admin-bots-token-scopes = API token scopes
admin-bots-create-button = Create bot
admin-bots-th-bot = Bot
admin-bots-th-created = Created
admin-bots-th-status = Status
admin-bots-th-actions = Actions
admin-bots-status-disabled = disabled
admin-bots-status-active = active
admin-bots-disable = Disable
admin-bots-disable-confirm = Disable this bot? It will be banned and all its API tokens revoked.
admin-bots-empty = No bots yet.

admin-bridges-title = Bridges
admin-bridges-heading = Bridges
admin-bridges-intro = Protocol bridges run OUT OF PROCESS as separate daemons (matrix-appservice-bridge or similar) and post foreign-protocol messages into a Let's Chat room via the API. Registering a bridge here creates the bot user, mints its bridge-scoped API token, and stores its sealed daemon config. The daemon authenticates with the token; Let's Chat tracks heartbeats but does not run the daemon. Removing a bridge stops new traffic but leaves historical bridged messages renderable.
admin-bridges-no-secret = Bridges need a server secret key to seal daemon configuration.
admin-bridges-created-prefix = Bridge bot
admin-bridges-created-suffix = created. Copy its API token now - it will not be shown again.
admin-bridges-token-scopes-note = Token scopes: bridge:post + bridge:heartbeat.
admin-bridges-create-heading = Register a bridge
admin-bridges-room = Room
admin-bridges-bot-username = Bot username
admin-bridges-kind = Protocol kind
admin-bridges-kind-note = IRC and XMPP daemons share this surface; v1 ships Matrix only.
admin-bridges-config = Daemon config (opaque to the server, sealed at rest)
admin-bridges-config-note = Stored encrypted under LETS_CHAT_SECRET_KEY. The shape is daemon-specific (typically JSON with homeserver URL and shared secret).
admin-bridges-create-button = Register bridge
admin-bridges-th-room = Room
admin-bridges-th-kind = Kind
admin-bridges-th-bot = Bot
admin-bridges-th-status = Status
admin-bridges-th-last-heartbeat = Last heartbeat
admin-bridges-remove = Remove
admin-bridges-remove-confirm = Remove this bridge? New traffic stops; historical messages remain.
admin-bridges-empty = No bridges registered.

## Branding
admin-branding-title = Branding
admin-branding-heading = Branding
admin-branding-saved = Branding saved.
admin-branding-intro-1 = Colors propagate to every page via CSS variables (no Tailwind rebuild required). The logo is shown on the login page and is served from
admin-branding-intro-2 = The login heading and body render through a restricted markdown pipeline: bold, italic, links, lists, and paragraphs are allowed; raw HTML and fenced code blocks are stripped.
admin-branding-primary-color = Primary color
admin-branding-accent-color = Accent color
admin-branding-logo = Logo
admin-branding-current-logo-alt = Current logo
admin-branding-current-logo = Current logo
admin-branding-logo-help = PNG / JPEG / WebP / GIF up to 1 MiB. Leave empty to keep the current logo.
admin-branding-favicon = Favicon
admin-branding-current-favicon-alt = Current favicon
admin-branding-current-favicon = Current favicon
admin-branding-favicon-help = PNG / ICO / SVG up to 1 MiB. Leave empty to keep the current favicon. Browsers cache favicons aggressively; a change may need a hard refresh to appear.
admin-branding-login-heading = Login page heading
admin-branding-login-heading-help = Plain text. Shown above the sign-in form. Empty falls back to "Sign in".
admin-branding-login-body = Login page body
admin-branding-login-body-help = Restricted markdown. Use it for a welcome note, a link to your privacy policy, or operator contact info.
admin-branding-save = Save branding

## Enclaves
admin-enclaves-title = Enclaves
admin-enclaves-heading = Enclaves
admin-enclaves-intro = Every enclave on this server, regardless of your membership. Use the manage link to enter the enclave (site-admin god-mode bypasses the membership check).
admin-enclaves-th-name = Name
admin-enclaves-th-visibility = Visibility
admin-enclaves-th-owner = Owner
admin-enclaves-th-members = Members
admin-enclaves-th-storage = Storage (used / quota MiB)
admin-enclaves-th-created = Created
admin-enclaves-th-actions = Actions
admin-enclaves-public = Public
admin-enclaves-private = Private
admin-enclaves-none = none
admin-enclaves-unlimited = unlimited
admin-enclaves-save = Save
admin-enclaves-open = Open
admin-enclaves-manage = Manage

## Invites
admin-invites-title = Invites
admin-invites-heading = Invite codes
admin-invites-create = Create invite
admin-invites-th-code = Code
admin-invites-th-created-by = Created by
admin-invites-th-created-at = Created at
admin-invites-th-used-by = Used by
admin-invites-th-action = Action
admin-invites-revoke = Revoke

## Link filter
admin-linkfilter-title = Link filter
admin-linkfilter-heading = Link filter rules
admin-linkfilter-intro-1 = Patterns match the host of every URL in a message body. Use a literal domain
admin-linkfilter-intro-2 = or a simple glob with
admin-linkfilter-intro-3 = Matching is case-insensitive.
admin-linkfilter-intro-4 = Make sure the feature is enabled on the
admin-linkfilter-intro-link = Anti-spam settings page
admin-linkfilter-pattern = Pattern
admin-linkfilter-action = Action
admin-linkfilter-warn = warn
admin-linkfilter-quarantine = quarantine
admin-linkfilter-block = block
admin-linkfilter-add-rule = Add rule
admin-linkfilter-th-pattern = Pattern
admin-linkfilter-th-action = Action
admin-linkfilter-th-added-by = Added by
admin-linkfilter-th-added = Added
admin-linkfilter-th-actions = Actions
admin-linkfilter-delete = Delete
admin-linkfilter-empty = No rules. Add one above to start filtering.
# LC-510: delete-rule confirmation
admin-linkfilter-delete-confirm = Delete this link-filter rule?

## Mod log
admin-modlog-title = Mod log
admin-modlog-heading = Moderation log
admin-modlog-th-who = Who
admin-modlog-th-action = Action
admin-modlog-th-target = Target
admin-modlog-th-reason = Reason
admin-modlog-th-when = When
admin-modlog-empty = No moderation actions logged yet.

## Webhook deliveries
admin-deliveries-title = Webhook deliveries
admin-deliveries-heading = Deliveries
admin-deliveries-webhook = webhook
admin-deliveries-back = Back
admin-deliveries-th-event = Event
admin-deliveries-th-attempt = Attempt
admin-deliveries-th-status = Status
admin-deliveries-th-scheduled = Scheduled
admin-deliveries-th-delivered = Delivered
admin-deliveries-pending = pending
admin-deliveries-empty = No deliveries recorded.

## Outgoing webhooks
admin-webhooks-title = Outgoing webhooks
admin-webhooks-heading = Outgoing webhooks
admin-webhooks-intro-1 = Register a URL to receive a signed
admin-webhooks-intro-2 = when matching events fire. The body is
admin-webhooks-intro-3 = and each request carries
admin-webhooks-intro-4 = over the raw body, keyed by the webhook's signing secret, plus
admin-webhooks-intro-5 = Verify both. Failed deliveries retry with backoff; after repeated failures the webhook auto-disables.
admin-webhooks-secret-prefix = Signing secret for webhook
admin-webhooks-secret-suffix = - copy it now, it won't be shown again.
admin-webhooks-create-heading = Create a webhook
admin-webhooks-scope = Scope
admin-webhooks-scope-global = global
admin-webhooks-scope-enclave = enclave
admin-webhooks-scope-room = room
admin-webhooks-scope-id = Scope id (enclave/room)
admin-webhooks-scope-id-placeholder = (blank for global)
admin-webhooks-delivery-url = Delivery URL
admin-webhooks-events = Events
admin-webhooks-create-button = Create webhook
admin-webhooks-th-scope = Scope
admin-webhooks-th-events = Events
admin-webhooks-th-url = URL
admin-webhooks-th-status = Status
admin-webhooks-th-actions = Actions
admin-webhooks-status-disabled = disabled
admin-webhooks-status-active = active
admin-webhooks-fails = fails
admin-webhooks-history = History
admin-webhooks-rotate = Rotate secret
admin-webhooks-rotate-confirm = Rotate this webhook's signing secret? The current secret stops working immediately.
admin-webhooks-enable = Enable
admin-webhooks-disable = Disable
admin-webhooks-delete = Delete
admin-webhooks-delete-confirm = Delete this webhook subscription?
admin-webhooks-empty = No outgoing webhooks yet.

## Quarantine
admin-quarantine-title = Quarantine
admin-quarantine-heading = Quarantined messages
admin-quarantine-intro = Messages held by the link filter pending moderator review. Approving releases the message into the room; rejecting soft-deletes it.
admin-quarantine-th-author = Author
admin-quarantine-th-room = Room
admin-quarantine-th-body = Body
admin-quarantine-th-matched = Matched
admin-quarantine-th-held = Held
admin-quarantine-th-actions = Actions
admin-quarantine-approve = Approve
admin-quarantine-reject = Reject
admin-quarantine-reject-confirm = Reject this message? It will be permanently discarded.
admin-quarantine-empty = Nothing in the queue.

## Room row
admin-roomrow-topic-placeholder = Topic
admin-roomrow-save = Save
admin-roomrow-username-placeholder = Username
admin-roomrow-invite = Invite
admin-roomrow-regen = Regen
admin-roomrow-delete = Delete
admin-roomrow-delete-confirm-prefix = Delete room
admin-roomrow-delete-confirm-suffix = This deletes all its messages.

## Rooms
admin-rooms-title = Rooms
admin-rooms-heading = Rooms
admin-rooms-intro = Create rooms inside an enclave via the per-enclave landing page. This view is read-only and intended for global moderation.
admin-rooms-th-name = Name
admin-rooms-th-type = Type
admin-rooms-th-members = Members
admin-rooms-th-invite = Invite
admin-rooms-th-actions = Actions

## Settings
admin-settings-title = Settings
admin-settings-maintenance-on-title = Maintenance mode is ON.
admin-settings-maintenance-on-body = Non-admin users see a 503 maintenance page. Toggle off below to restore access.
admin-settings-maintenance-heading = Maintenance mode
admin-settings-maintenance-enable = Enable maintenance mode
admin-settings-maintenance-enable-note = Non-admins see a 503 page; admins keep full access so you can flip this back off when done.
admin-settings-maintenance-message-label = Message shown to users
admin-settings-maintenance-message-placeholder = Back at 17:00 UTC; upgrading the database.
admin-settings-maintenance-save = Save maintenance mode
admin-settings-smtp-heading = SMTP (outbound mail)
admin-settings-smtp-note-1 = SMTP is configured via environment variables, not the admin UI. Set
admin-settings-smtp-note-2 = and the optional
admin-settings-smtp-note-3 = pair, then restart the server. See
admin-settings-smtp-note-4 = for the full list.
admin-settings-smtp-note-5 = Earlier builds rendered an SMTP form here that wrote to
admin-settings-smtp-note-6 = but was never read by the mailer; the form has been removed and any stale rows are cleared by the next migration.
admin-settings-imap-heading = Email ingress (IMAP poll)
admin-settings-imap-saved = IMAP settings saved. Restart the server to pick up the new configuration.
admin-settings-imap-intro-1 = Let's Chat polls this mailbox every 5 minutes; messages addressed to
admin-settings-imap-intro-2 = post to their target room as the synthetic email actor. Set up a dedicated mailbox at your provider and point this form at it. The password is encrypted at rest under
admin-settings-imap-intro-3 = After saving, restart the server: the spawn gate reads this row at startup, not per tick.
admin-settings-imap-host = IMAP host
admin-settings-imap-port = IMAP port
admin-settings-imap-port-note = 993 for IMAPS (recommended). The TLS toggle below should match.
admin-settings-imap-use-tls = Use TLS (port 993)
admin-settings-imap-username = IMAP username
admin-settings-imap-password = IMAP password (write-only)
admin-settings-imap-password-keep = leave blank to keep existing
admin-settings-imap-password-unset = not configured yet
admin-settings-imap-folder = Folder to poll
admin-settings-imap-ingress-domain = Ingress domain
admin-settings-imap-ingress-domain-note-1 = The
admin-settings-imap-ingress-domain-note-2 = half of the address an external sender mails. Per-room inbox addresses become
admin-settings-imap-dead-letter = Dead-letter folder (optional)
admin-settings-imap-dead-letter-note-1 = When set, dropped messages are UID-COPYd into this IMAP folder before being marked
admin-settings-imap-dead-letter-note-2 = on the source. You must create the folder at your IMAP provider; Let's Chat does not auto-create it. Empty = off (drops are diagnosed by log only).
admin-settings-imap-enable = Enable IMAP poll
admin-settings-imap-enable-note = Off until you have verified the configuration. The poll loop refuses to spawn on missing fields; check server logs after restart.
admin-settings-imap-save = Save IMAP settings
admin-settings-uploads-heading = Uploads
admin-settings-uploads-generated-prefix = Generated
admin-settings-uploads-generated-suffix = preview(s).
# LC-510: inline save-feedback toasts (htmx path)
admin-saved = Saved
admin-uploads-regenerated = Previews regenerated
admin-uploads-purged = Orphans purged
admin-settings-uploads-purged-prefix = Purged
admin-settings-uploads-purged-suffix = orphan upload(s).
admin-settings-uploads-disk-size = On-disk size
admin-settings-uploads-orphan-rows = Orphan rows (uploads not attached to a message)
admin-settings-uploads-regenerate = Regenerate thumbnails
admin-settings-uploads-regenerate-note = May take several minutes on large deployments. Wait for the page to reload; do not click again.
admin-settings-uploads-purge = Purge orphans now
admin-settings-defaults-heading = Defaults for new users
admin-settings-defaults-digest = New users start with email digest enabled
admin-settings-defaults-digest-note = Off by default for privacy. Only affects future registrations; existing users are not changed. Email digest also requires SMTP configured via environment variables.
admin-settings-defaults-save = Save default

# LC-207-OBSERVABILITY (#278): email-ingress poll health + drop log.
admin-settings-ingress-health-heading = Email ingress health
admin-settings-ingress-last-poll = Last poll
admin-settings-ingress-last-ok = Last successful poll
admin-settings-ingress-consecutive-failures = Consecutive failures
admin-settings-ingress-last-tick = Last tick (fetched / posted / dropped)
admin-settings-ingress-last-error = Last error:
admin-settings-ingress-not-run = The IMAP poll loop has not run yet (disabled, or no tick since startup).
admin-settings-ingress-drops-24h = Drops by reason (last 24h)
admin-settings-ingress-drop-time = Time
admin-settings-ingress-drop-reason = Reason
admin-settings-ingress-drop-uid = UID
admin-settings-ingress-drop-detail = Detail

# LC-207-OBSERVABILITY (#278): retention-sweep status.
admin-settings-retention-heading = Message retention sweep
admin-settings-retention-enabled = Enabled via LETS_CHAT_RETENTION_SWEEP_ENABLED.
admin-settings-retention-disabled = Disabled. Set LETS_CHAT_RETENTION_SWEEP_ENABLED=1 and restart to enable the per-room retention hard-delete sweep.
admin-settings-retention-not-run = Enabled, but the sweep has not run yet (runs hourly; first tick is one hour after startup).
admin-settings-retention-last-run = Last run
admin-settings-retention-last-deleted = Last run deleted (rooms)
admin-settings-retention-total-deleted = Total deleted (lifetime)
admin-settings-retention-runs = Completed runs
admin-settings-retention-last-error = Last error:

## Slash commands
admin-slash-title = Slash commands
admin-slash-heading = Slash commands
admin-slash-intro-1 = Built-in commands ship with the app. Custom commands let you add your own:
admin-slash-intro-2 = substitutes
admin-slash-intro-3 = into a template;
admin-slash-intro-4 = POSTs the args as JSON to a URL and posts the response body. Admin-only commands can only be run by admins.
admin-slash-builtin-heading = Built-in
admin-slash-custom-heading = Custom
admin-slash-name-label = Name (no slash)
admin-slash-kind-label = Kind
admin-slash-description-label = Description
admin-slash-description-placeholder = Post the standup template
admin-slash-target-label = Target (template text or webhook URL)
admin-slash-admin-only = Admin-only
admin-slash-add-command = Add command
admin-slash-th-command = Command
admin-slash-th-kind = Kind
admin-slash-th-target = Target
admin-slash-th-admin-only = Admin-only
admin-slash-th-actions = Actions
admin-slash-yes = yes
admin-slash-no = no
admin-slash-delete = Delete
admin-slash-delete-confirm = Delete this custom command?
admin-slash-empty = No custom commands yet.

## User row
admin-userrow-role-user = user
admin-userrow-role-moderator = moderator
admin-userrow-role-admin = admin
admin-userrow-save = Save
admin-userrow-banned = Banned
admin-userrow-active = Active
admin-userrow-muted = Muted
admin-userrow-unlimited = unlimited
admin-userrow-unban = Unban
admin-userrow-ban = Ban
admin-userrow-unmute = Unmute
admin-userrow-mute = Mute
admin-userrow-delete = Delete
admin-userrow-delete-confirm-prefix = Permanently delete
admin-userrow-delete-confirm-suffix = This cannot be undone.
# LC-510: destructive-action confirmations + table empty states
admin-userrow-ban-confirm = Ban this user? They will be unable to sign in or post.
admin-userrow-mute-confirm = Mute this user? They will be unable to post.
admin-invites-revoke-confirm = Revoke this invite code? It can no longer be used.
admin-users-empty = No users yet.
admin-invites-empty = No invite codes yet.
admin-rooms-empty = No rooms yet.
admin-enclaves-empty = No enclaves yet.

## Users
admin-users-title = Users
admin-users-heading = Users
admin-users-th-username = Username
admin-users-th-role = Role
admin-users-th-status = Status
admin-users-th-storage = Storage (used / quota MiB)
admin-users-th-actions = Actions

# LC-207: bridge-avatar cache diagnostic page
admin-bridges-avatars-link = Avatar cache
admin-bridge-avatars-title = Bridge avatar cache
admin-bridge-avatars-heading = Bridge avatar cache
admin-bridge-avatars-back = Back to bridges
admin-bridge-avatars-intro = Diagnostics for the proxied foreign bridge-avatar cache. When a bridged user renders as initials instead of an avatar, the failed fetch and its reason appear below. Read-only; failed fetches are terminal in v2 (render falls back to initials).
admin-bridge-avatars-stat-total = Cached
admin-bridge-avatars-stat-ok = OK
admin-bridge-avatars-stat-pending = Pending
admin-bridge-avatars-stat-failed = Failed
admin-bridge-avatars-stat-bytes = Bytes on disk
admin-bridge-avatars-stat-oldest = Oldest last-seen
admin-bridge-avatars-stale-pending-prefix = Anomaly:
admin-bridge-avatars-stale-pending-suffix = pending fetch(es) older than 70 minutes (the orphan sweep should have flipped these to failed; the sweeper may be broken or restart-looping).
admin-bridge-avatars-failures-heading = Recent failed fetches
admin-bridge-avatars-th-host = Foreign host
admin-bridge-avatars-th-reason = Failure reason
admin-bridge-avatars-th-type = Type
admin-bridge-avatars-th-last-seen = Last referenced
admin-bridge-avatars-empty = No failed bridge-avatar fetches.
