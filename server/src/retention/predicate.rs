//! Shared candidate-selection predicate.
//!
//! The retention sweep and the room-settings preview must agree on
//! exactly which messages count as deletion candidates. If the preview
//! says "this will delete approximately X messages" and the sweep then
//! deletes Y != X, the admin has consented to a blast radius that does
//! not match what happens. The fix is to define the predicate once and
//! have both call sites use it.
//!
//! Same pattern as `crate::scheduled::validation::check_delivery_gates`,
//! which unifies the HTTP-handler delivery gate with the dispatcher's
//! delivery gate so the two cannot drift.

/// Returns a SQL WHERE-clause fragment that selects retention-candidate
/// messages at the given cutoff. The fragment is intended to be embedded
/// inside a larger query that has already joined `messages AS m` (and,
/// for the sweep, `rooms AS r`).
///
/// Predicate shape (loose-correct, sweep-by-newest-reply):
///
/// 1. `m.parent_id IS NULL` - only thread roots and standalone messages
///    are candidates; replies survive or die with their root via the
///    `messages.parent_id ON DELETE CASCADE` self-reference.
/// 2. `m.created_at < cutoff` - strict less-than. A message at exactly
///    the cutoff survives one tick and dies the next.
/// 3. `NOT EXISTS (... c.parent_id = m.id AND c.created_at >= cutoff)` -
///    a thread with any direct reply newer than the cutoff is preserved.
///
/// # Limitation: direct-children-only recency check
///
/// The `NOT EXISTS` clause only inspects direct children of `m`. A thread
/// with a grandchild newer than the cutoff but whose direct child is past
/// the cutoff will still match this predicate, so the root (and the whole
/// descendant tree) gets purged when the sweep fires. SQLite cannot
/// express recursive descendant existence checks inside a `WHERE` clause
/// without a `WITH RECURSIVE` CTE at the outer query, which would break
/// the "predicate is a self-contained WHERE fragment" shape this function
/// guarantees. In practice the codebase's thread UI builds one-level-deep
/// trees (all replies are direct children of the root), so the limitation
/// rarely manifests. If grandchild-aware loose-correct becomes a hard
/// requirement, this predicate gets reshaped at the call site (the SELECT
/// query grows a recursive CTE) rather than here.
///
/// # SAFETY INVARIANT: `cutoff_expr` MUST be a compile-time literal
///
/// `cutoff_expr` is interpolated verbatim into the returned SQL string
/// with no escaping. Pass only SQL literal expressions you wrote
/// yourself, such as:
///
/// - `datetime('now', '-' || ? || ' days')` for callers that bind the
///   day-count as a `?` parameter (the preview path).
/// - `datetime('now', '-' || r.retention_days || ' days')` for callers
///   that read the day-count from a joined column (the sweep path).
///
/// **Never pass a user-formatted string here.** The day-count `N` flows
/// through bound `?` parameters in the caller's SQL, not through this
/// function. If a future caller is tempted to do
/// `candidate_predicate(&format!("datetime('now', '-{user_n} days')"))`,
/// they have just opened a SQL injection on `user_n`. The cure is to
/// pass a literal cutoff expression and bind `user_n` as a parameter.
///
/// Both current call sites (`sweep::run_retention_sweep` and
/// `sweep::count_candidates_for_room`) pass compile-time string literals.
/// Keep it that way.
pub fn candidate_predicate(cutoff_expr: &str) -> String {
    format!(
        "m.parent_id IS NULL \
         AND m.created_at < {cutoff_expr} \
         AND NOT EXISTS ( \
             SELECT 1 FROM messages c \
             WHERE c.parent_id = m.id \
               AND c.created_at >= {cutoff_expr} \
         )"
    )
}
