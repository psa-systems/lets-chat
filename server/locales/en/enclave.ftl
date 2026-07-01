# LC-188: Enclave UI strings (source locale). Keys are kebab-case, prefixed
# "enclave-", no dots. Grouped by template area.

## Branding
enclave-branding-breadcrumb-settings = settings
enclave-branding-breadcrumb-current = Branding
enclave-branding-heading-prefix = Branding for
enclave-branding-saved = Saved.
enclave-branding-intro-1 = These colors apply only when a member views a page under this enclave (URLs starting with
enclave-branding-intro-2 = ). The deployment-wide branding set under
enclave-branding-intro-3 = covers everything else. The logo is served from
enclave-branding-primary-color = Primary color
enclave-branding-accent-color = Accent color
enclave-branding-logo = Logo
enclave-branding-current-logo-alt = Current logo
enclave-branding-logo-help = PNG / JPEG / WebP / GIF up to 1 MiB. Leave empty to keep the current logo.
enclave-branding-login-heading-label = Login page heading (only relevant when this enclave is the active scope)
enclave-branding-login-body-label = Login page body
enclave-branding-save = Save

## Discover
enclave-discover-title = Discover enclaves
enclave-discover-heading = Enclaves
enclave-discover-subtitle = Top-level groups of rooms. Create one, join via code, or browse public ones.
enclave-discover-create-heading = Create
enclave-discover-name-placeholder = Enclave name
enclave-discover-description-placeholder = Description (optional)
enclave-discover-create-button = Create
enclave-discover-join-heading = Join by invite code
enclave-discover-join-code-placeholder = invite code
enclave-discover-join-button = Join
enclave-discover-public-heading = Public enclaves
enclave-discover-empty = No public enclaves yet. Set yours public from its settings page.

## Member / invite search
enclave-search-no-matches = No matching people.
enclave-search-add = Add
enclave-search-in-group = In group
enclave-search-not-in-enclave = Not in enclave
enclave-search-invite = Invite
enclave-search-member = Member
enclave-search-invited = Invited
enclave-search-you = (you)

## Members list
enclave-members-settings-heading = Members

## Enclave empty-state placeholder + create-chat
enclave-public-badge = Public
enclave-new-room-placeholder = new-room
enclave-room-kind-text = text
enclave-room-kind-voice = voice
enclave-room-type-public = public
enclave-room-type-private = private
enclave-add-room = Add room
enclave-invite-placeholder = Invite by username...
# LC-516: add a bot (which cannot accept invitations) directly to the enclave.
enclave-add-bot-title = Add a bot
enclave-add-bot-button = Add bot
enclave-empty-no-rooms = No chats yet.
enclave-empty-hint = Use the + next to ROOMS in the sidebar to add a chat.

## Enclave settings
enclave-settings-title = settings
enclave-settings-name = Name
enclave-settings-description = Description
enclave-settings-save = Save
enclave-settings-visibility-heading = Visibility
enclave-settings-visibility-public = Public - listed on /enclaves/discover.
enclave-settings-visibility-private = Private - join requires direct invite or invite code.
enclave-settings-make-private = Make private
enclave-settings-make-public = Make public
enclave-settings-invite-code-heading = Invite code
enclave-settings-no-invite-code = No invite code generated.
enclave-settings-rotate = Rotate
enclave-settings-generate = Generate
enclave-settings-rate-limit-heading = Anti-spam (messages per minute)
enclave-settings-rate-limit-help = Max messages a member can post per minute in this enclave. 0 = use the site default. Layers in addition to the site rate limit; never relaxes it.
enclave-settings-rate-limit-burst = Burst (per minute)
enclave-settings-rate-limit-save = Save
enclave-settings-rate-limit-status-global = Using site default
enclave-settings-rate-limit-status-active-prefix = Limit:
enclave-settings-rate-limit-status-active-suffix = per minute
enclave-settings-coyote-heading = Coyote Mode (bot burst protection)
enclave-settings-coyote-help = When on, a member who posts in 3 or more rooms of this enclave within 3 seconds is treated as a bot: banned from this enclave and their messages from the last 24 hours in this enclave are removed. Enclave managers (owners and admins) and site admins are exempt.
enclave-settings-coyote-on-label = On.
enclave-settings-coyote-on-text = Cross-room message bursts are auto-banned.
enclave-settings-coyote-off-label = Off.
enclave-settings-coyote-off-text = No burst detection.
enclave-settings-coyote-enable = Enable
enclave-settings-coyote-disable = Disable
enclave-settings-shame-heading = Shame tags (community moderation, prototype)
enclave-settings-shame-help = When on, members can flag messages (spam / abusive / off-topic / misinformation). A message enough members flag as spam or abusive is hidden behind a click-through. Moderators can override.
enclave-settings-shame-on-label = On.
enclave-settings-shame-on-text = Members can flag messages; heavily-flagged ones hide.
enclave-settings-shame-off-label = Off.
enclave-settings-shame-off-text = No community flagging.
enclave-settings-shame-enable = Enable
enclave-settings-shame-disable = Disable
shame-flag = Flag
shame-flag-title = Flag this message
shame-hidden-prefix = Hidden - flagged as
shame-hidden-moderator = Hidden by a moderator
shame-show-anyway = Show anyway
shame-mod-force-hide = Force hide
shame-mod-force-show = Force show
shame-mod-clear = Clear override
enclave-settings-bans-heading = Banned users
enclave-settings-bans-help = Users banned from this enclave (e.g. by Coyote Mode). Unban to let them rejoin and post again.
enclave-settings-bans-empty = No banned users.
enclave-settings-bans-unban = Unban
enclave-settings-emojis-heading = Custom emojis
enclave-settings-emojis-help-1 = Type
enclave-settings-emojis-help-2 = in any message or reaction. Visible to every member of this enclave.
enclave-settings-emojis-shared-label = Shared:
enclave-settings-emojis-shared-text = these emojis resolve in other enclaves and DMs.
enclave-settings-emojis-private-label = Private:
enclave-settings-emojis-private-text = these emojis only resolve in this enclave's rooms.
enclave-settings-emojis-stop-sharing = Stop sharing
enclave-settings-emojis-share-globally = Share globally
enclave-settings-emojis-empty = No custom emojis yet.
enclave-settings-emoji-delete = Delete
enclave-settings-emoji-delete-confirm-prefix = Delete :
enclave-settings-emoji-delete-confirm-suffix = :?
enclave-settings-emoji-shortcode-label = Shortcode (lowercase letters, digits, underscore; 2-32 chars)
enclave-settings-emoji-image-label = Image (png, gif, webp; up to 256 KiB)
enclave-settings-add-emoji = Add emoji
enclave-settings-groups-heading = User groups
enclave-settings-groups-help-1 = Create a named group;
enclave-settings-groups-help-2 = in a room expands to a mention of every member. Per-enclave.
enclave-settings-groups-empty = No groups yet.
enclave-settings-group-member-singular = member
enclave-settings-group-member-plural = members
enclave-settings-group-delete = Delete
enclave-settings-group-delete-confirm-prefix = Delete @
enclave-settings-group-delete-confirm-suffix = ?
enclave-settings-group-add-member = Add member
enclave-settings-group-search-placeholder = Search by username or display name...
enclave-settings-create-group-placeholder = group-name
enclave-settings-create-group = Create group
enclave-settings-branding-heading = Branding
enclave-settings-branding-link = Edit logo, colors, and login-page copy
enclave-settings-branding-text-1 = for this enclave. These overrides apply only when a member is viewing a URL under
enclave-settings-branding-text-2 = ; everywhere else falls back to the deployment-wide branding.
enclave-settings-delete = Delete enclave
enclave-settings-delete-confirm = Permanently delete this enclave and all rooms?

## Settings members list (controls)
enclave-settings-member-demote = Demote
enclave-settings-member-promote = Promote
enclave-settings-member-kick = Kick
enclave-settings-member-transfer = Transfer
enclave-settings-member-transfer-confirm-prefix = Transfer ownership to
enclave-settings-member-transfer-confirm-suffix = ?

# LC-463: enclave settings redesign (tabs, copy, switches, danger zone, feedback)
enclave-settings-back = Back to enclave
enclave-settings-tabs-aria = Enclave settings sections
enclave-settings-tab-general = General
enclave-settings-tab-members = Members
enclave-settings-tab-moderation = Moderation
enclave-settings-tab-custom = Customization
enclave-settings-tab-danger = Danger zone
enclave-settings-visibility-public-label = Public enclave
enclave-settings-copy = Copy
enclave-settings-copied = Copied
enclave-settings-emoji-choose = Choose image
enclave-settings-no-file = No file chosen
enclave-settings-emoji-formats = PNG, GIF, or WebP, up to 256 KiB.
enclave-settings-emoji-err-type = Use a PNG, GIF, or WebP image.
enclave-settings-emoji-err-size = Image must be under 256 KiB.
enclave-settings-delete-heading = Delete this enclave
enclave-settings-delete-desc = Permanently delete this enclave and all of its rooms, messages, and history. This cannot be undone.
enclave-delete-confirm-prefix = Type
enclave-delete-confirm-phrase = delete this enclave
enclave-delete-confirm-suffix = to confirm.
enclave-settings-member-kick-confirm-prefix = Kick
enclave-settings-member-kick-confirm-suffix = from this enclave?
enclave-flash-saved = Saved
enclave-flash-rotated = Invite code rotated
enclave-flash-added = Added
enclave-flash-created = Created
enclave-flash-deleted = Deleted
enclave-flash-updated = Updated
enclave-flash-unbanned = User unbanned
enclave-flash-transferred = Ownership transferred

# LC-469: branding page redesign
enclave-branding-back = Back to settings
enclave-branding-colors-heading = Colors
enclave-branding-login-card-heading = Login page
enclave-branding-preview-label = Preview
enclave-branding-choose-logo = Choose image
enclave-branding-no-file = No file chosen
enclave-branding-logo-err-type = Use a PNG, JPEG, WebP, or GIF image.
enclave-branding-logo-err-size = Image must be under 1 MiB.
enclave-branding-saving = Saving...
