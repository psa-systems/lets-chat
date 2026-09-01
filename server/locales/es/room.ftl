# LC-188: Cadenas de la interfaz de salas (locale es). Las claves deben
# coincidir con las del locale fuente (en).

## Shared
room-save = Guardar
room-edit-message-label = Editar mensaje
room-cancel = Cancelar

## Composer
room-composer-send-failed = No se pudo enviar.
room-composer-retry = Reintentar
room-composer-attach-file = Adjuntar archivo
room-composer-record-voice = Grabar mensaje de voz
room-composer-record-clip = Grabar videoclip
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
room-composer-ttl-label = Temporizador de autodestruccion
room-composer-ttl-off = Sin temporizador
room-composer-ttl-5m = 5m
room-composer-ttl-1h = 1h
room-composer-ttl-1d = 1d
room-composer-ttl-7d = 7d
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
room-table-revoke-confirm = ¿Revocar esta credencial? Todas las integraciones que la usan dejarán de funcionar de inmediato y no se puede restaurar.

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
room-nickname-heading = Tu apodo en
room-nickname-help = Se muestra en tus mensajes de esta sala en lugar de tu nombre visible. Solo se aplica aquí.
room-nickname-placeholder = p. ej. Capitán
room-nickname-clear = Borrar

## Wiki (standalone view/edit)
room-wiki-label = Wiki (Markdown)

## Message row
# LC-680: collapsed run of consecutive call/system events. %n% is the event count.
room-callgroup-summary = %n% eventos de llamada
room-callgroup-transcript = 📄 Transcripcion
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
# LC-528: read a message aloud (browser speech synthesis)
room-msg-read-aloud = Leer en voz alta
room-msg-stop-reading = Detener lectura
room-msg-forward = Reenviar
room-msg-report = Denunciar
room-msg-edit = Editar
room-msg-delete = Eliminar
# LC-486: traduccion en linea
room-msg-translate = Traducir
room-msg-translate-to = Traducir a
room-msg-translated-to = Traducido a
room-msg-translate-language = Idioma de traducción
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
# LC-714: cola de tickets de soporte de la mesa de ayuda con IA (/admin/support).
support-queue-title = Soporte
support-queue-empty = No hay tickets de soporte abiertos.
support-jump = Abrir canal
support-row-in = En:
support-action-claim = Atender
support-action-resolve = Resolver
# LC-726: visibilidad del soporte fuera de la sección de administración.
support-rail-label = Solicitudes de soporte
support-home-title = Solicitudes de soporte
support-home-open-queue = Abrir la cola de soporte

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

## Load-older sentinel
room-load-older = Cargando mensajes anteriores...

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
# LC-665: interruptor del resumen diario con IA (pagina de gestion).
room-digest-heading = Resumen diario
room-digest-intro = Publica una vez al dia un breve resumen con IA de la actividad reciente de esta sala, como el bot asistente.
room-digest-on-label = Activado.
room-digest-on-text = Se publica un resumen diario de actividad en esta sala.
room-digest-off-label = Desactivado.
room-digest-off-text = No se publica ningun resumen automatico en esta sala.
# LC-494: interruptor del modo escenario (pagina de gestion).
room-stage-heading = Modo escenario
room-stage-intro = Convierte esta sala en un escenario con oradores y oyentes, donde la gente pide la palabra.
room-stage-audio-note = Los roles y la peticion de palabra ya estan; el audio para audiencias grandes necesita un servidor de medios (proximamente).
room-stage-on-label = Activado.
room-stage-on-text = Esta sala muestra la lista del escenario.
room-stage-off-label = Desactivado.
room-stage-off-text = El modo escenario esta desactivado en esta sala.
room-remote-control-heading = Control remoto de pantallas compartidas
room-remote-control-intro = Permite que un participante del huddle pida el control de una pantalla compartida en esta sala; quien comparte aprueba cada solicitud.
room-remote-control-workspace-off = El control remoto esta desactivado en todo el espacio de trabajo, asi que este ajuste no surte efecto hasta que un administrador lo active.
room-remote-control-on-label = Activado.
room-remote-control-on-text = Los participantes pueden pedir el control de una pantalla compartida en esta sala.
room-remote-control-off-label = Desactivado.
room-remote-control-off-text = El control remoto esta desactivado en esta sala.
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
# LC-674: visibilidad de los comandos - boton, enlace "ver todos", consejo.
room-composer-commands = Comandos (/)
room-slash-see-all = Ver todos los comandos
room-slash-tip = Consejo: escribe / para comandos, @ para mencionar, : para emoji.
room-slash-tip-dismiss = Descartar el consejo
# LC-675: se muestra al invocar un comando sin permiso para ejecutarlo.
room-slash-forbidden = No tienes permiso para ejecutar ese comando.

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
room-thread-mute = Silenciar
room-thread-mute-title = Silencia este hilo para dejar de recibir avisos de nuevas respuestas
room-thread-muted = Silenciado
room-thread-muted-title = Este hilo está silenciado. Haz clic para reanudar los avisos.
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

# LC-683: panel de miembros de la sala (abierto desde el grupo de avatares del encabezado)
members-panel-title = Miembros
members-count-suffix = miembros
members-open-label = Ver miembros
members-filter-placeholder = Filtrar miembros
members-role-owner = Propietario
members-role-admin = Administrador
members-role-moderator = Moderador
members-manage-link = Gestionar miembros
members-you = Tú

# LC-484: resumenes con IA "ponme al dia" (hilos + canal)
summary-catch-up-heading = Ponme al dia
summary-unread-suffix = mensajes sin leer
summary-recent-scope = Resumir la actividad reciente
summary-generate = Generar resumen
summary-regenerate = Regenerar
summary-disclaimer = Generado por IA a partir de mensajes recientes. Puede estar incompleto.
# LC-650: shared "AI is working" pending labels shown while an LLM request runs.
ai-generating = Generando...
ai-summarizing = Resumiendo...
ai-translating = Traduciendo...
ai-working-slow = Calentando el modelo local: la primera solicitud tras un periodo inactivo puede tardar unos segundos.
# LC-654: first stage of the catch-me-up skeleton status, before "Summarizing...".
ai-reading-messages = Leyendo mensajes recientes...
# LC-655: staged status for the composer writing assistant.
ai-thinking = Pensando...
ai-writing = Escribiendo...
room-thread-summarize = Resumir

# LC-495: automatizaciones de flujo de trabajo (pagina de gestion de sala)
room-automations-heading = Automatizaciones
room-automations-intro = Reglas sin codigo que se ejecutan cuando ocurre algo en esta sala.
room-automations-empty = Aun no hay automatizaciones. Agrega una abajo.
room-automations-on = Activada
room-automations-off = Desactivada
room-automations-trigger-message = Cuando un mensaje contiene
room-automations-trigger-reaction = Cuando alguien reacciona con
room-automations-any = cualquier cosa
room-automations-then-post = entonces publica un mensaje.
room-automations-disable = Desactivar
room-automations-enable = Activar
room-automations-delete = Eliminar
room-automations-delete-confirm = Eliminar esta automatizacion?
room-automations-new-heading = Nueva automatizacion
room-automations-name-label = Nombre (opcional)
room-automations-name-ph = p. ej. Bot de bienvenida
room-automations-when-label = Cuando
room-automations-match-label = Coincidencia (en blanco para cualquiera)
room-automations-match-ph = palabra clave o emoji
room-automations-do-label = Entonces publica
room-automations-do-ph = El mensaje a publicar
room-automations-vars-help = Marcadores, cada uno entre llaves: user, text, emoji.
room-automations-create = Crear automatizacion

## LC-527: follow-up tasks (from a call transcript's action items)
followup-card-title = Tareas de seguimiento
followup-create-button = Crear tareas de seguimiento
followup-created = Tareas de seguimiento publicadas en la sala.
followup-claim = Reclamar
followup-assigned-you = Tú
followup-toggle-aria = Marcar como hecha
followup-done-suffix = hechas

## LC-529: reaction highlights recap
room-highlights-title = Destacados
room-highlights-window = Lo más reaccionado en los últimos 7 días
room-highlights-empty = Aún no hay reacciones en los últimos 7 días.
room-highlights-jump = Ir al mensaje
room-highlights-reactions = reacciones
partials-room-highlights = Destacados
partials-room-highlights-title = Reacciones destacadas

## LC-526: kudos / recognition
kudos-recognition-prefix = 🎉 Kudos para
kudos-title = Tabla de kudos
kudos-window = Kudos en los últimos 30 días
kudos-empty = Aún no hay kudos. Da algunos con /kudos @usuario.
kudos-most-appreciated = Más apreciados
kudos-most-generous = Más generosos
kudos-hint = Da kudos en cualquier canal con /kudos @usuario <motivo>.
sidebar-link-kudos = Kudos
stats-title = Tus estadísticas
stats-subtitle = Un resumen de tu actividad hasta ahora
stats-messages-sent = Mensajes enviados
stats-active-days = Días activos
stats-kudos-received = Kudos recibidos
stats-reactions-received = Reacciones recibidas
stats-reactions-given = Reacciones dadas
stats-member-since = Miembro desde
stats-top-channels = Canales principales
stats-top-channels-empty = Aún no hay actividad en canales.
stats-hint = Solo tú puedes ver tus estadísticas.
sidebar-link-stats = Tus estadísticas

## LC-532: composer AI writing assistant
compose-assist-tip = Asistente de redacción con IA
# Aún lo usa el panel de respuestas sugeridas (descartar los chips, sin cambiar el borrador).
compose-assist-dismiss = Descartar
# LC-655: menú de modos + panel de vista previa (Aceptar / Regenerar / Descartar).
compose-assist-menu-label = Reescribir con IA
compose-assist-heading = Asistente de IA
compose-assist-accept = Aceptar
compose-assist-regenerate = Regenerar
compose-assist-discard = Descartar
compose-assist-action-rephrase = Mejorar redacción
compose-assist-action-grammar = Corregir gramática
compose-assist-action-concise = Acortar
compose-assist-action-friendly = Tono más amable
compose-assist-action-formal = Más formal
# LC-669: aviso de tono/claridad bajo demanda (una revisión, no una reescritura).
compose-assist-action-tone = Revisar el tono
compose-tone-heading = Revisión de tono
compose-tone-checking = Revisando el tono...
compose-tone-looks-good = Se lee claro y amable.
compose-tone-dismiss = Descartar

## LC-548: AI suggested replies
room-msg-suggest-reply = Sugerir respuesta
suggest-reply-heading = Toca un borrador para añadirlo a tu mensaje

## LC-549: búsqueda semántica / de mensajes relacionados
room-msg-related = Buscar relacionados
related-heading = Mensajes relacionados
related-subheading = Ordenados por significado, no por palabras clave
related-empty = No se encontraron mensajes muy relacionados.

## LC-534: per-channel slowmode
room-slowmode-heading = Modo lento
room-slowmode-intro = Limita con qué frecuencia puede publicar cada miembro en este canal. Los moderadores están exentos.
room-slowmode-label = Espera entre mensajes
room-slowmode-off = Desactivado
room-slowmode-5s = 5 segundos
room-slowmode-10s = 10 segundos
room-slowmode-30s = 30 segundos
room-slowmode-60s = 1 minuto
room-slowmode-saved = Modo lento actualizado.

## LC-568: panel de detalles (columna derecha, debajo del panel de hilo)
room-details-title = Detalles
# LC-576: chevron para plegar/desplegar el panel de detalles (refleja el de la barra lateral izquierda)
room-details-toggle = Alternar el panel de detalles
room-details-created = Creado
room-details-members = Miembros
room-details-notifications = Notificaciones
room-details-pinned = Fijado
room-details-pinned-yes = Sí
room-details-pinned-no = No
room-details-leave = Salir de la sala
room-details-leave-confirm = ¿Salir de esta sala? Necesitarás una nueva invitación para volver a unirte.

# LC-805: room page htmx error toast (mirrors en room-action-failed).
room-action-failed = No se pudo completar esa accion. Intentalo de nuevo.
