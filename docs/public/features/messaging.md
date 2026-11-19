# Messaging

The core of lets chat. Rooms, direct messages, and the surfaces around them.

## Rooms

Public rooms are discoverable; any member of the enclave can join. Private and invite-only rooms are hidden from the room list and require an admin or moderator to add a member. Private rooms get a dedicated add-member picker in the room header.

A room has a name, a topic, a wiki page, a member list, notification defaults, and a message history. Deleting a room is an admin action; it tombstones the room rather than erasing it so inbound references do not dangle.

## Direct messages

A direct message is a two-person room with no name and no topic. Starting a DM uses the user picker at the top of the sidebar. DMs are not browsable; they appear in the DM list of each participant.

Group DMs are a separate surface: up to a configurable maximum members, no topic, no wiki, otherwise a private room.

## Message editing and history

Any message you sent can be edited. An edited message shows an edit indicator and a drawer reveals the edit history with timestamps. Admins see the history for any user's messages.

Deleting a message is permitted for the author and for moderators and admins. The row remains with a deletion marker so thread continuity is not broken.

## Reactions, mentions, pins, bookmarks

- Emoji reactions, including custom emoji admins upload.
- `@user` and `@group` mentions push a notification to the target.
- `@room` is a broadcast mention; use it sparingly and only in rooms that allow it.
- Pinning a message keeps it in a per-room pinned list. Bookmarks are personal, not shared.

## Reading state

Messages carry read receipts. The right rail shows who has seen the most recent message in a room. Typing indicators show while any member has an open composer.

Message grouping collapses several messages from the same sender within a short window into one block.

## Composing

The composer accepts Markdown (bold, italic, links, lists, code), inline emoji shortcodes, slash commands, and file and image uploads. A link in the composer unfurls into a preview card that senders can edit or dismiss before sending.

## Slash commands

`/help` lists the commands available in the current room. Common ones include `/me`, `/shrug`, `/remind`, `/topic`, `/invite`, `/leave`.

## Polls

A poll is a message with options and a vote count. Everyone in the room can vote; the author sees the per-option tally and can close the poll.

## Scheduled messages

Pick a future time in the composer and the message delivers then. Pending sends live at `/scheduled`, where you can edit or cancel.

## Message reminders

Any message you want to come back to can be reminded. "Remind me about this" lets you choose a time; at that time the server posts a direct notification linking back to the message.
