// LC-81: jump-to-unread keybind + sidebar button.
//
// Alt+Shift+ArrowDown navigates to the next sidebar room with an
// unread badge (chip / pill rendered by partials/unread_badge.html).
// Alt+Shift+ArrowUp navigates to the previous. Wraps at both ends.
// A `[data-lc-jump-unread]` button in the sidebar fires the same
// action via click.
(function () {
  function unreadRows() {
    // Sidebar room rows wrap their /room/N link in an <a>; DM rows do
    // the same with /dm/peer-id. An unread badge is a <span> whose
    // text content is non-empty (the badge partial renders an empty
    // span when unread == 0, so .textContent is the discriminator).
    var anchors = document.querySelectorAll(
      '#sidebar a[href^="/room/"], #sidebar a[href^="/dm/"]'
    );
    var out = [];
    anchors.forEach(function (a) {
      var badge =
        a.querySelector('[id^="unread-room-"], [id^="unread-dm-"]');
      if (badge && badge.textContent.trim() !== '') out.push(a);
    });
    return out;
  }

  function jump(dir) {
    var rows = unreadRows();
    if (rows.length === 0) return;
    var here = window.location.pathname;
    var idx = -1;
    for (var i = 0; i < rows.length; i++) {
      if (rows[i].getAttribute('href') === here) { idx = i; break; }
    }
    var next;
    if (dir > 0) {
      next = idx === -1 ? rows[0] : rows[(idx + 1) % rows.length];
    } else {
      next = idx === -1
        ? rows[rows.length - 1]
        : rows[(idx - 1 + rows.length) % rows.length];
    }
    if (next) next.click();
  }

  document.addEventListener('keydown', function (e) {
    if (!e.altKey || !e.shiftKey) return;
    if (e.key === 'ArrowDown') { e.preventDefault(); jump(1); }
    if (e.key === 'ArrowUp') { e.preventDefault(); jump(-1); }
  });

  document.addEventListener('click', function (e) {
    var btn = e.target.closest('[data-lc-jump-unread]');
    if (!btn) return;
    e.preventDefault();
    jump(1);
  });
})();
