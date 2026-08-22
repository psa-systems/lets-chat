# LC-188: Catálogo en español de la página de invitaciones. Debe definir los
# mismos ids que en/invitations.ftl.

## Invitations
invitations-page-title = Invitaciones
invitations-pending-heading = Invitaciones pendientes
# LC-772: estado vacío más amable (antes "No hay invitaciones.").
invitations-empty = No hay invitaciones pendientes
invitations-invited-by = Invitado por
invitations-accept = Aceptar
invitations-decline = Rechazar
# LC-772: recuento de miembros en cada tarjeta; con formas de plural ($count).
invitations-members = { $count ->
    [one] { $count } miembro
   *[other] { $count } miembros
    }
# LC-772: etiqueta de reserva cuando ya no se puede resolver quien invita.
invitations-inviter-unknown = Un miembro
# LC-772: confirmación para que rechazar no sea un error de un solo clic.
invitations-decline-confirm = ¿Rechazar esta invitación?
