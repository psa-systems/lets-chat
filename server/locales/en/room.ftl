# LC-188: Room UI strings (source locale). Keys are kebab-case, prefixed
# "room-", no dots. Grouped by template area.

## Shared
room-save = Save
room-cancel = Cancel

## Composer
room-composer-send-failed = Could not send.
room-composer-retry = Retry
room-composer-attach-file = Attach file
room-composer-record-voice = Record voice message
room-composer-message-placeholder = Message
room-composer-create-poll = Create poll
room-composer-schedule-title = Schedule for later
room-composer-schedule-aria = Schedule message for later
room-composer-send-message = Send message
room-composer-preview = Preview
room-composer-emoji = Emoji
room-composer-edit = Write
room-composer-drop-file = Drop file to attach
room-composer-echo-sending = Sending...
room-composer-echo-discard = Discard
room-composer-format-bold = Bold
room-composer-format-italic = Italic
room-composer-format-code = Code
room-composer-format-link = Link
room-composer-format-strike = Strikethrough
room-composer-format-list = Bulleted list
room-composer-format-quote = Quote
room-composer-format-bold-ph = bold text
room-composer-format-italic-ph = italic text
room-composer-format-code-ph = code
room-composer-format-link-text-ph = text
room-composer-format-link-url-ph = url
room-composer-format-strike-ph = strikethrough
room-composer-format-list-ph = list item
room-composer-format-quote-ph = quote
# LC-323: #channel autocomplete popover.
room-channel-popover-aria = Channel suggestions

## Quote chip
room-quote-replying-to = Replying to
room-quote-cancel = Cancel quote-reply

## Description
room-description-label = Description (Markdown)
room-description-empty = No description set.
room-description-set = Set description
room-description-edit = Edit description

## Email inboxes
room-inboxes-title = Email inboxes
room-inboxes-heading = Email inboxes
room-inboxes-intro = An email inbox lets an external sender mail this room. Each inbox has a secret address; mail addressed to that address posts to this room as an "email" actor. The address is the credential - keep it secret; it is shown only once.
room-inboxes-created = Inbox created. Copy its address now - it won't be shown again.
room-inboxes-unavailable = Email inboxes are not configurable yet on this deployment.
room-inboxes-missing = Missing:
room-inboxes-restart = Once set, restart the server to enable email ingress.
room-inboxes-create-heading = Create an email inbox
room-inboxes-display-name = Display name
room-inboxes-name-placeholder = Pager
room-inboxes-avatar-url = Avatar URL (optional)
room-inboxes-create-button = Create inbox
room-inboxes-empty = No email inboxes yet.

## Feeds
room-feeds-title = Feeds
room-feeds-heading = Feeds
room-feeds-intro-1 = A read-only feed lets an external reader follow this room without logging in.
room-feeds-intro-2 = is an Atom feed of recent messages;
room-feeds-intro-3 = is a calendar of scheduled polls. The URL is the credential - keep it secret; it is shown only once. A feed stops working (returns 410) if it is revoked or if the person who created it loses access to the room.
room-feeds-created = Feed created. Copy its URL now - it won't be shown again.
room-feeds-unavailable-1 = Feeds require a server secret key
room-feeds-unavailable-2 = Set it and restart to create feeds.
room-feeds-type-label = Feed type
room-feeds-type-rss = RSS / Atom (messages)
room-feeds-type-ical = iCal (scheduled polls)
room-feeds-create-button = Create feed
room-feeds-table-type = Type
room-feeds-table-last-fetched = Last fetched
room-feeds-empty = No feeds yet.

## Webhooks
room-webhooks-title = Webhooks
room-webhooks-heading = Incoming webhooks
room-webhooks-intro-1 = An incoming webhook lets an external system POST a JSON body to a secret URL and have it appear in this room. Send
room-webhooks-intro-2 = to
room-webhooks-intro-3 = The URL is the credential - keep it secret; it is shown only once.
room-webhooks-created = Webhook created. Copy its URL now - it won't be shown again.
room-webhooks-unavailable-1 = Webhooks require a server secret key
room-webhooks-unavailable-2 = Set it and restart to create webhooks.
room-webhooks-create-heading = Create a webhook
room-webhooks-name-placeholder = Grafana
room-webhooks-create-button = Create webhook
room-webhooks-empty = No webhooks yet.

## Shared table headers / status
room-table-name = Name
room-table-created = Created
room-table-last-used = Last used
room-table-status = Status
room-table-actions = Actions
room-table-never = never
room-table-active = active
room-table-revoked = revoked
room-table-revoke = Revoke

## Files
room-files-empty = No files match this filter.
room-files-load-more = Load more

## History panel
room-history-heading = Edit history
room-history-close = Close history

## Room info
room-info-title-suffix = info
room-info-back-to-chat = Back to chat
room-info-tabs-aria = Room info tabs
room-info-tab-docs = Docs
room-info-tab-pinned = Pinned
room-info-tab-files = Files
room-info-description-heading = Description
room-info-wiki-heading = Wiki
room-info-wiki-empty = No wiki page yet.
room-info-wiki-last-edited = Last edited
room-info-wiki-on = on
room-info-wiki-by = by
room-info-wiki-create = Create wiki
room-info-wiki-edit = Edit wiki
room-info-pinned-heading = Pinned messages
room-info-pinned-empty = No pinned messages yet.
room-info-pinned-by = pinned by
room-info-pinned-on = on
room-info-files-heading = Files
room-info-files-filter = Filter
room-info-files-all = All files
room-info-files-images = Images
room-info-files-video = Video
room-info-files-audio = Audio
room-info-files-pdf = PDF
room-info-files-other = Other
room-info-files-empty = No files uploaded in this room yet.

# LC-321: per-room nickname (docs tab)
room-nickname-heading = Your nickname in this room
room-nickname-help = Shown on your messages in this room instead of your display name. Only applies here.
room-nickname-placeholder = e.g. Captain
room-nickname-clear = Clear

## Wiki (standalone view/edit)
room-wiki-label = Wiki (Markdown)

## Message row
room-msg-unread-divider = Unread messages
# LC-294: floating pill that scrolls back to the unread divider.
room-jump-unread-label = Unread
room-jump-unread-aria = Jump to first unread message
# LC-244: date separators in the message list.
room-day-today = Today
room-day-yesterday = Yesterday
room-msg-webhook-title = Posted by an incoming webhook
room-msg-webhook-badge = webhook
room-msg-email-title = Posted via email ingress
room-msg-email-badge = email
room-msg-bridge-title = Posted via a protocol bridge
room-msg-bridge-badge = via
room-msg-dm-title = Direct message
room-msg-bot-title = Bot account
room-msg-bot-badge = bot
room-msg-view-history = View edit history
room-msg-edited = (edited)
room-msg-show-more = Show more
room-msg-show-less = Show less
room-msg-reply = Reply
room-msg-thread = Thread
room-msg-copy-link = Copy link
room-msg-copy-text = Copy text
room-msg-copied = Copied
room-msg-unpin = Unpin
room-msg-pin = Pin
room-msg-unsave = Unsave
room-msg-save = Save
room-msg-remind = Remind
room-msg-mark-unread = Mark unread
room-msg-forward = Forward
room-msg-edit = Edit
room-msg-delete = Delete
room-msg-delete-confirm = Delete this message?
room-msg-quote-deleted = (quoted message was deleted)
room-msg-seen = Seen
# LC-302: accessible name for the one-tap quick-reaction hover bar.
room-quick-react-aria = Quick reactions

## Forward (LC-278)
room-forward-title = Forward message
room-forward-label = Forward message to a conversation
room-forward-rooms = Rooms
room-forward-dms = Direct messages
room-forward-filter = Filter conversations...
room-forward-close = Close
room-forward-empty = No conversations to forward to.
room-forward-confirm = Forwarded to
room-forward-done = Done
room-forward-attribution = Forwarded from

## Reply count
room-reply-singular = reply
room-reply-plural = replies

## Moderators
room-mods-title = Moderators
room-mods-heading = Moderators
room-mods-manage-webhooks = Manage incoming webhooks
room-mods-manage-inboxes = Manage email inboxes
room-mods-manage-feeds = Manage feeds
room-mods-intro = Grant a room-scoped Moderator or Admin role. Overrides only elevate; removing one returns the user to their org-wide role inside this room.
room-mods-policy-heading = Posting policy
room-mods-policy-intro = Controls who can post messages in this room. Reactions, pins, and edits of own messages are unaffected.
room-mods-who-can-post = Who can post
room-mods-policy-all = Everyone (default)
room-mods-policy-mods = Moderators only
room-mods-policy-admins = Admins only
room-mods-overrides-heading = Current overrides
room-mods-overrides-empty = No overrides yet.
room-mods-granted-by = granted by
room-mods-granted-on = on
room-mods-revoke-confirm-1 = Revoke
room-mods-revoke-confirm-2 = for
room-mods-grant-heading = Grant override
room-mods-grant-all-have = Every enclave member already has an override.
room-mods-member = Member
room-mods-role = Role
room-mods-role-moderator = Moderator (delete others' messages in this room)
room-mods-role-admin = Admin (Moderator + room settings, in this room)
room-mods-grant-button = Grant
room-mods-back-to = Back to

## Retention (moderators page)
room-retention-heading = Message retention
room-retention-intro-1 = Permanently delete messages older than N days.
room-retention-cannot-undo = This cannot be undone.
room-retention-intro-2 = Disabling retention later does not restore previously-deleted messages. Pinned messages are not exempt; copy important content to the
room-retention-wiki-link = room wiki
room-retention-intro-3 = to preserve it. Messages in threads with replies newer than the cutoff are preserved as a unit (active threads survive).
room-retention-enabled-1 = Currently
room-retention-enabled-word = enabled
room-retention-enabled-2 = : messages older than
room-retention-days = days
room-retention-enabled-3 = are deleted.
room-retention-disabled-1 = Currently
room-retention-disabled-word = disabled
room-retention-input-label = Retention (days, blank to disable)
room-retention-off = Off
room-retention-preview = Preview

## Retention preview
room-rpreview-setting-to = Setting retention to
room-rpreview-will-delete = will permanently delete
room-rpreview-on-next-sweep = messages on the next sweep.
room-rpreview-currently-set = Currently set to
room-rpreview-currently-disabled = Retention is currently disabled.
room-rpreview-permanent-word = Permanent.
room-rpreview-permanent-desc = Disabling retention later does NOT restore deleted messages.
room-rpreview-pinned-word = Pinned messages are NOT exempt.
room-rpreview-pinned-desc = Copy important content to the room wiki to preserve it.
room-rpreview-older-1 = Messages older than
room-rpreview-older-2 = days are deleted, except messages in threads with replies newer than
room-rpreview-older-3 = days (active threads are preserved as a unit).
room-rpreview-soft-1 = Soft-deleted, quarantined, and system messages older than
room-rpreview-soft-2 = days are deleted too.
room-rpreview-confirm-apply = Confirm and apply
room-rpreview-disable-1 = Disable retention. Currently set to
room-rpreview-disable-2 = ; previously-deleted messages will stay deleted.
room-rpreview-already-disabled = Retention is already disabled.
room-rpreview-confirm = Confirm

## Slash commands
room-slash-help-heading = Slash commands
room-slash-dismiss = Dismiss

## Notify dropdown
room-notify-unmuted = Unmuted
room-notify-unmuted-desc = All notifications.
room-notify-muted-mentions = Muted (mentions on)
room-notify-muted-mentions-desc = Only @-mentions notify.
room-notify-muted = Muted
room-notify-muted-desc = No notifications, even mentions.

## Pins page
room-pins-title-prefix = Pinned in
room-pins-back = Back

## Thread panel
room-thread-heading = Thread
room-thread-close = Close thread
# LC-310: thread following toggle.
room-thread-follow = Follow
room-thread-follow-title = Follow this thread to be notified of new replies
room-thread-following = Following
room-thread-following-title = You are following this thread. Click to stop notifications.
room-thread-reply-placeholder = Reply...
room-thread-send-reply = Send reply
