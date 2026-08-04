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
room-composer-record-clip = Record video clip
room-composer-message-placeholder = Message
room-composer-create-poll = Create poll
room-composer-create-event = Create event
room-composer-gif = Add a GIF
# LC-506: admin-only hint when an AI action is shown but the LLM is unconfigured
ai-needs-setup = AI is not configured. Set LETS_CHAT_LLM_URL to enable.
# LC-511: admin-only hint when the GIF button is shown but Giphy is unconfigured
gif-needs-setup = GIF picker is not configured. Set LETS_CHAT_GIPHY_API_KEY to enable.
room-composer-schedule-title = Schedule for later
room-composer-schedule-aria = Schedule message for later
room-composer-ttl-label = Self-destruct timer
room-composer-ttl-off = No timer
room-composer-ttl-5m = 5m
room-composer-ttl-1h = 1h
room-composer-ttl-1d = 1d
room-composer-ttl-7d = 7d
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
room-composer-format-strike-ph = strikethrough
room-composer-format-list-ph = list item
room-composer-format-quote-ph = quote
# LC-332: composer character counter. %n% is replaced client-side with the count
# (a plain token, not a Fluent placeable, so the brace-free text round-trips).
room-composer-chars-remaining = %n% left
room-composer-chars-over = %n% over limit
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
room-info-tab-docs = About
room-info-tab-pinned = Pinned
room-info-tab-files = Files
room-info-tab-prefs = Preferences
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
room-msg-more = More actions
room-msg-copy-link = Copy link
room-msg-copy-text = Copy text
room-msg-copied = Copied
room-msg-unpin = Unpin
room-msg-pin = Pin
room-msg-unsave = Unsave
room-msg-save = Save
# LC-490: acknowledgement / required-read
room-msg-require-ack = Require acknowledgement
room-msg-unrequire-ack = Clear acknowledgement
room-ack-required = Acknowledgement required
room-ack-button = Acknowledge
room-ack-done = Acknowledged
room-ack-count-prefix = Acknowledged by
room-msg-remind = Remind
room-msg-mark-unread = Mark unread
# LC-528: read a message aloud (browser speech synthesis)
room-msg-read-aloud = Read aloud
room-msg-stop-reading = Stop reading
room-msg-forward = Forward
room-msg-report = Report
room-msg-edit = Edit
room-msg-delete = Delete
# LC-486: inline translation
room-msg-translate = Translate
room-msg-translated-to = Translated to
room-msg-show-original = Show original
room-msg-delete-confirm = Delete this message?
room-msg-quote-deleted = (quoted message was deleted)
room-msg-seen = Seen
# LC-302: accessible name for the one-tap quick-reaction hover bar.
room-quick-react-aria = Quick reactions

## Report (LC-334): report-a-message modal + site-admin review queue.
report-title = Report message
report-close = Close
report-intro = Tell the moderators what's wrong with this message.
report-category-legend = Reason
report-category-spam = Spam
report-category-harassment = Harassment
report-category-inappropriate = Inappropriate content
report-category-other = Other
report-note-label = Additional details
report-note-placeholder = Add any details (optional)
report-cancel = Cancel
report-submit = Submit report
report-thanks = Thanks. The moderators have been notified.
report-done = Done
report-queue-title = Reports
report-queue-heading = Reported messages
report-queue-empty = No open reports.
report-jump = Jump to message
report-row-author = Author:
report-row-in = in
report-row-note = Note:
report-row-reporter = Reported by
report-action-resolve = Resolve
report-action-dismiss = Dismiss
report-message-deleted = (message deleted)
report-room-dm = Direct message

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
room-mods-policy-intro = Controls who can post messages in this room. Restricting posting turns this into an announcement channel: everyone can still read and react, but only the chosen roles can post. Reactions, pins, and edits of own messages are unaffected.
room-mods-who-can-post = Who can post
room-mods-policy-all = Everyone (default)
room-mods-policy-mods = Moderators only (announcement)
room-mods-policy-admins = Admins only (announcement)
# LC-480: announcement-channel banner + read-only composer notice
room-announce-label = Announcement
room-announce-admins = Only admins can post in this channel.
room-announce-mods = Only moderators can post in this channel.
room-readonly-hint = You can still react to messages.
# LC-489: group-room "Seen by" avatar stack label.
room-seen-by = Seen by
# LC-476: broadcast-mention (@here / @channel) policy
room-broadcast-policy-heading = Broadcast mentions
room-broadcast-policy-intro = Controls who can use @here and @channel to notify many people at once. Restrict it to curb noise; normal @mentions are unaffected.
room-broadcast-who = Who can use @here / @channel
room-broadcast-all = Everyone (default)
room-broadcast-mods = Moderators only
room-broadcast-admins = Admins only
# LC-492: in-channel AI assistant toggle (room manage page).
room-assistant-heading = AI assistant
room-assistant-intro = Let members ask the room's AI assistant questions with
room-assistant-unconfigured = No LLM is configured on this server yet, so the assistant will not answer until an operator sets LETS_CHAT_LLM_URL.
room-assistant-on-label = On.
room-assistant-on-text = Members can use /ask in this room.
room-assistant-off-label = Off.
room-assistant-off-text = /ask is disabled in this room.
# LC-494: stage-mode toggle (room manage page).
room-stage-heading = Stage mode
room-stage-intro = Turn this room into a stage with speakers and listeners, where people request to speak.
room-stage-audio-note = Roles and request-to-speak ship now; large-audience audio needs a media server (coming soon).
room-stage-on-label = On.
room-stage-on-text = This room shows the stage roster.
room-stage-off-label = Off.
room-stage-off-text = Stage mode is disabled in this room.
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
room-slash-no-match = No matching commands

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
# LC-460: thread/reply affordance labels
room-thread-view = View thread
room-quote-jump = Jump to the replied message
# LC-461: thread panel parent reference + composer cue
room-thread-replies-to = Replies to
room-thread-composer-cue = Replying in thread
room-thread-close = Close thread
# LC-310: thread following toggle.
room-thread-follow = Follow
room-thread-follow-title = Follow this thread to be notified of new replies
room-thread-following = Following
room-thread-following-title = You are following this thread. Click to stop notifications.
room-thread-mute = Mute
room-thread-mute-title = Mute this thread to stop notifications for its new replies
room-thread-muted = Muted
room-thread-muted-title = This thread is muted. Click to resume notifications.
room-thread-reply-placeholder = Reply...
room-thread-send-reply = Send reply
room-info-danger-heading = Danger zone
room-info-delete-room = Delete room
room-info-delete-room-confirm = Delete this room and all of its messages? This cannot be undone.

# LC-454: room Manage page + Preferences tab + delete-room confirm
room-prefs-nickname-desc = Shown on your messages in this room instead of your display name. Only applies here.
room-manage-heading = Manage
room-manage-integrations-heading = Integrations
room-manage-integrations-desc = Connect this room to incoming webhooks, email inboxes, and RSS/Atom feeds.
room-manage-roles-heading = Roles & overrides
room-policy-saved = Posting policy saved
room-nickname-saved = Nickname saved
room-nickname-cleared = Nickname cleared
room-retention-pill-disabled = Disabled
room-retention-preview-note = Preview only - nothing is deleted until you review and apply.
room-delete-room-desc = Permanently delete this room and all its messages, files, and history. This cannot be undone.
room-delete-confirm-prefix = Type
room-delete-confirm-phrase = delete this room
room-delete-confirm-suffix = to confirm.

# LC-484: AI "catch me up" summaries (threads + channel)
summary-catch-up-heading = Catch me up
summary-unread-suffix = unread messages
summary-recent-scope = Summarize recent activity
summary-generate = Generate summary
summary-regenerate = Regenerate
summary-disclaimer = AI-generated from recent messages. May be incomplete.
# LC-650: shared "AI is working" pending labels shown while an LLM request runs.
ai-generating = Generating...
ai-summarizing = Summarizing...
ai-translating = Translating...
ai-working-slow = Still working - the local model can take a few seconds.
# LC-654: first stage of the catch-me-up skeleton status, before "Summarizing...".
ai-reading-messages = Reading recent messages...
# LC-655: staged status for the composer writing assistant.
ai-thinking = Thinking...
ai-writing = Writing...
room-thread-summarize = Summarize

# LC-495: workflow automations (room manage page)
room-automations-heading = Automations
room-automations-intro = No-code rules that run when something happens in this room.
room-automations-empty = No automations yet. Add one below.
room-automations-on = On
room-automations-off = Off
room-automations-trigger-message = When a message contains
room-automations-trigger-reaction = When someone reacts with
room-automations-any = anything
room-automations-then-post = then post a message.
room-automations-disable = Disable
room-automations-enable = Enable
room-automations-delete = Delete
room-automations-delete-confirm = Delete this automation?
room-automations-new-heading = New automation
room-automations-name-label = Name (optional)
room-automations-name-ph = e.g. Welcome bot
room-automations-when-label = When
room-automations-match-label = Match (leave blank for any)
room-automations-match-ph = keyword or emoji
room-automations-do-label = Then post
room-automations-do-ph = The message to post
room-automations-vars-help = Placeholders, each wrapped in curly braces: user, text, emoji.
room-automations-create = Create automation

## LC-527: follow-up tasks (from a call transcript's action items)
followup-card-title = Follow-up tasks
followup-create-button = Create follow-up tasks
followup-created = Follow-up tasks posted to the room.
followup-claim = Claim
followup-assigned-you = You
followup-toggle-aria = Toggle done
followup-done-suffix = done

## LC-529: reaction highlights recap
room-highlights-title = Highlights
room-highlights-window = Most-reacted in the past 7 days
room-highlights-empty = No reactions in the past 7 days yet.
room-highlights-jump = Jump to message
room-highlights-reactions = reactions
partials-room-highlights = Highlights
partials-room-highlights-title = Reaction highlights

## LC-526: kudos / recognition
kudos-recognition-prefix = 🎉 Kudos to
kudos-title = Kudos leaderboard
kudos-window = Kudos in the past 30 days
kudos-empty = No kudos yet. Give some with /kudos @user.
kudos-most-appreciated = Most appreciated
kudos-most-generous = Most generous
kudos-hint = Give kudos in any channel with /kudos @user <reason>.
sidebar-link-kudos = Kudos
stats-title = Your stats
stats-subtitle = A recap of your activity so far
stats-messages-sent = Messages sent
stats-active-days = Active days
stats-kudos-received = Kudos received
stats-reactions-received = Reactions received
stats-reactions-given = Reactions given
stats-member-since = Member since
stats-top-channels = Top channels
stats-top-channels-empty = No channel activity yet.
stats-hint = Only you can see your stats.
sidebar-link-stats = Your stats

## LC-532: composer AI writing assistant
compose-assist-tip = AI writing assistant
# Still used by the suggested-reply panel (dismiss the chips, no draft change).
compose-assist-dismiss = Dismiss
# LC-655: mode menu + preview panel (Accept / Regenerate / Discard).
compose-assist-menu-label = Rewrite with AI
compose-assist-heading = AI assistant
compose-assist-accept = Accept
compose-assist-regenerate = Regenerate
compose-assist-discard = Discard
compose-assist-action-rephrase = Improve writing
compose-assist-action-grammar = Fix grammar
compose-assist-action-concise = Make shorter
compose-assist-action-friendly = Friendlier tone
compose-assist-action-formal = More formal

## LC-548: AI suggested replies
room-msg-suggest-reply = Suggest reply
suggest-reply-heading = Tap a draft to add it to your message

## LC-549: semantic / related-message search
room-msg-related = Find related
related-heading = Related messages
related-subheading = Ranked by meaning, not keywords
related-empty = No closely related messages found.

## LC-534: per-channel slowmode
room-slowmode-heading = Slowmode
room-slowmode-intro = Limit how often each member can post in this channel. Moderators are exempt.
room-slowmode-label = Cooldown between posts
room-slowmode-off = Off
room-slowmode-5s = 5 seconds
room-slowmode-10s = 10 seconds
room-slowmode-30s = 30 seconds
room-slowmode-60s = 1 minute
room-slowmode-saved = Slowmode updated.

## LC-568: Details panel (right column, below the thread panel)
room-details-title = Details
# LC-576: collapse/expand chevron for the Details panel (mirrors the left sidebar's)
room-details-toggle = Toggle details panel
room-details-created = Created
room-details-members = Members
room-details-notifications = Notifications
room-details-pinned = Pinned
room-details-pinned-yes = Yes
room-details-pinned-no = No
room-details-leave = Leave room
room-details-leave-confirm = Leave this room? You'll need a new invite to rejoin.
