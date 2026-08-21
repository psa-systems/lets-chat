# LC-188: Invitations page chrome. Keep in sync with es/invitations.ftl.

## Invitations
invitations-page-title = Invitations
invitations-pending-heading = Pending invitations
# LC-772: friendlier empty state (was "No invitations.").
invitations-empty = No pending invitations
invitations-invited-by = Invited by
invitations-accept = Accept
invitations-decline = Decline
# LC-772: live member count on each invite card; plural-aware ($count).
invitations-members = { $count ->
    [one] { $count } member
   *[other] { $count } members
    }
# LC-772: fallback label when the inviter's account can no longer be resolved.
invitations-inviter-unknown = A member
# LC-772: confirm prompt so declining is not a one-click mistake.
invitations-decline-confirm = Decline this invitation?
