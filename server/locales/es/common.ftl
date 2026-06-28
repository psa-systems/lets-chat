# LC-188: Catálogo en español para el chrome de nivel raíz y la página de
# Configuración. Debe definir los mismos ids de mensaje que en/common.ftl
# (CI comprueba la cobertura).

## A11y (LC-197): textos para lectores de pantalla / navegación por teclado.
a11y-skip-to-content = Saltar al contenido principal
a11y-messages-region-label = Mensajes del chat
a11y-composer-message-input = Escribir un mensaje
a11y-status-picker-dialog-label = Establecer estado

## Admin nav
admin-nav-settings = Configuración
admin-nav-users = Usuarios
admin-nav-invites = Invitaciones
admin-nav-rooms = Salas
admin-nav-enclaves = Enclaves
admin-nav-anti-spam = Antispam
admin-nav-link-filter = Filtro de enlaces
admin-nav-quarantine = Cuarentena
admin-nav-branding = Marca
admin-nav-backup = Copia de seguridad
admin-nav-modlog = Registro de moderación
admin-nav-reports = Denuncias
admin-nav-analytics = Analíticas
admin-nav-commands = Comandos
admin-nav-bots = Bots
admin-nav-webhooks = Webhooks
admin-nav-bridges = Puentes
admin-nav-version-release = Versión
admin-nav-version-built = compilado
admin-nav-label = Navegación de administración

## Layout shell
layout-menu = Menú
layout-toggle-nav = Alternar navegación

## Call UI (layout shell)
call-incoming = Llamada entrante
call-accept = Aceptar
call-decline = Rechazar
call-active = Llamada activa
call-control-request = Solicitud de control remoto
call-control-request-explain = Podrán mover tu ratón y escribir con tu teclado.
call-control-grant = Conceder control
call-control-deny = Denegar
call-control-uipi = No se pueden controlar las ventanas con privilegios de administrador en su equipo.
call-mute = Silenciar
call-start-video = Iniciar vídeo
call-share-screen = Compartir pantalla
call-request-control = Solicitar control
call-devices = Dispositivos de llamada
call-choose-devices = Elegir dispositivos de llamada
call-hang-up = Colgar
call-controlling-suffix = está controlando tu pantalla.
call-stop-control = Detener control
call-stop-control-hint = o pulsa Ctrl+Alt+F9

## Maintenance page
maintenance-page-title = Mantenimiento en curso
maintenance-heading = Volvemos enseguida
maintenance-body = El sitio está en mantenimiento.
maintenance-retry = Intenta actualizar en unos minutos.

## Not found page
not-found-page-title = No encontrado
not-found-heading = Página no encontrada
not-found-exists-prefix = No existe nada en
not-found-exists-suffix = .
not-found-generic = La página que buscabas no existe.
not-found-back-home = Volver al inicio

## LC-220: standalone error page (themed, no sidebar chrome). The router-
## level 404 still uses not_found.html with sidebar context; these strings
## cover the handler-returned AppError variants.
error-status-not-found = No encontrado
error-status-forbidden = Prohibido
error-status-unauthorized = No autorizado
error-status-conflict = Conflicto
error-status-bad-request = Solicitud incorrecta
error-status-payload-too-large = Carga demasiado grande
error-status-too-many-requests = Demasiadas solicitudes
error-status-internal = Error del servidor
## (La etiqueta "Volver al inicio" se comparte con el 404 de nivel de router
## a través de la clave existente `not-found-back-home`; LC-220 no la duplica.)

## Poll modal
poll-create-title = Crear encuesta
poll-close-dialog = Cerrar diálogo
poll-question = Pregunta
poll-options = Opciones (una por línea, 2-10)
poll-allow-multi = Permitir varias opciones
poll-anonymous = Anónima (ocultar quién votó)
poll-close-after = Cerrar tras
poll-close-after-suffix = minutos (0 = nunca)
poll-post = Publicar encuesta

## Modal de evento (LC-491)
event-create-title = Crear evento
event-title-label = Titulo del evento
event-when-label = Cuando (tu hora local)
event-location-label = Lugar (opcional)
event-post = Publicar evento
event-rsvp-going = Asistire
event-rsvp-maybe = Quiza
event-rsvp-no = No puedo

## Keyboard shortcuts overlay (LC-252)
shortcuts-title = Atajos de teclado
shortcuts-close-dialog = Cerrar diálogo
shortcuts-section-messaging = Mensajería
shortcuts-section-navigation = Navegación
shortcuts-section-general = General
shortcuts-send = Enviar mensaje
shortcuts-newline = Nueva línea
shortcuts-edit-last = Editar tu último mensaje
shortcuts-bold = Negrita
shortcuts-italic = Cursiva
shortcuts-cancel = Cancelar edición / cerrar diálogo
shortcuts-mention = Mencionar a alguien
shortcuts-slash = Comando de barra
shortcuts-next-unread = Siguiente sala sin leer
shortcuts-prev-unread = Sala anterior sin leer
shortcuts-switcher = Cambio rápido
shortcuts-help = Mostrar esta ayuda

## Quick switcher (LC-260)
switcher-placeholder = Ir a una sala, MD o persona...
switcher-no-results = Sin coincidencias

## Image lightbox (LC-262)
lightbox-label = Visor de imágenes
lightbox-close = Cerrar
lightbox-prev = Imagen anterior
lightbox-next = Imagen siguiente

## Code block copy (LC-272)
code-copy = Copiar
code-copied = Copiado

## Scheduled message modal
scheduled-modal-title = Programar mensaje
scheduled-modal-deliver-at = Entregar el (tu hora local)
scheduled-modal-repeat = Repetir
scheduled-modal-submit = Programar
# LC-485: opciones de recurrencia + insignia de la fila pendiente
scheduled-repeat-none = No se repite
scheduled-repeat-daily = Se repite a diario
scheduled-repeat-weekdays = Se repite cada dia laborable
scheduled-repeat-weekly = Se repite semanalmente

## Scheduled messages page
scheduled-page-title = Programados
scheduled-heading = Mensajes programados
scheduled-delivery-note = Entregados dentro de los 30 segundos de la hora programada.
scheduled-empty = No tienes mensajes programados pendientes. Usa el botón de programar en cualquier redactor para encolar uno.
scheduled-dropped-heading = Descartados recientemente
scheduled-for-prefix = para
scheduled-scheduled-for = programado para
scheduled-dropped-at = descartado el
scheduled-dropped-label = Descartado:
scheduled-row-delivers = se entrega
scheduled-cancel-confirm = ¿Cancelar este mensaje programado?

## Settings page
settings-page-title = Configuración
settings-heading = Configuración
settings-sections-nav = Secciones de ajustes
settings-section-profile = Perfil
settings-section-appearance = Apariencia
settings-section-notifications = Notificaciones y actividad
settings-section-privacy = Privacidad y seguridad
settings-section-account = Datos y cuenta
settings-saved = Guardado.
# LC-426: feedback por acción (estado en línea + toast)
settings-fb-saved = Guardado
settings-fb-profile = Perfil guardado
settings-fb-avatar = Foto de perfil actualizada
settings-fb-avatar-removed = Foto de perfil eliminada
settings-fb-session-revoked = Sesión revocada
settings-fb-keyword-added = Palabra destacada añadida
settings-avatar-pending = Sin aplicar todavía - haz clic en Guardar perfil
settings-remove-avatar-confirm = ¿Eliminar tu foto de perfil?
settings-data-preparing = Preparando tus datos...
settings-deleting = Eliminando...
settings-account = Cuenta
settings-username = Usuario
settings-role = Rol
settings-profile = Perfil
settings-status-label = Estado:
settings-presence-online = En línea
settings-presence-away = Ausente
settings-presence-dnd = No molestar
settings-presence-offline = Desconectado
settings-profile-picture = Foto de perfil
settings-choose-image = Elegir imagen
settings-no-file = Ningún archivo seleccionado
settings-avatar-formats = PNG, JPEG o WebP. Máximo 1 MiB.
settings-avatar-err-size = La imagen debe pesar menos de 1 MiB.
settings-avatar-err-type = Formato no admitido: usa PNG, JPEG o WebP.
settings-display-name = Nombre visible
settings-email = Correo electrónico
settings-email-help = Se usa solo para restablecer la contraseña. Déjalo en blanco para eliminarlo.
settings-email-verified = Verificado
settings-email-unverified = Sin verificar
settings-email-resend = Reenviar correo de verificación
settings-email-verify-sent = Si tu cuenta tiene un correo sin verificar registrado, enviamos un nuevo enlace de verificación.
settings-bio = Biografía
settings-save-profile = Guardar perfil
settings-remove-avatar = Quitar avatar
settings-preferences = Preferencias
settings-pref-read-receipts = Enviar y recibir confirmaciones de lectura
settings-pref-public-profile = Perfil público
settings-pref-public-profile-help = Cuando está desactivado, tu perfil queda oculto en la búsqueda de personas y otros no pueden iniciar un nuevo MD contigo.
settings-pref-browser-notify = Mostrar notificaciones del navegador cuando me @mencionen o me envíen un MD
settings-pref-sound = Reproducir un sonido en nuevas menciones y MD
settings-pref-push = Activar notificaciones push (funciona con la pestaña cerrada)
settings-pref-push-unavailable-prefix = No disponible: define la variable de entorno
settings-pref-push-unavailable-suffix = en el servidor y reinicia para activar las notificaciones push.
settings-pref-push-available = Actívalo y luego usa el botón de abajo para suscribir este dispositivo.
settings-push-enable = Activar notificaciones de escritorio
settings-push-enable-help = Concede el permiso y suscribe este navegador de inmediato. Mantén activado el interruptor de arriba y guarda para recibir menciones, mensajes directos y recordatorios incluso con la pestaña cerrada.
settings-push-status-ok = Suscrito. Este dispositivo recibirá notificaciones push.
settings-push-status-ok-toggle-off = Suscrito, pero activa el interruptor de arriba y guarda. Hasta que lo hagas, el servidor no enviará notificaciones a este dispositivo.
settings-push-status-prompting = Esperando la solicitud de permiso del navegador...
settings-push-status-denied = No se concedió el permiso, así que este dispositivo no está suscrito.
settings-push-status-blocked = Las notificaciones están bloqueadas para este sitio. Permítelas en la configuración del navegador e inténtalo de nuevo.
settings-push-status-unsupported = Este navegador no admite notificaciones push.
settings-push-status-unavailable = Las notificaciones push no están disponibles en el servidor en este momento.
settings-push-status-failed = No se pudo suscribir. Revisa la consola del navegador para más detalles e inténtalo de nuevo.
settings-pref-email-digest = Enviarme por correo un resumen de menciones y MD perdidos
settings-pref-email-unavailable = No disponible: el administrador del servidor no ha configurado SMTP.
settings-pref-email-digest-help = Se envía como máximo una vez por sesión sin conexión (tras al menos una hora de inactividad). Define tu correo arriba para recibirlos.
settings-pref-login-alerts = Enviarme un correo cuando un nuevo dispositivo inicie sesión en mi cuenta
settings-pref-login-alerts-help = Se envía la primera vez que un navegador o IP inicia sesión. No se envía alerta para dispositivos ya registrados.
settings-pref-email-activity = Enviarme un correo por cada mención y mensaje directo
settings-pref-email-activity-help = Un correo por evento (no agrupado como el resumen). Si el operador tiene configurada la entrada por correo, responder al correo publica tu respuesta en el chat en tu nombre. Limitado a 20 correos por minuto por destinatario.
settings-save-preferences = Guardar preferencias
# LC-304: highlight words
settings-keyword-heading = Palabras destacadas
settings-keyword-help = Recibe una notificación y resalta el mensaje cuando aparezca una de estas palabras, igual que una @mención. No distingue mayúsculas; coincide por palabra completa.
settings-keyword-placeholder = Añadir una palabra
settings-keyword-add = Añadir
settings-keyword-remove = Quitar
settings-keyword-empty = Aún no hay palabras destacadas.
settings-keyword-cap = Límite alcanzado
settings-dnd-heading = No molestar
settings-dnd-active = Activo ahora
settings-dnd-explain = Durante No molestar, las notificaciones push se suprimen y el resumen por correo retiene tus menciones perdidas para el próximo envío. La actividad en la aplicación se sigue registrando. Otros ven una insignia de "no molestar" en tu avatar.
settings-dnd-pause-heading = Pausar notificaciones
settings-dnd-paused-until-prefix = En pausa hasta
settings-dnd-paused-until-suffix = (UTC).
settings-dnd-resume = Reanudar ahora
settings-dnd-30-minutes = 30 minutos
settings-dnd-2-hours = 2 horas
settings-dnd-8-hours = 8 horas
settings-dnd-pause-minutes = Minutos de pausa
settings-dnd-min = min
settings-dnd-schedule-heading = Horario de silencio
settings-dnd-timezone = Zona horaria (déjalo en Desactivado para desactivar el horario)
settings-dnd-off = Desactivado
settings-dnd-weekday-start = Inicio entre semana
settings-dnd-weekday-end = Fin entre semana
settings-dnd-weekend-start = Inicio fin de semana
settings-dnd-weekend-end = Fin fin de semana
settings-dnd-schedule-help = Deja un par inicio/fin en blanco para omitir ese grupo. Una ventana cuyo fin es anterior a su inicio abarca la medianoche (p. ej. 22:00 a 07:00).
settings-dnd-save-schedule = Guardar horario
settings-change-password = Cambiar contraseña
settings-password-updated = Contraseña actualizada.
settings-current-password = Contraseña actual
settings-new-password = Nueva contraseña
settings-confirm-new-password = Confirmar nueva contraseña
settings-update-password = Actualizar contraseña
settings-active-sessions = Sesiones activas
settings-sessions-help = Todos los dispositivos con sesión iniciada en esta cuenta. Revoca cualquier sesión que no reconozcas. Cerrar sesión finaliza la actual.
settings-session-revoked = Sesión revocada.
settings-this-device = Este dispositivo
settings-last-seen = Visto por última vez
settings-from = desde
settings-signed-in = Sesión iniciada
settings-privacy = Privacidad
settings-manage-blocked = Gestionar usuarios bloqueados
settings-api-tokens-heading = Tokens de API
settings-manage-api-tokens = Gestionar tokens de API
settings-api-tokens-help = para la API HTTP (bots, scripts, integraciones).
settings-storage-heading = Uso de almacenamiento
settings-storage-using = Estás usando
settings-storage-of-quota = de tu
settings-storage-quota-suffix = cuota de subidas.
settings-storage-no-quota = de subidas. Actualmente no hay ninguna cuota establecida en tu cuenta.
settings-your-data-heading = Tus datos
settings-your-data-help = Descarga un archivo JSON con tu perfil, mensajes, reacciones, marcadores, sesiones, bloqueos, membresías de salas y enclaves, menciones, metadatos de archivos subidos y preferencias de notificación.
settings-download-data = Descargar mis datos
settings-delete-account-heading = Eliminar cuenta
settings-delete-account-explain = Elimina permanentemente tu cuenta, perfil, mensajes, reacciones, marcadores, menciones, archivos subidos, bloqueos, sesiones y suscripciones push. Esto no se puede deshacer. Si eres propietario de un enclave con otros miembros, transfiere la propiedad o elimina el enclave primero.
settings-delete-confirm-prefix = Escribe
settings-delete-confirm-suffix = para confirmar
settings-delete-account-button = Eliminar mi cuenta
settings-delete-account-confirm-js = ¿Eliminar tu cuenta? Esto no se puede deshacer.
settings-about-heading = Acerca de
settings-about-version = Versión
settings-about-release = Lanzamiento
settings-about-commit = Commit
settings-about-built = Compilado

## API tokens page
api-tokens-page-title = Tokens de API
api-tokens-heading = Tokens de API
api-tokens-back = Volver a configuración
api-tokens-intro-prefix = Tokens bearer personales para la API HTTP. Un token puede tener un alcance más reducido que tu cuenta y se muestra una sola vez. Envíalo como
api-tokens-intro-suffix = . Consulta la referencia de la API para conocer las rutas y los alcances requeridos.
api-tokens-created-note = Token creado. Cópialo ahora: no se mostrará de nuevo.
api-tokens-disabled-prefix = Los tokens de API están desactivados porque el servidor no tiene una clave secreta configurada
api-tokens-disabled-suffix = .
api-tokens-create-heading = Crear un token
api-tokens-name = Nombre
api-tokens-scopes = Alcances
api-tokens-expires = Caduca tras (días, 0 = nunca)
api-tokens-create-button = Crear token
api-tokens-your-tokens = Tus tokens
api-tokens-none = Aún no hay tokens.
api-tokens-created-at = creado
api-tokens-last-used = usado por última vez
api-tokens-never-used = nunca usado
api-tokens-expires-at = caduca
api-tokens-revoked = revocado

## Blocked users page
blocked-page-title = Usuarios bloqueados
blocked-heading = Usuarios bloqueados
blocked-back = Configuración
blocked-explain = Bloquear a un usuario lo oculta de la búsqueda de personas, le impide iniciar o enviarte MD y oculta sus mensajes para ti.
blocked-by-username = Bloquear por nombre de usuario
blocked-by-username-help = Introduce el nombre de usuario exacto, incluso para usuarios con perfiles privados.
blocked-username-placeholder = usuario
blocked-block-button = Bloquear
blocked-empty = No has bloqueado a nadie.
blocked-unblock = Desbloquear
