# Enclaves — Phase 3: UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Prereq:** Phases 1 and 2 merged onto `feat/enclaves`.

**Goal:** Restructure the layout into a Discord-style two-column chrome (enclave switcher + per-enclave sidebar), wire enclave landing/settings/discover/invitations templates fully, and remove the legacy single-column sidebar.

**Architecture:** A `ChromeView` view-model carrying both columns is built once per request. Every page template extends `layout.html`, which now renders `partials/enclave_switcher.html` and `partials/enclave_sidebar.html` side-by-side. The old `partials/sidebar.html` is deleted; `routes/mod.rs::load_sidebar` becomes `load_chrome(state, user, current_enclave)`.

**Tech Stack:** Askama, HTMX, Tailwind. No JavaScript beyond the existing HTMX extensions.

---

## File Structure (Phase 3)

| File | Purpose |
|---|---|
| `server/src/views/layout.rs` | Replace `SidebarRoom`/`SidebarPeer` with `ChromeView { switcher, current }`. |
| `server/src/routes/mod.rs` | `load_chrome(state, user, current_enclave)` returning `ChromeView`; old `load_sidebar` deleted. |
| `server/templates/layout.html` | Two-column flex chrome. |
| `server/templates/partials/enclave_switcher.html` | Narrow column with Home + enclave icons + plus button. |
| `server/templates/partials/enclave_sidebar.html` | Per-enclave room list or DM list (Home). |
| `server/templates/partials/sidebar.html` | DELETED. |
| `server/templates/enclave/page.html` | Full enclave landing. |
| `server/templates/enclave/settings.html` | Full settings page. |
| `server/templates/enclave/members.html` | Member-list partial used by settings. |
| `server/templates/enclave/member_row.html` | Single-member partial (HTMX swap target). |
| `server/templates/enclave/room_row.html` | Single-room partial. |
| `server/templates/enclave/discover.html` | Full discover page. |
| `server/templates/invitations/page.html` | Full invitations page. |
| `server/templates/invitations/row.html` | Single-invitation partial. |
| `server/templates/home/welcome.html` | Updated copy. |
| `server/templates/admin/rooms.html` | Grouped-by-enclave moderation view. |

---

## Task 1: `ChromeView` view-model + `load_chrome`

**Files:**
- Modify: `server/src/views/layout.rs`
- Modify: `server/src/routes/mod.rs`
- Update every existing page handler to call `load_chrome` instead of `load_sidebar`.

- [ ] **Step 1: Define the new view-model**

Replace `server/src/views/layout.rs` with:

```rust
pub struct EnclaveSwitcherEntry {
    pub id: Option<i64>,        // None = Home
    pub label: String,          // enclave name (or "Home")
    pub initial: String,        // first character for icon
    pub unread: i64,
    pub badge_invitations: i64, // only set on Home entry
    pub active: bool,
}

pub struct CurrentSidebar {
    pub kind: SidebarKind,
    pub rooms: Vec<RoomEntry>,
    pub peers: Vec<PeerEntry>,
    pub enclave_name: Option<String>,
    pub enclave_id: Option<i64>,
    pub can_manage: bool,
}

pub enum SidebarKind { Home, Enclave }

pub struct RoomEntry {
    pub id: i64,
    pub name: String,
    pub unread: i64,
    pub is_private: bool,
}

pub struct PeerEntry {
    pub id: String,
    pub username: String,
    pub unread: i64,
}

pub struct ChromeView {
    pub switcher: Vec<EnclaveSwitcherEntry>,
    pub current: CurrentSidebar,
}
```

- [ ] **Step 2: Implement `load_chrome`**

Replace the existing `load_sidebar` body in `server/src/routes/mod.rs` with:

```rust
pub(crate) async fn load_chrome(
    state: &AppState,
    user: &User,
    current_enclave: Option<i64>,
) -> Result<ChromeView, AppError> {
    let is_admin = user.role == "admin";
    let enclaves = db::enclave::list_enclaves_for_user(&state.chat, &user.id).await?;
    let invitations = db::enclave::list_invitations_for_user(&state.chat, &user.id).await?;
    let invitation_count = invitations.len() as i64;

    let dm_unreads: i64 = db::chat::list_dm_unread_counts(&state.chat, &user.id)
        .await?.iter().map(|(_, c)| *c).sum();

    let mut switcher: Vec<EnclaveSwitcherEntry> = Vec::with_capacity(enclaves.len() + 1);
    switcher.push(EnclaveSwitcherEntry {
        id: None,
        label: "Home".into(),
        initial: "H".into(),
        unread: dm_unreads,
        badge_invitations: invitation_count,
        active: current_enclave.is_none(),
    });
    for e in &enclaves {
        let unread: i64 = db::chat::list_room_unread_counts_in_enclave(&state.chat, e.id, &user.id, is_admin)
            .await?.iter().map(|(_, c)| *c).sum();
        switcher.push(EnclaveSwitcherEntry {
            id: Some(e.id),
            label: e.name.clone(),
            initial: e.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into()),
            unread,
            badge_invitations: 0,
            active: current_enclave == Some(e.id),
        });
    }

    let current = if let Some(eid) = current_enclave {
        let enclave = db::enclave::get_enclave(&state.chat, eid).await?
            .ok_or(AppError::NotFound)?;
        let m = db::enclave::get_membership(&state.chat, eid, &user.id).await?;
        let can_manage = crate::perms::enclave_can_manage(m.map(|x| x.role), &user.role);
        let rooms = db::chat::list_rooms_in_enclave(&state.chat, eid, &user.id, can_manage).await?;
        let unread_map: std::collections::HashMap<i64, i64> =
            db::chat::list_room_unread_counts_in_enclave(&state.chat, eid, &user.id, is_admin)
                .await?.into_iter().collect();
        let entries = rooms.into_iter().map(|r| RoomEntry {
            unread: *unread_map.get(&r.id).unwrap_or(&0),
            is_private: r.room_type == "private",
            id: r.id, name: r.name,
        }).collect();
        CurrentSidebar {
            kind: SidebarKind::Enclave,
            rooms: entries,
            peers: vec![],
            enclave_name: Some(enclave.name),
            enclave_id: Some(eid),
            can_manage,
        }
    } else {
        let dm_rooms = db::chat::list_user_dm_rooms(&state.chat, &user.id).await?;
        let dm_unread_map: std::collections::HashMap<i64, i64> =
            db::chat::list_dm_unread_counts(&state.chat, &user.id).await?.into_iter().collect();
        let mut peers = Vec::with_capacity(dm_rooms.len());
        for (room, peer_id) in &dm_rooms {
            if let Some(record) = db::auth::find_user_by_id(&state.auth, peer_id).await? {
                peers.push(PeerEntry {
                    id: record.id.clone(),
                    username: record.username.clone(),
                    unread: *dm_unread_map.get(&room.id).unwrap_or(&0),
                });
            }
        }
        CurrentSidebar {
            kind: SidebarKind::Home, rooms: vec![], peers,
            enclave_name: None, enclave_id: None, can_manage: false,
        }
    };

    Ok(ChromeView { switcher, current })
}
```

- [ ] **Step 3: Add `list_room_unread_counts_in_enclave` helper**

Append to `server/src/db/chat.rs` an enclave-scoped variant of the unread query (mirror `list_room_unread_counts` but with `AND r.enclave_id = ?`).

- [ ] **Step 4: Update every page struct**

Replace `sidebar_rooms`/`sidebar_peers` fields in all `views/*.rs` page structs with a single `chrome: &'a ChromeView`. Update every handler that previously called `load_sidebar` to call `load_chrome(&state, &user, current_enclave)` (current_enclave inferred from the route).

- [ ] **Step 5: Run; PASS**

Run: `./dev/cargo build -p lets-chat-server` (compile-only first, then tests).
Run: `./dev/cargo test -p lets-chat-server`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(enclaves): replace sidebar view-model with two-column ChromeView

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Layout shift to two-column chrome

**Files:**
- Modify: `server/templates/layout.html`
- Create: `server/templates/partials/enclave_switcher.html`
- Create: `server/templates/partials/enclave_sidebar.html`
- Delete: `server/templates/partials/sidebar.html`

- [ ] **Step 1: New layout**

Edit `server/templates/layout.html`. Replace `<aside id="sidebar">...</aside>` with:

```html
<aside id="chrome" class="flex flex-row{% if oob %} hx-swap-oob='outerHTML'{% endif %}">
  {% include "partials/enclave_switcher.html" %}
  {% include "partials/enclave_sidebar.html" %}
</aside>
```

- [ ] **Step 2: Switcher partial**

```html
<!-- server/templates/partials/enclave_switcher.html -->
<nav id="switcher" class="w-16 bg-slate-900 text-white flex flex-col items-center py-2 gap-2 overflow-y-auto">
  {% for e in chrome.switcher %}
  <a href="{% if let Some(id) = e.id %}/enclave/{{ id }}{% else %}/{% endif %}"
     class="relative w-10 h-10 rounded-lg flex items-center justify-center
            {% if e.active %}bg-blue-600{% else %}bg-slate-700 hover:bg-slate-600{% endif %}"
     title="{{ e.label }}">
    <span class="font-bold">{{ e.initial }}</span>
    {% if e.unread > 0 || e.badge_invitations > 0 %}
    <span class="absolute -top-1 -right-1 bg-red-500 text-white text-xs rounded-full px-1">
      {{ e.unread + e.badge_invitations }}
    </span>
    {% endif %}
  </a>
  {% endfor %}
  <a href="/enclaves/discover"
     class="w-10 h-10 rounded-lg bg-slate-700 hover:bg-slate-600 flex items-center justify-center font-bold mt-auto">
    +
  </a>
</nav>
```

- [ ] **Step 3: Per-enclave sidebar partial**

```html
<!-- server/templates/partials/enclave_sidebar.html -->
<aside id="sidebar" class="w-64 bg-slate-100 border-r border-slate-200 flex flex-col">
  <div class="p-4 border-b border-slate-200">
    <div class="font-semibold">
      {% match chrome.current.kind %}
        {% when SidebarKind::Home %}Home
        {% when SidebarKind::Enclave %}{% if let Some(n) = chrome.current.enclave_name %}{{ n }}{% endif %}
      {% endmatch %}
    </div>
    <div class="text-xs text-slate-500">{{ user.username }}</div>
  </div>
  <form class="p-2">
    <input name="q" placeholder="Search messages..."
           hx-get="/search{% if let Some(id) = chrome.current.enclave_id %}?enclave_id={{ id }}{% endif %}"
           hx-trigger="input changed delay:200ms, keyup[key=='Enter']"
           hx-target="#main" hx-swap="innerHTML" hx-push-url="true"
           class="w-full border rounded px-2 py-1 text-sm">
  </form>
  <nav class="flex-1 overflow-y-auto p-2 space-y-4">
    {% match chrome.current.kind %}
      {% when SidebarKind::Home %}
        <section>
          <h2 class="text-xs uppercase text-slate-500 px-2">Direct messages</h2>
          <ul class="mt-1">
            {% for peer in chrome.current.peers %}
            <li>
              <a href="/dm/{{ peer.id }}" class="flex items-center px-2 py-1 rounded hover:bg-slate-200">
                <span>@ {{ peer.username }}</span>
                {% let kind = "dm" %}
                {% let id = peer.id.clone() %}
                {% let unread = peer.unread %}
                {% include "partials/unread_badge.html" %}
              </a>
            </li>
            {% endfor %}
          </ul>
        </section>
        <section>
          <a href="/invitations" class="text-sm text-blue-600 hover:underline px-2">Invitations</a>
        </section>
      {% when SidebarKind::Enclave %}
        <section>
          <h2 class="text-xs uppercase text-slate-500 px-2">Open rooms</h2>
          <ul class="mt-1">
            {% for room in chrome.current.rooms %}{% if !room.is_private %}
            <li>
              <a href="/room/{{ room.id }}" class="flex items-center px-2 py-1 rounded hover:bg-slate-200">
                <span># {{ room.name }}</span>
                {% let kind = "room" %}
                {% let id = room.id.to_string() %}
                {% let unread = room.unread %}
                {% include "partials/unread_badge.html" %}
              </a>
            </li>
            {% endif %}{% endfor %}
          </ul>
        </section>
        <section>
          <h2 class="text-xs uppercase text-slate-500 px-2">Private rooms</h2>
          <ul class="mt-1">
            {% for room in chrome.current.rooms %}{% if room.is_private %}
            <li>
              <a href="/room/{{ room.id }}" class="flex items-center px-2 py-1 rounded hover:bg-slate-200">
                <span>🔒 {{ room.name }}</span>
                {% let kind = "room" %}
                {% let id = room.id.to_string() %}
                {% let unread = room.unread %}
                {% include "partials/unread_badge.html" %}
              </a>
            </li>
            {% endif %}{% endfor %}
          </ul>
        </section>
        {% if chrome.current.can_manage %}
        <section>
          {% if let Some(id) = chrome.current.enclave_id %}
          <a href="/enclave/{{ id }}/settings" class="text-sm text-blue-600 hover:underline px-2">Enclave settings</a>
          {% endif %}
        </section>
        {% endif %}
    {% endmatch %}
  </nav>
  <div class="p-2 border-t border-slate-200 text-sm flex flex-col gap-1">
    {% if user.role == "admin" || user.role == "moderator" %}
    <a href="{% if user.role == \"admin\" %}/admin/settings{% else %}/admin/users{% endif %}" class="text-red-600 hover:underline">{% if user.role == "admin" %}Admin{% else %}Moderate{% endif %}</a>
    {% endif %}
    <a href="/settings" class="text-slate-600 hover:underline">Settings</a>
    <a href="/logout" class="text-slate-600 hover:underline">Sign out</a>
  </div>
</aside>
```

- [ ] **Step 4: Delete the legacy partial**

```bash
rm server/templates/partials/sidebar.html
```

- [ ] **Step 5: Run `just verify` (smoke)**

Run: `just verify`
Expected: PASS (release binary boots, /login returns 200 with form).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(enclaves): two-column chrome layout (switcher + sidebar)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Full enclave landing template (`enclave/page.html`)

Replace the minimal Phase-2 stub with the full landing layout.

**Files:**
- Modify: `server/templates/enclave/page.html`
- Create: `server/templates/enclave/room_row.html`
- Create: `server/templates/enclave/member_row.html`

- [ ] **Step 1: Full landing page**

```html
{% extends "layout.html" %}
{% block title %}{{ enclave.name }} - lets-chat{% endblock %}
{% block main %}
<section class="p-6 max-w-4xl">
  <header class="mb-4">
    <h1 class="text-2xl font-semibold">{{ enclave.name }}</h1>
    {% if let Some(d) = enclave.description %}<p class="text-slate-600 mt-1">{{ d }}</p>{% endif %}
  </header>

  <div class="grid grid-cols-2 gap-6">
    <div>
      <h2 class="font-semibold flex items-center justify-between">
        Rooms
        {% if can_manage %}
        <button hx-get="/enclave/{{ enclave.id }}/rooms/new" hx-target="#main"
                class="text-sm bg-blue-600 text-white rounded px-2 py-0.5">+ Add</button>
        {% endif %}
      </h2>
      <ul id="enclave-rooms" class="mt-2 space-y-1">
        {% for r in rooms %}{% include "enclave/room_row.html" %}{% endfor %}
      </ul>
    </div>

    <div>
      <h2 class="font-semibold flex items-center justify-between">
        Members ({{ members.len() }})
        {% if can_manage %}
        <form method="post" action="/enclave/{{ enclave.id }}/invite" class="flex gap-1">
          <input name="username" placeholder="Invite by username"
                 class="border rounded px-2 py-0.5 text-sm" required>
          <button type="submit" class="text-sm bg-slate-200 rounded px-2 py-0.5">Invite</button>
        </form>
        {% endif %}
      </h2>
      <ul id="enclave-members" class="mt-2 space-y-1">
        {% for m in members %}{% include "enclave/member_row.html" %}{% endfor %}
      </ul>
    </div>
  </div>
</section>
{% endblock %}
```

- [ ] **Step 2: Row partials**

```html
<!-- server/templates/enclave/room_row.html -->
<li id="room-{{ r.id }}" class="flex items-center justify-between">
  <a href="/room/{{ r.id }}" class="hover:underline">
    {% if r.room_type == "private" %}🔒 {% else %}# {% endif %}{{ r.name }}
  </a>
  {% if can_manage %}
  <form method="post" action="/enclave/{{ enclave.id }}/rooms/{{ r.id }}/delete" class="inline">
    <button type="submit" class="text-xs text-red-600">Remove</button>
  </form>
  {% endif %}
</li>
```

```html
<!-- server/templates/enclave/member_row.html -->
<li id="member-{{ m.user_id }}" class="flex items-center justify-between">
  <span>{{ m.user_id }}</span>
  <span class="text-xs uppercase text-slate-500">{{ m.role.as_str() }}</span>
</li>
```

- [ ] **Step 3: Run `just verify`; PASS**

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(enclaves): full enclave landing with room + member rows

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Full settings template + members partial

**Files:**
- Modify: `server/templates/enclave/settings.html`
- Create: `server/templates/enclave/members.html`

- [ ] **Step 1: Settings template**

```html
{% extends "layout.html" %}
{% block title %}{{ enclave.name }} settings{% endblock %}
{% block main %}
<section class="p-6 max-w-2xl space-y-6">
  <h1 class="text-2xl font-semibold">{{ enclave.name }} settings</h1>

  <form method="post" action="/enclave/{{ enclave.id }}/edit" class="space-y-2">
    <label class="block">
      <span class="text-sm">Name</span>
      <input name="name" value="{{ enclave.name }}" class="w-full border rounded px-2 py-1" required>
    </label>
    <label class="block">
      <span class="text-sm">Description</span>
      <input name="description" value="{% if let Some(d) = enclave.description %}{{ d }}{% endif %}"
             class="w-full border rounded px-2 py-1">
    </label>
    <button type="submit" class="bg-blue-600 text-white rounded px-3 py-1">Save</button>
  </form>

  <form method="post" action="/enclave/{{ enclave.id }}/visibility" class="space-y-1">
    <h2 class="font-semibold">Visibility</h2>
    <p class="text-sm text-slate-600">{{ enclave.is_public }}</p>
    <button name="is_public" value="{% if enclave.is_public %}0{% else %}1{% endif %}" type="submit"
            class="bg-slate-200 rounded px-3 py-1">
      {% if enclave.is_public %}Make private{% else %}Make public{% endif %}
    </button>
  </form>

  <section class="space-y-2">
    <h2 class="font-semibold">Invite code</h2>
    {% if let Some(c) = enclave.invite_code %}
    <p class="font-mono text-sm bg-slate-100 rounded p-2 select-all">{{ c }}</p>
    <form method="post" action="/enclave/{{ enclave.id }}/invite-code/delete" class="inline">
      <button type="submit" class="text-sm text-red-600">Revoke</button>
    </form>
    {% endif %}
    <form method="post" action="/enclave/{{ enclave.id }}/invite-code">
      <button type="submit" class="bg-slate-200 rounded px-3 py-1">{% if enclave.invite_code.is_some() %}Rotate{% else %}Generate{% endif %}</button>
    </form>
  </section>

  <section>
    <h2 class="font-semibold">Members</h2>
    {% include "enclave/members.html" %}
  </section>

  {% if can_delete %}
  <form method="post" action="/enclave/{{ enclave.id }}/delete" class="pt-6 border-t">
    <button type="submit" class="text-red-600"
            onclick="return confirm('Permanently delete this enclave?')">Delete enclave</button>
  </form>
  {% endif %}
</section>
{% endblock %}
```

- [ ] **Step 2: Members partial**

```html
<!-- server/templates/enclave/members.html -->
<ul class="mt-2 space-y-1">
{% for m in members %}
<li class="flex items-center justify-between gap-2">
  <span>{{ m.user_id }}</span>
  <span class="text-xs uppercase text-slate-500 mr-auto">{{ m.role.as_str() }}</span>
  {% if can_delete %}
  {# can_delete implies owner privileges -> show role + kick controls #}
  {% if m.role.as_str() != "owner" %}
  <form method="post" action="/enclave/{{ enclave.id }}/members/{{ m.user_id }}/role" class="inline">
    <button name="role" value="{% if m.role.as_str() == \"admin\" %}member{% else %}admin{% endif %}"
            type="submit" class="text-xs bg-slate-200 rounded px-2 py-0.5">
      {% if m.role.as_str() == "admin" %}Demote{% else %}Promote{% endif %}
    </button>
  </form>
  <form method="post" action="/enclave/{{ enclave.id }}/members/{{ m.user_id }}/kick" class="inline">
    <button type="submit" class="text-xs text-red-600">Kick</button>
  </form>
  <form method="post" action="/enclave/{{ enclave.id }}/transfer" class="inline">
    <input type="hidden" name="new_owner_id" value="{{ m.user_id }}">
    <button type="submit" class="text-xs text-blue-600"
            onclick="return confirm('Transfer ownership to {{ m.user_id }}?')">Transfer</button>
  </form>
  {% endif %}
  {% endif %}
</li>
{% endfor %}
</ul>
```

- [ ] **Step 3: Run `just verify`; PASS**

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(enclaves): full settings + members partial with role/kick/transfer

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Discover + invitations templates

**Files:**
- Modify: `server/templates/enclave/discover.html`
- Modify: `server/templates/invitations/page.html`
- Create: `server/templates/invitations/row.html`

- [ ] **Step 1: Discover full template**

(Use the Phase-2 stub but expand: each row carries description, member-count, join button. Display the create form prominently above the public list.)

- [ ] **Step 2: Invitations full template + row partial**

```html
<!-- server/templates/invitations/row.html -->
<li id="invitation-{{ pair.0.id }}" class="flex items-center justify-between border rounded p-3">
  <div>
    <div class="font-medium">{{ pair.1.name }}</div>
    {% if let Some(d) = pair.1.description %}<div class="text-sm text-slate-600">{{ d }}</div>{% endif %}
    <div class="text-xs text-slate-500">Invited by {{ pair.0.invited_by }}</div>
  </div>
  <div class="flex gap-2">
    <form method="post" action="/invitations/{{ pair.0.id }}/accept">
      <button type="submit" class="bg-green-600 text-white rounded px-3 py-1 text-sm">Accept</button>
    </form>
    <form method="post" action="/invitations/{{ pair.0.id }}/decline">
      <button type="submit" class="bg-slate-200 rounded px-3 py-1 text-sm">Decline</button>
    </form>
  </div>
</li>
```

```html
{% extends "layout.html" %}
{% block title %}Invitations{% endblock %}
{% block main %}
<section class="p-6 max-w-2xl">
  <h1 class="text-2xl font-semibold">Pending invitations</h1>
  {% if invitations.is_empty() %}
  <p class="text-slate-600 mt-4">No invitations.</p>
  {% else %}
  <ul class="mt-4 space-y-2">
    {% for pair in invitations %}{% include "invitations/row.html" %}{% endfor %}
  </ul>
  {% endif %}
</section>
{% endblock %}
```

- [ ] **Step 3: Run `just verify`; PASS**

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(enclaves): full discover + invitations templates

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Welcome + admin/rooms updates

**Files:**
- Modify: `server/templates/home/welcome.html`
- Modify: `server/templates/admin/rooms.html`

- [ ] **Step 1: Welcome update**

Replace contents:

```html
{% extends "layout.html" %}
{% block title %}Home - lets-chat{% endblock %}
{% block main %}
<section class="p-6 max-w-2xl">
  <h1 class="text-2xl font-semibold">Welcome, {{ user.username }}</h1>
  <p class="mt-2 text-slate-600">Pick a DM from the sidebar, or
    <a href="/enclaves/discover" class="text-blue-600 hover:underline">create or join an enclave</a>
    to chat in rooms.</p>
</section>
{% endblock %}
```

- [ ] **Step 2: Admin rooms grouped by enclave**

Update `templates/admin/rooms.html` so the loop groups rows by enclave name. The handler in `routes/admin.rs::get_rooms` should join `rooms` to `enclaves` and pass a `Vec<(EnclaveName, Vec<RoomRow>)>`.

- [ ] **Step 3: Run `just verify`; PASS**

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(enclaves): welcome copy + admin/rooms grouped by enclave

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Tailwind rebuild + manual smoke

**Files:** none (artifacts).

- [ ] **Step 1: Rebuild CSS**

Run: `just build-css`

- [ ] **Step 2: Boot the dev server**

Run: `just dev-web-local` in a separate terminal. Manually exercise:

- Register / log in.
- `/` redirects to last room after a visit, else welcome.
- Create an enclave; verify icon appears in switcher.
- Add a room; verify it shows in sidebar.
- Generate invite code; copy it; log in as a second user; join via code; both users now see each other in the member list.
- Create a private room; non-members can't see it; add a non-member via the manage UI; they now see it.
- Public-discover toggle: flip is_public, see the enclave appear at `/enclaves/discover`.
- Direct invite a third user; that user sees pending invite at `/invitations`; accept; lands in the enclave.
- Transfer ownership to another member; previous owner becomes admin; new owner can now delete.
- Delete the enclave; switcher icon disappears for everyone immediately (after Phase 4 broadcasts) or on next page load.

- [ ] **Step 3: `just check`**

Run: `just check`
Expected: PASS.

- [ ] **Step 4: Commit (only if anything changed)**

If `tailwind-built.css` is gitignored, no commit needed. Otherwise:

```bash
git add -A
git commit -m "chore(enclaves): rebuild Tailwind output

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 3 Done

Sanity gates:

- All UI flows above work end-to-end manually.
- `just check` is green.
- `just verify` is green.
- Sidebar restored everywhere via `ChromeView`; old `partials/sidebar.html` deleted.

Next: Phase 4 (`2026-05-05-enclaves-phase4-realtime-cleanup.md`).
