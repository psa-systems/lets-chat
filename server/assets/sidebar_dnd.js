// Sidebar drag-and-drop reordering. Native HTML5 DnD, no library.
//
// Two reorder targets coexist:
//   - room rows (li[data-room-id]) inside per-category lists or the
//     uncategorized list. Drop fires PATCH /sidebar/categories/{cat_id}/positions
//     with the new ordering of room ids. Cross-category drag works
//     because the target endpoint upserts the assignment alongside
//     setting the position.
//   - category sections (section[data-category-id]) inside the nav.
//     Drop fires PATCH /sidebar/categories/positions with the new
//     ordering of category ids.
//
// Event delegation off `document` keeps the handlers attached when
// HTMX swaps the sidebar after each mutation. The currently dragged
// element is tracked in a closure-local variable so we don't have to
// roundtrip the id through dataTransfer (which is unreliable across
// browsers during dragover).
(function () {
  var dragged = null;

  document.addEventListener('dragstart', function (e) {
    var t = e.target.closest('[data-room-id], [data-category-id]');
    if (!t || !t.draggable) return;
    dragged = t;
    e.dataTransfer.effectAllowed = 'move';
    // Firefox refuses to start the drag without dataTransfer.setData.
    try { e.dataTransfer.setData('text/plain', t.dataset.roomId || t.dataset.categoryId || ''); } catch (_) {}
    t.classList.add('opacity-50');
  });

  document.addEventListener('dragend', function () {
    if (dragged) dragged.classList.remove('opacity-50');
    dragged = null;
  });

  // Match the drag with the corresponding drop container: room drags go
  // into [data-room-list], category drags into [data-category-list].
  function matchingList(target) {
    if (!dragged) return null;
    if (dragged.dataset.roomId !== undefined) return target.closest('[data-room-list]');
    if (dragged.dataset.categoryId !== undefined) return target.closest('[data-category-list]');
    return null;
  }

  document.addEventListener('dragover', function (e) {
    var list = matchingList(e.target);
    if (!list) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';

    // Insert the dragged element before the first sibling whose
    // midpoint is below the mouse Y. If none qualifies, append.
    var children = Array.prototype.filter.call(
      list.children,
      function (c) { return c !== dragged && c.matches('[data-room-id], [data-category-id]'); }
    );
    var insertBefore = null;
    for (var i = 0; i < children.length; i++) {
      var box = children[i].getBoundingClientRect();
      if (e.clientY < box.top + box.height / 2) {
        insertBefore = children[i];
        break;
      }
    }
    list.insertBefore(dragged, insertBefore);
  });

  document.addEventListener('drop', function (e) {
    var list = matchingList(e.target);
    if (!list || !dragged) return;
    e.preventDefault();
    // Uncategorized "All rooms" list opts in to drop by exposing
    // `data-uncategorize-url` instead of `data-positions-url`. A drop
    // there means "remove this room's category assignment", so fire a
    // DELETE per dropped room rather than the bulk positions PATCH.
    if (list.dataset.uncategorizeUrl) {
      unassign(list.dataset.uncategorizeUrl, dragged.dataset.roomId);
    } else {
      submitOrder(list);
    }
  });

  function unassign(baseUrl, roomId) {
    if (!roomId) return;
    fetch(baseUrl + '/' + encodeURIComponent(roomId), {
      method: 'DELETE',
      headers: { 'HX-Request': 'true' },
      credentials: 'same-origin'
    }).then(function (res) {
      if (!res.ok) return;
      return res.text();
    }).then(swapSidebar);
  }

  function swapSidebar(html) {
    if (!html) return;
    var tmp = document.createElement('div');
    tmp.innerHTML = html;
    var fresh = tmp.firstElementChild;
    if (fresh) {
      var old = document.getElementById('sidebar');
      if (old) old.replaceWith(fresh);
      if (window.htmx && window.htmx.process) window.htmx.process(fresh);
    }
  }

  // Read the current DOM order from the list and POST it back. The
  // server returns the rebuilt sidebar fragment, which HTMX swaps over
  // #sidebar via the existing OOB hook on ws/sidebar_update.html.
  function submitOrder(list) {
    var url = list.dataset.positionsUrl;
    if (!url) return;
    var key = list.dataset.categoryList !== undefined ? 'category' : 'room';
    var attr = key === 'category' ? 'categoryId' : 'roomId';
    var ids = Array.prototype.map.call(
      list.querySelectorAll('[data-' + (key === 'category' ? 'category-id' : 'room-id') + ']'),
      function (n) { return n.dataset[attr]; }
    );
    var body = 'ids=' + encodeURIComponent(ids.join(','));
    fetch(url, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded', 'HX-Request': 'true' },
      body: body,
      credentials: 'same-origin'
    }).then(function (res) {
      if (!res.ok) return;
      return res.text();
    }).then(swapSidebar);
  }
})();
