# LC-188: Cadenas de la interfaz de enclaves (locale es). Las claves deben
# coincidir con las del locale fuente (en).

## Branding
enclave-branding-breadcrumb-settings = ajustes
enclave-branding-breadcrumb-current = Personalizacion
enclave-branding-heading-prefix = Personalizacion de
enclave-branding-saved = Guardado.
enclave-branding-intro-1 = Estos colores se aplican solo cuando un miembro visualiza una pagina dentro de este enclave (URLs que comienzan con
enclave-branding-intro-2 = ). La personalizacion para todo el despliegue definida en
enclave-branding-intro-3 = cubre todo lo demas. El logotipo se sirve desde
enclave-branding-primary-color = Color primario
enclave-branding-accent-color = Color de acento
enclave-branding-logo = Logotipo
enclave-branding-current-logo-alt = Logotipo actual
enclave-branding-logo-help = PNG / JPEG / WebP / GIF de hasta 1 MiB. Dejalo vacio para conservar el logotipo actual.
enclave-branding-login-heading-label = Titulo de la pagina de inicio de sesion (solo relevante cuando este enclave es el ambito activo)
enclave-branding-login-body-label = Cuerpo de la pagina de inicio de sesion
enclave-branding-save = Guardar

## Discover
enclave-discover-title = Descubrir enclaves
enclave-discover-heading = Enclaves
enclave-discover-subtitle = Grupos de salas de nivel superior. Crea uno, unete con un codigo o explora los publicos.
enclave-discover-create-heading = Crear
enclave-discover-name-placeholder = Nombre del enclave
enclave-discover-description-placeholder = Descripcion (opcional)
enclave-discover-create-button = Crear
enclave-discover-join-heading = Unirse con un codigo de invitacion
enclave-discover-join-code-placeholder = codigo de invitacion
enclave-discover-join-button = Unirse
enclave-discover-public-heading = Enclaves publicos
enclave-discover-empty = Aun no hay enclaves publicos. Haz publico el tuyo desde su pagina de ajustes.

## Member / invite search
enclave-search-no-matches = No hay personas coincidentes.
enclave-search-add = Anadir
enclave-search-in-group = En el grupo
enclave-search-not-in-enclave = No esta en el enclave
enclave-search-invite = Invitar
enclave-search-member = Miembro
enclave-search-invited = Invitado
enclave-search-you = (tu)

## Members list
enclave-members-settings-heading = Miembros

## Enclave empty-state placeholder + create-chat
enclave-public-badge = Publico
enclave-new-room-placeholder = sala-nueva
enclave-room-kind-text = texto
enclave-room-kind-voice = voz
enclave-room-type-public = publica
enclave-room-type-private = privada
enclave-add-room = Anadir sala
enclave-invite-placeholder = Invitar por nombre de usuario...
enclave-empty-no-rooms = Aun no hay chats.
enclave-empty-hint = Usa el + junto a SALAS en la barra lateral para anadir un chat.

## Enclave settings
enclave-settings-title = ajustes
enclave-settings-name = Nombre
enclave-settings-description = Descripcion
enclave-settings-save = Guardar
enclave-settings-visibility-heading = Visibilidad
enclave-settings-visibility-public = Publico - aparece en /enclaves/discover.
enclave-settings-visibility-private = Privado - para unirse se requiere una invitacion directa o un codigo de invitacion.
enclave-settings-make-private = Hacer privado
enclave-settings-make-public = Hacer publico
enclave-settings-invite-code-heading = Codigo de invitacion
enclave-settings-no-invite-code = No se ha generado ningun codigo de invitacion.
enclave-settings-rotate = Rotar
enclave-settings-generate = Generar
enclave-settings-rate-limit-heading = Anti-spam (mensajes por minuto)
enclave-settings-rate-limit-help = Maximo de mensajes que un miembro puede publicar por minuto en este enclave. 0 = usar el valor predeterminado del sitio. Se aplica ADEMAS del limite global del sitio; nunca lo relaja.
enclave-settings-rate-limit-burst = Rafaga (por minuto)
enclave-settings-rate-limit-save = Guardar
enclave-settings-rate-limit-status-global = Usando valor predeterminado del sitio
enclave-settings-rate-limit-status-active-prefix = Limite:
enclave-settings-rate-limit-status-active-suffix = por minuto
enclave-settings-coyote-heading = Modo Coyote (proteccion contra rafagas de bots)
enclave-settings-coyote-help = Cuando esta activo, un miembro que publica en 3 o mas salas de este enclave en 3 segundos se trata como un bot: se le expulsa de este enclave y se eliminan sus mensajes de las ultimas 24 horas en este enclave. Los gestores del enclave (propietarios y administradores) y los administradores del sitio estan exentos.
enclave-settings-coyote-on-label = Activado.
enclave-settings-coyote-on-text = Las rafagas de mensajes entre salas se expulsan automaticamente.
enclave-settings-coyote-off-label = Desactivado.
enclave-settings-coyote-off-text = Sin deteccion de rafagas.
enclave-settings-coyote-enable = Activar
enclave-settings-coyote-disable = Desactivar
enclave-settings-bans-heading = Usuarios expulsados
enclave-settings-bans-help = Usuarios vetados de este enclave (por ejemplo, por el Modo Coyote). Quita el veto para que puedan volver a unirse y publicar.
enclave-settings-bans-empty = No hay usuarios vetados.
enclave-settings-bans-unban = Quitar veto
enclave-settings-emojis-heading = Emojis personalizados
enclave-settings-emojis-help-1 = Escribe
enclave-settings-emojis-help-2 = en cualquier mensaje o reaccion. Visible para todos los miembros de este enclave.
enclave-settings-emojis-shared-label = Compartidos:
enclave-settings-emojis-shared-text = estos emojis funcionan en otros enclaves y en los mensajes directos.
enclave-settings-emojis-private-label = Privados:
enclave-settings-emojis-private-text = estos emojis solo funcionan en las salas de este enclave.
enclave-settings-emojis-stop-sharing = Dejar de compartir
enclave-settings-emojis-share-globally = Compartir globalmente
enclave-settings-emojis-empty = Aun no hay emojis personalizados.
enclave-settings-emoji-delete = Eliminar
enclave-settings-emoji-delete-confirm-prefix = Eliminar :
enclave-settings-emoji-delete-confirm-suffix = :?
enclave-settings-emoji-shortcode-label = Codigo corto (letras minusculas, digitos, guion bajo; 2-32 caracteres)
enclave-settings-emoji-image-label = Imagen (png, gif, webp; hasta 256 KiB)
enclave-settings-add-emoji = Anadir emoji
enclave-settings-groups-heading = Grupos de usuarios
enclave-settings-groups-help-1 = Crea un grupo con nombre;
enclave-settings-groups-help-2 = en una sala se expande a una mencion de cada miembro. Por enclave.
enclave-settings-groups-empty = Aun no hay grupos.
enclave-settings-group-member-singular = miembro
enclave-settings-group-member-plural = miembros
enclave-settings-group-delete = Eliminar
enclave-settings-group-delete-confirm-prefix = Eliminar @
enclave-settings-group-delete-confirm-suffix = ?
enclave-settings-group-add-member = Anadir miembro
enclave-settings-group-search-placeholder = Buscar por nombre de usuario o nombre visible...
enclave-settings-create-group-placeholder = nombre-de-grupo
enclave-settings-create-group = Crear grupo
enclave-settings-branding-heading = Personalizacion
enclave-settings-branding-link = Editar el logotipo, los colores y el texto de la pagina de inicio de sesion
enclave-settings-branding-text-1 = para este enclave. Estas anulaciones se aplican solo cuando un miembro visualiza una URL dentro de
enclave-settings-branding-text-2 = ; en cualquier otro lugar se recurre a la personalizacion para todo el despliegue.
enclave-settings-delete = Eliminar enclave
enclave-settings-delete-confirm = Eliminar permanentemente este enclave y todas sus salas?

## Settings members list (controls)
enclave-settings-member-demote = Degradar
enclave-settings-member-promote = Ascender
enclave-settings-member-kick = Expulsar
enclave-settings-member-transfer = Transferir
enclave-settings-member-transfer-confirm-prefix = Transferir la propiedad a
enclave-settings-member-transfer-confirm-suffix = ?
