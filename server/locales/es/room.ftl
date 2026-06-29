# LC-188: Cadenas de la interfaz de salas (locale es). Las claves deben
# coincidir con las del locale fuente (en).

## Shared
room-save = Guardar
room-cancel = Cancelar

## Composer
room-composer-send-failed = No se pudo enviar.
room-composer-retry = Reintentar
room-composer-attach-file = Adjuntar archivo
room-composer-record-voice = Grabar mensaje de voz
room-composer-message-placeholder = Mensaje
room-composer-create-poll = Crear encuesta
room-composer-create-event = Crear evento
room-composer-gif = Anadir un GIF
# LC-506: aviso solo para administradores cuando una accion de IA aparece sin LLM configurado
ai-needs-setup = La IA no esta configurada. Define LETS_CHAT_LLM_URL para habilitarla.
# LC-511: aviso solo para administradores cuando el boton de GIF aparece sin Giphy configurado
gif-needs-setup = El selector de GIF no esta configurado. Define LETS_CHAT_GIPHY_API_KEY para habilitarlo.
room-composer-schedule-title = Programar para mas tarde
room-composer-schedule-aria = Programar mensaje para mas tarde
room-composer-send-message = Enviar mensaje
room-composer-preview = Vista previa
room-composer-emoji = Emoji
room-composer-edit = Escribir
room-composer-drop-file = Suelta el archivo para adjuntarlo
room-composer-echo-sending = Enviando...
room-composer-echo-discard = Descartar
room-composer-format-bold = Negrita
room-composer-format-italic = Cursiva
room-composer-format-code = Código
room-composer-format-link = Enlace
room-composer-format-strike = Tachado
room-composer-format-list = Lista con viñetas
room-composer-format-quote = Cita
room-composer-format-bold-ph = texto en negrita
room-composer-format-italic-ph = texto en cursiva
room-composer-format-code-ph = código
room-composer-format-link-text-ph = texto
room-composer-format-link-url-ph = url
room-composer-format-strike-ph = tachado
room-composer-format-list-ph = elemento de lista
room-composer-format-quote-ph = cita
# LC-332: contador de caracteres del redactor. %n% se reemplaza en el cliente con
# la cantidad (token simple, no un placeable de Fluent, para evitar las llaves).
room-composer-chars-remaining = quedan %n%
room-composer-chars-over = %n% de más
# LC-323: popover de autocompletado de #canal.
room-channel-popover-aria = Sugerencias de canales

## Quote chip
room-quote-replying-to = Respondiendo a
room-quote-cancel = Cancelar respuesta citada

## Description
room-description-label = Descripcion (Markdown)
room-description-empty = No hay descripcion definida.
room-description-set = Definir descripcion
room-description-edit = Editar descripcion

## Email inboxes
room-inboxes-title = Buzones de correo
room-inboxes-heading = Buzones de correo
room-inboxes-intro = Un buzon de correo permite que un remitente externo escriba a esta sala. Cada buzon tiene una direccion secreta; el correo enviado a esa direccion se publica en esta sala como un actor de "correo". La direccion es la credencial: mantenla en secreto; solo se muestra una vez.
room-inboxes-created = Buzon creado. Copia su direccion ahora: no se volvera a mostrar.
room-inboxes-unavailable = Los buzones de correo aun no se pueden configurar en este despliegue.
room-inboxes-missing = Falta:
room-inboxes-restart = Una vez definido, reinicia el servidor para habilitar la entrada de correo.
room-inboxes-create-heading = Crear un buzon de correo
room-inboxes-display-name = Nombre visible
room-inboxes-name-placeholder = Localizador
room-inboxes-avatar-url = URL del avatar (opcional)
room-inboxes-create-button = Crear buzon
room-inboxes-empty = Aun no hay buzones de correo.

## Feeds
room-feeds-title = Canales
room-feeds-heading = Canales
room-feeds-intro-1 = Un canal de solo lectura permite que un lector externo siga esta sala sin iniciar sesion.
room-feeds-intro-2 = es un canal Atom de mensajes recientes;
room-feeds-intro-3 = es un calendario de encuestas programadas. La URL es la credencial: mantenla en secreto; solo se muestra una vez. Un canal deja de funcionar (devuelve 410) si se revoca o si la persona que lo creo pierde el acceso a la sala.
room-feeds-created = Canal creado. Copia su URL ahora: no se volvera a mostrar.
room-feeds-unavailable-1 = Los canales requieren una clave secreta del servidor
room-feeds-unavailable-2 = Definela y reinicia para crear canales.
room-feeds-type-label = Tipo de canal
room-feeds-type-rss = RSS / Atom (mensajes)
room-feeds-type-ical = iCal (encuestas programadas)
room-feeds-create-button = Crear canal
room-feeds-table-type = Tipo
room-feeds-table-last-fetched = Ultima obtencion
room-feeds-empty = Aun no hay canales.

## Webhooks
room-webhooks-title = Webhooks
room-webhooks-heading = Webhooks entrantes
room-webhooks-intro-1 = Un webhook entrante permite que un sistema externo envie (POST) un cuerpo JSON a una URL secreta y que aparezca en esta sala. Envia
room-webhooks-intro-2 = a
room-webhooks-intro-3 = La URL es la credencial: mantenla en secreto; solo se muestra una vez.
room-webhooks-created = Webhook creado. Copia su URL ahora: no se volvera a mostrar.
room-webhooks-unavailable-1 = Los webhooks requieren una clave secreta del servidor
room-webhooks-unavailable-2 = Definela y reinicia para crear webhooks.
room-webhooks-create-heading = Crear un webhook
room-webhooks-name-placeholder = Grafana
room-webhooks-create-button = Crear webhook
room-webhooks-empty = Aun no hay webhooks.

## Shared table headers / status
room-table-name = Nombre
room-table-created = Creado
room-table-last-used = Ultimo uso
room-table-status = Estado
room-table-actions = Acciones
room-table-never = nunca
room-table-active = activo
room-table-revoked = revocado
room-table-revoke = Revocar

## Files
room-files-empty = No hay archivos que coincidan con este filtro.
room-files-load-more = Cargar mas

## History panel
room-history-heading = Historial de ediciones
room-history-close = Cerrar historial

## Room info
room-info-title-suffix = info
room-info-back-to-chat = Volver al chat
room-info-tabs-aria = Pestanas de informacion de la sala
room-info-tab-docs = Acerca de
room-info-tab-pinned = Fijados
room-info-tab-files = Archivos
room-info-tab-prefs = Preferencias
room-info-description-heading = Descripcion
room-info-wiki-heading = Wiki
room-info-wiki-empty = Aun no hay pagina wiki.
room-info-wiki-last-edited = Ultima edicion
room-info-wiki-on = el
room-info-wiki-by = por
room-info-wiki-create = Crear wiki
room-info-wiki-edit = Editar wiki
room-info-pinned-heading = Mensajes fijados
room-info-pinned-empty = Aun no hay mensajes fijados.
room-info-pinned-by = fijado por
room-info-pinned-on = el
room-info-files-heading = Archivos
room-info-files-filter = Filtrar
room-info-files-all = Todos los archivos
room-info-files-images = Imagenes
room-info-files-video = Video
room-info-files-audio = Audio
room-info-files-pdf = PDF
room-info-files-other = Otros
room-info-files-empty = Aun no se han subido archivos en esta sala.

# LC-321: apodo por sala (pestaña Docs)
room-nickname-heading = Tu apodo en esta sala
room-nickname-help = Se muestra en tus mensajes de esta sala en lugar de tu nombre visible. Solo se aplica aquí.
room-nickname-placeholder = p. ej. Capitán
room-nickname-clear = Borrar

## Wiki (standalone view/edit)
room-wiki-label = Wiki (Markdown)

## Message row
room-msg-unread-divider = Mensajes no leidos
# LC-294: floating pill that scrolls back to the unread divider.
room-jump-unread-label = No leidos
room-jump-unread-aria = Saltar al primer mensaje no leido
# LC-244: date separators in the message list.
room-day-today = Hoy
room-day-yesterday = Ayer
room-msg-webhook-title = Publicado por un webhook entrante
room-msg-webhook-badge = webhook
room-msg-email-title = Publicado mediante entrada de correo
room-msg-email-badge = correo
room-msg-bridge-title = Publicado mediante puente de protocolo
room-msg-bridge-badge = via
room-msg-dm-title = Mensaje directo a
room-msg-bot-title = Cuenta de bot
room-msg-bot-badge = bot
room-msg-view-history = Ver historial de ediciones
room-msg-edited = (editado)
room-msg-show-more = Mostrar más
room-msg-show-less = Mostrar menos
room-msg-reply = Responder
room-msg-thread = Hilo
room-msg-more = Más acciones
room-msg-copy-link = Copiar enlace
room-msg-copy-text = Copiar texto
room-msg-copied = Copiado
room-msg-unpin = Desfijar
room-msg-pin = Fijar
room-msg-unsave = Quitar guardado
room-msg-save = Guardar
# LC-490: confirmacion de lectura
room-msg-require-ack = Requerir confirmacion
room-msg-unrequire-ack = Quitar confirmacion
room-ack-required = Confirmacion requerida
room-ack-button = Confirmar
room-ack-done = Confirmado
room-ack-count-prefix = Confirmado por
room-msg-remind = Recordar
room-msg-mark-unread = Marcar como no leído
room-msg-forward = Reenviar
room-msg-report = Denunciar
room-msg-edit = Editar
room-msg-delete = Eliminar
# LC-486: traduccion en linea
room-msg-translate = Traducir
room-msg-translated-to = Traducido a
room-msg-show-original = Mostrar original
room-msg-delete-confirm = ¿Eliminar este mensaje?
room-msg-quote-deleted = (el mensaje citado fue eliminado)
room-msg-seen = Visto
# LC-302: accessible name for the one-tap quick-reaction hover bar.
room-quick-react-aria = Reacciones rápidas

## Report (LC-334): modal de denuncia de mensajes + cola de revisión del administrador.
report-title = Denunciar mensaje
report-close = Cerrar
report-intro = Indica a los moderadores qué problema tiene este mensaje.
report-category-legend = Motivo
report-category-spam = Spam
report-category-harassment = Acoso
report-category-inappropriate = Contenido inapropiado
report-category-other = Otro
report-note-label = Detalles adicionales
report-note-placeholder = Añade detalles (opcional)
report-cancel = Cancelar
report-submit = Enviar denuncia
report-thanks = Gracias. Se ha notificado a los moderadores.
report-done = Hecho
report-queue-title = Denuncias
report-queue-heading = Mensajes denunciados
report-queue-empty = No hay denuncias abiertas.
report-jump = Ir al mensaje
report-row-author = Autor:
report-row-in = en
report-row-note = Nota:
report-row-reporter = Denunciado por
report-action-resolve = Resolver
report-action-dismiss = Descartar
report-message-deleted = (mensaje eliminado)
report-room-dm = Mensaje directo

## Forward (LC-278)
room-forward-title = Reenviar mensaje
room-forward-label = Reenviar el mensaje a una conversación
room-forward-rooms = Salas
room-forward-dms = Mensajes directos
room-forward-filter = Filtrar conversaciones...
room-forward-close = Cerrar
room-forward-empty = No hay conversaciones a las que reenviar.
room-forward-confirm = Reenviado a
room-forward-done = Hecho
room-forward-attribution = Reenviado de

## Reply count
room-reply-singular = respuesta
room-reply-plural = respuestas

## Moderators
room-mods-title = Moderadores
room-mods-heading = Moderadores
room-mods-manage-webhooks = Gestionar webhooks entrantes
room-mods-manage-inboxes = Gestionar buzones de correo
room-mods-manage-feeds = Gestionar canales
room-mods-intro = Otorga un rol de Moderador o Administrador a nivel de sala. Las excepciones solo elevan; quitar una devuelve al usuario a su rol global dentro de esta sala.
room-mods-policy-heading = Politica de publicacion
room-mods-policy-intro = Controla quien puede publicar mensajes en esta sala. Restringir la publicacion la convierte en un canal de anuncios: todos pueden seguir leyendo y reaccionando, pero solo los roles elegidos pueden publicar. Las reacciones, los fijados y la edicion de mensajes propios no se ven afectados.
room-mods-who-can-post = Quien puede publicar
room-mods-policy-all = Todos (predeterminado)
room-mods-policy-mods = Solo moderadores (anuncios)
room-mods-policy-admins = Solo administradores (anuncios)
# LC-480: aviso de canal de anuncios + nota de solo lectura del redactor
room-announce-label = Anuncios
room-announce-admins = Solo los administradores pueden publicar en este canal.
room-announce-mods = Solo los moderadores pueden publicar en este canal.
room-readonly-hint = Aun puedes reaccionar a los mensajes.
# LC-489: etiqueta de la pila de avatares "Visto por" en salas de grupo.
room-seen-by = Visto por
# LC-476: politica de menciones masivas (@here / @channel)
room-broadcast-policy-heading = Menciones masivas
room-broadcast-policy-intro = Controla quien puede usar @here y @channel para notificar a muchas personas a la vez. Restringelo para reducir el ruido; las menciones normales no se ven afectadas.
room-broadcast-who = Quien puede usar @here / @channel
room-broadcast-all = Todos (predeterminado)
room-broadcast-mods = Solo moderadores
room-broadcast-admins = Solo administradores
# LC-492: interruptor del asistente de IA en el canal (pagina de gestion).
room-assistant-heading = Asistente de IA
room-assistant-intro = Permite que los miembros hagan preguntas al asistente de IA de la sala con
room-assistant-unconfigured = Aun no hay un LLM configurado en este servidor, asi que el asistente no respondera hasta que un operador defina LETS_CHAT_LLM_URL.
room-assistant-on-label = Activado.
room-assistant-on-text = Los miembros pueden usar /ask en esta sala.
room-assistant-off-label = Desactivado.
room-assistant-off-text = /ask esta desactivado en esta sala.
# LC-494: interruptor del modo escenario (pagina de gestion).
room-stage-heading = Modo escenario
room-stage-intro = Convierte esta sala en un escenario con oradores y oyentes, donde la gente pide la palabra.
room-stage-audio-note = Los roles y la peticion de palabra ya estan; el audio para audiencias grandes necesita un servidor de medios (proximamente).
room-stage-on-label = Activado.
room-stage-on-text = Esta sala muestra la lista del escenario.
room-stage-off-label = Desactivado.
room-stage-off-text = El modo escenario esta desactivado en esta sala.
room-mods-overrides-heading = Excepciones actuales
room-mods-overrides-empty = Aun no hay excepciones.
room-mods-granted-by = otorgado por
room-mods-granted-on = el
room-mods-revoke-confirm-1 = ¿Revocar
room-mods-revoke-confirm-2 = para
room-mods-grant-heading = Otorgar excepcion
room-mods-grant-all-have = Todos los miembros del enclave ya tienen una excepcion.
room-mods-member = Miembro
room-mods-role = Rol
room-mods-role-moderator = Moderador (eliminar mensajes de otros en esta sala)
room-mods-role-admin = Administrador (Moderador + ajustes de la sala, en esta sala)
room-mods-grant-button = Otorgar
room-mods-back-to = Volver a

## Retention (moderators page)
room-retention-heading = Retencion de mensajes
room-retention-intro-1 = Eliminar permanentemente los mensajes con mas de N dias.
room-retention-cannot-undo = Esto no se puede deshacer.
room-retention-intro-2 = Desactivar la retencion mas adelante no restaura los mensajes ya eliminados. Los mensajes fijados no estan exentos; copia el contenido importante a la
room-retention-wiki-link = wiki de la sala
room-retention-intro-3 = para conservarlo. Los mensajes de hilos con respuestas mas recientes que el limite se conservan como una unidad (los hilos activos sobreviven).
room-retention-enabled-1 = Actualmente
room-retention-enabled-word = activada
room-retention-enabled-2 = : los mensajes con mas de
room-retention-days = dias
room-retention-enabled-3 = se eliminan.
room-retention-disabled-1 = Actualmente
room-retention-disabled-word = desactivada
room-retention-input-label = Retencion (dias, en blanco para desactivar)
room-retention-off = Desactivado
room-retention-preview = Vista previa

## Retention preview
room-rpreview-setting-to = Establecer la retencion en
room-rpreview-will-delete = eliminara permanentemente
room-rpreview-on-next-sweep = mensajes en el proximo barrido.
room-rpreview-currently-set = Actualmente establecido en
room-rpreview-currently-disabled = La retencion esta actualmente desactivada.
room-rpreview-permanent-word = Permanente.
room-rpreview-permanent-desc = Desactivar la retencion mas adelante NO restaura los mensajes eliminados.
room-rpreview-pinned-word = Los mensajes fijados NO estan exentos.
room-rpreview-pinned-desc = Copia el contenido importante a la wiki de la sala para conservarlo.
room-rpreview-older-1 = Los mensajes con mas de
room-rpreview-older-2 = dias se eliminan, excepto los mensajes de hilos con respuestas mas recientes que
room-rpreview-older-3 = dias (los hilos activos se conservan como una unidad).
room-rpreview-soft-1 = Los mensajes eliminados de forma reversible, en cuarentena y del sistema con mas de
room-rpreview-soft-2 = dias tambien se eliminan.
room-rpreview-confirm-apply = Confirmar y aplicar
room-rpreview-disable-1 = Desactivar la retencion. Actualmente establecido en
room-rpreview-disable-2 = ; los mensajes ya eliminados permaneceran eliminados.
room-rpreview-already-disabled = La retencion ya esta desactivada.
room-rpreview-confirm = Confirmar

## Slash commands
room-slash-help-heading = Comandos de barra
room-slash-dismiss = Descartar
room-slash-no-match = No hay comandos coincidentes

## Notify dropdown
room-notify-unmuted = Sin silenciar
room-notify-unmuted-desc = Todas las notificaciones.
room-notify-muted-mentions = Silenciado (menciones activas)
room-notify-muted-mentions-desc = Solo notifican las menciones con @.
room-notify-muted = Silenciado
room-notify-muted-desc = Sin notificaciones, ni siquiera menciones.

## Pins page
room-pins-title-prefix = Fijados en
room-pins-back = Volver

## Thread panel
room-thread-heading = Hilo
# LC-460: etiquetas de hilo/respuesta
room-thread-view = Ver hilo
room-quote-jump = Ir al mensaje respondido
# LC-461: referencia al mensaje padre del panel de hilo + indicador del redactor
room-thread-replies-to = Respuestas a
room-thread-composer-cue = Respondiendo en el hilo
room-thread-close = Cerrar hilo
# LC-310: thread following toggle.
room-thread-follow = Seguir
room-thread-follow-title = Sigue este hilo para recibir avisos de nuevas respuestas
room-thread-following = Siguiendo
room-thread-following-title = Estás siguiendo este hilo. Haz clic para dejar de recibir avisos.
room-thread-reply-placeholder = Responder...
room-thread-send-reply = Enviar respuesta
room-info-danger-heading = Zona de peligro
room-info-delete-room = Eliminar sala
room-info-delete-room-confirm = ¿Eliminar esta sala y todos sus mensajes? Esta acción no se puede deshacer.

# LC-454: pagina Gestionar de la sala + pestana Preferencias + confirmacion de borrado
room-prefs-nickname-desc = Se muestra en tus mensajes de esta sala en lugar de tu nombre visible. Solo se aplica aqui.
room-manage-heading = Gestionar
room-manage-integrations-heading = Integraciones
room-manage-integrations-desc = Conecta esta sala con webhooks entrantes, buzones de correo y fuentes RSS/Atom.
room-manage-roles-heading = Roles y excepciones
room-policy-saved = Politica de publicacion guardada
room-nickname-saved = Apodo guardado
room-nickname-cleared = Apodo eliminado
room-retention-pill-disabled = Desactivada
room-retention-preview-note = Solo vista previa: no se elimina nada hasta que revises y apliques.
room-delete-room-desc = Elimina permanentemente esta sala y todos sus mensajes, archivos e historial. Esto no se puede deshacer.
room-delete-confirm-prefix = Escribe
room-delete-confirm-phrase = eliminar esta sala
room-delete-confirm-suffix = para confirmar.

# LC-484: resumenes con IA "ponme al dia" (hilos + canal)
summary-catch-up-heading = Ponme al dia
summary-unread-suffix = mensajes sin leer
summary-recent-scope = Resumir la actividad reciente
summary-generate = Generar resumen
summary-regenerate = Regenerar
summary-disclaimer = Generado por IA a partir de mensajes recientes. Puede estar incompleto.
room-thread-summarize = Resumir
