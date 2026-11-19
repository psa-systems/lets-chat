# Spaces

How lets chat organizes people and rooms above the single-room level.

## Enclaves

An enclave is a workspace: a bounded set of rooms, a default room, a member list, and a settings page. Lets chat supports multiple enclaves per deployment; a user may belong to several.

Each enclave has its own:
- Default room (where new members land on first visit)
- Welcome behavior: an automatic message posted when a new member joins, with an optional animated wave reaction, both configurable in enclave Settings > General
- Member list with invite and remove actions
- Sidebar categories

## User groups

A user group is a named set of members. Mentioning the group with `@group-name` notifies every member. Groups are managed from the enclave settings and can be used anywhere `@user` works.

## Room wikis

Every room has a Markdown wiki page. Moderators and admins edit it inline. The wiki is a good place for the room's shared context: on-call rota, meeting agenda, links to the authoritative source for a topic.

## Sidebar categories, starred rooms, mutes

- Categories: operators arrange rooms into collapsible groups in the sidebar.
- Starred rooms: personal favorites, surfaced at the top.
- Room and DM mute: hides the row's notification badge and silences its sounds. Mentions still land in the notifications inbox.

## Status and presence

- Custom status: a short text (`In a meeting`, `Heads-down until 3`) that shows next to your name.
- Presence: Discord-style. The server marks you idle after 10 minutes of inactivity and offline when a background tab goes silent. A foreground tab or the desktop app is treated as online.

## Handles

Every user has an `@handle`. The initial handle is derived by the SSO provisioning flow from the user's identity claims. The handle picker lets each user pick a different one, subject to the same validation rules.

Handle changes keep working for existing mentions: a mention links to the user id, not the string.
