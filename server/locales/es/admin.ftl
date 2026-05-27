# LC-188: Cadenas de la interfaz de administracion (locale es). Las claves son
# kebab-case, con prefijo "admin-", sin puntos. Agrupadas por area de plantilla.

## Analytics
admin-analytics-title = Analiticas
admin-analytics-heading = Analiticas
admin-analytics-date-range = Rango de fechas
admin-analytics-recompute = Recalcular hoy
admin-analytics-recompute-title-prefix = Recalcular las metricas de hoy
admin-analytics-counts-note = Solo recuentos; no se muestra la actividad por usuario. Las metricas se preagregan a diario.
admin-analytics-over = en
admin-analytics-retention-heading = Retencion por cohorte de registro
admin-analytics-retention-note = Porcentaje de cada cohorte semanal de registro que envio un mensaje en la enesima semana tras unirse.
admin-analytics-no-cohorts = Aun no hay cohortes de registro.
admin-analytics-th-cohort = Cohorte
admin-analytics-th-users = Usuarios

## Anti-spam
admin-antispam-title = Anti-spam
admin-antispam-heading = Anti-spam
admin-antispam-saved = Ajustes guardados.
admin-antispam-rate-limits-heading = Limites de tasa (por minuto)
admin-antispam-rate-limits-note-1 = Un limite de
admin-antispam-rate-limits-note-2 = desactiva el limite. Los limites por IP no hacen nada en despliegues sin un proxy inverso que reenvie
admin-antispam-messages-per-user = Mensajes por usuario
admin-antispam-registrations-per-ip = Registros por IP
admin-antispam-login-attempts-per-ip = Intentos de inicio de sesion por IP
admin-antispam-login-attempts-note = Cubre el formulario de contrasena y el desafio de 2FA / recuperacion (presupuesto compartido). Por IP, asi que requiere un proxy inverso de confianza. 0 lo desactiva.
admin-antispam-password-resets-per-ip = Solicitudes de restablecimiento de contrasena por IP
admin-antispam-defenses-heading = Defensas
admin-antispam-link-filter = Filtro de enlaces
admin-antispam-link-filter-note-1 = Pasa los cuerpos de los mensajes por las reglas en
admin-antispam-link-filter-note-2 = Las reglas pueden bloquear, poner en cuarentena o advertir sobre dominios coincidentes.
admin-antispam-honeypot = Honeypot en el registro
admin-antispam-honeypot-note = Campo de formulario oculto que los usuarios legitimos nunca rellenan pero los bots perezosos si. Sin impacto de falsos positivos.
admin-antispam-save = Guardar

## Backup / restore
admin-backup-title = Copia de seguridad / Restauracion
admin-backup-heading = Copia de seguridad / restauracion
admin-backup-staged-title = Hay una restauracion preparada.
admin-backup-staged-1 = Reinicia el servidor para aplicarla. Al arrancar, el directorio de datos actual se renombra a un lado (sufijo
admin-backup-staged-2 = ) y la copia preparada ocupa su lugar. Si cambias de opinion, elimina el archivo
admin-backup-staged-3 = y el directorio hermano
admin-backup-staged-4 = dentro del directorio de datos antes de reiniciar.
admin-backup-headsup-title = Atencion:
admin-backup-headsup-1 = cada restauracion deja el directorio de datos anterior en disco como un hermano llamado
admin-backup-headsup-2 = . Estos se acumulan con el tiempo; una vez confirmada la salud de la restauracion, elimina manualmente los directorios antiguos
admin-backup-headsup-3 = para recuperar espacio. No hay limpieza automatica.
admin-backup-create-heading = Crear copia de seguridad
admin-backup-create-1 = Descarga un unico
admin-backup-create-2 = con instantaneas SQLite consistentes (via
admin-backup-create-3 = ) de las tres bases de datos mas los arboles
admin-backup-create-4 = y
admin-backup-create-5 = . Cada entrada se hace sha256 en un
admin-backup-create-6 = para que una restauracion posterior pueda verificar la integridad. El servidor sigue sirviendo mientras se ejecuta la instantanea. Los despliegues grandes pueden tardar uno o dos minutos; la respuesta empieza a transmitirse en cuanto el archivo termina de construirse en disco.
admin-backup-download = Descargar copia de seguridad
admin-backup-restore-heading = Restaurar desde archivo
admin-backup-restore-note = Sube una copia de seguridad descargada previamente. El servidor valida el manifiesto + sha256 por archivo, rechaza archivos creados en una version distinta de lets-chat, y luego prepara el contenido en un directorio hermano. Un reinicio finaliza el intercambio.
admin-backup-stage-confirm = Preparar este archivo para restaurar? En el proximo reinicio del servidor los datos actuales seran reemplazados.
admin-backup-stage = Preparar restauracion
admin-backup-stage-note = El paso de reinicio es la puerta de confirmacion; preparar por si solo no toca el directorio de datos en vivo.

## Bots
admin-bots-title = Bots
admin-bots-heading = Bots
admin-bots-intro = Las cuentas de bot son identidades de maquina. Solo se autentican mediante su token de API (el inicio de sesion por cookie los rechaza), publican con una insignia "bot" y nunca reciben notificaciones. Desactivar un bot lo banea y revoca todos sus tokens.
admin-bots-created-prefix = Bot
admin-bots-created-suffix = creado. Copia su token de API ahora: no se mostrara de nuevo.
admin-bots-no-secret-1 = Los bots requieren una clave secreta del servidor
admin-bots-no-secret-2 = para acunar su token de API. Configurala y reinicia para crear bots.
admin-bots-create-heading = Crear un bot
admin-bots-username = Nombre de usuario
admin-bots-token-scopes = Ambitos del token de API
admin-bots-create-button = Crear bot
admin-bots-th-bot = Bot
admin-bots-th-created = Creado
admin-bots-th-status = Estado
admin-bots-th-actions = Acciones
admin-bots-status-disabled = desactivado
admin-bots-status-active = activo
admin-bots-disable = Desactivar
admin-bots-empty = Aun no hay bots.

admin-bridges-title = Puentes
admin-bridges-heading = Puentes
admin-bridges-intro = Los puentes de protocolo se ejecutan FUERA DEL PROCESO como demonios separados y publican mensajes de protocolos externos en una sala de lets-chat mediante la API. Registrar un puente aqui crea el usuario bot, emite su token API con alcance de puente, y guarda su configuracion de demonio sellada. El demonio se autentica con el token; lets-chat hace seguimiento de los latidos pero no ejecuta el demonio. Eliminar un puente detiene el trafico nuevo pero deja los mensajes historicos renderizables.
admin-bridges-no-secret = Los puentes necesitan una clave secreta del servidor para sellar la configuracion del demonio.
admin-bridges-created-prefix = Bot de puente
admin-bridges-created-suffix = creado. Copia su token API ahora - no se mostrara de nuevo.
admin-bridges-token-scopes-note = Alcances del token: bridge:post + bridge:heartbeat.
admin-bridges-create-heading = Registrar un puente
admin-bridges-room = Sala
admin-bridges-bot-username = Nombre de usuario del bot
admin-bridges-kind = Tipo de protocolo
admin-bridges-kind-note = Los demonios de IRC y XMPP comparten esta superficie; v1 incluye solo Matrix.
admin-bridges-config = Configuracion del demonio (opaca para el servidor, sellada en reposo)
admin-bridges-config-note = Almacenada cifrada bajo LETS_CHAT_SECRET_KEY. La forma es especifica del demonio (tipicamente JSON con URL del homeserver y secreto compartido).
admin-bridges-create-button = Registrar puente
admin-bridges-th-room = Sala
admin-bridges-th-kind = Tipo
admin-bridges-th-bot = Bot
admin-bridges-th-status = Estado
admin-bridges-th-last-heartbeat = Ultimo latido
admin-bridges-remove = Eliminar
admin-bridges-empty = No hay puentes registrados.

## Branding
admin-branding-title = Marca
admin-branding-heading = Marca
admin-branding-saved = Marca guardada.
admin-branding-intro-1 = Los colores se propagan a cada pagina mediante variables CSS (sin necesidad de reconstruir Tailwind). El logotipo se muestra en la pagina de inicio de sesion y se sirve desde
admin-branding-intro-2 = El encabezado y el cuerpo del inicio de sesion se renderizan a traves de un pipeline de markdown restringido: se permiten negrita, cursiva, enlaces, listas y parrafos; el HTML en bruto y los bloques de codigo se eliminan.
admin-branding-primary-color = Color primario
admin-branding-accent-color = Color de acento
admin-branding-logo = Logotipo
admin-branding-current-logo-alt = Logotipo actual
admin-branding-current-logo = Logotipo actual
admin-branding-logo-help = PNG / JPEG / WebP / GIF de hasta 1 MiB. Deja vacio para conservar el logotipo actual.
admin-branding-favicon = Favicon
admin-branding-current-favicon-alt = Favicon actual
admin-branding-current-favicon = Favicon actual
admin-branding-favicon-help = PNG / ICO / SVG de hasta 1 MiB. Deja vacio para conservar el favicon actual. Los navegadores cachean los favicons de forma agresiva; un cambio puede requerir una recarga forzada para aparecer.
admin-branding-login-heading = Encabezado de la pagina de inicio de sesion
admin-branding-login-heading-help = Texto plano. Se muestra sobre el formulario de inicio de sesion. Si esta vacio, recurre a "Iniciar sesion".
admin-branding-login-body = Cuerpo de la pagina de inicio de sesion
admin-branding-login-body-help = Markdown restringido. Usalo para una nota de bienvenida, un enlace a tu politica de privacidad o la informacion de contacto del operador.
admin-branding-save = Guardar marca

## Enclaves
admin-enclaves-title = Enclaves
admin-enclaves-heading = Enclaves
admin-enclaves-intro = Cada enclave en este servidor, independientemente de tu membresia. Usa el enlace de gestion para entrar al enclave (el modo dios de administrador del sitio omite la comprobacion de membresia).
admin-enclaves-th-name = Nombre
admin-enclaves-th-visibility = Visibilidad
admin-enclaves-th-owner = Propietario
admin-enclaves-th-members = Miembros
admin-enclaves-th-storage = Almacenamiento (usado / cuota MiB)
admin-enclaves-th-created = Creado
admin-enclaves-th-actions = Acciones
admin-enclaves-public = Publico
admin-enclaves-private = Privado
admin-enclaves-none = ninguno
admin-enclaves-unlimited = ilimitado
admin-enclaves-save = Guardar
admin-enclaves-open = Abrir
admin-enclaves-manage = Gestionar

## Invites
admin-invites-title = Invitaciones
admin-invites-heading = Codigos de invitacion
admin-invites-create = Crear invitacion
admin-invites-th-code = Codigo
admin-invites-th-created-by = Creado por
admin-invites-th-created-at = Creado el
admin-invites-th-used-by = Usado por
admin-invites-th-action = Accion
admin-invites-revoke = Revocar

## Link filter
admin-linkfilter-title = Filtro de enlaces
admin-linkfilter-heading = Reglas del filtro de enlaces
admin-linkfilter-intro-1 = Los patrones coinciden con el host de cada URL en el cuerpo de un mensaje. Usa un dominio literal
admin-linkfilter-intro-2 = o un glob simple con
admin-linkfilter-intro-3 = La coincidencia no distingue mayusculas de minusculas.
admin-linkfilter-intro-4 = Asegurate de que la funcion este habilitada en la
admin-linkfilter-intro-link = pagina de ajustes anti-spam
admin-linkfilter-pattern = Patron
admin-linkfilter-action = Accion
admin-linkfilter-warn = advertir
admin-linkfilter-quarantine = cuarentena
admin-linkfilter-block = bloquear
admin-linkfilter-add-rule = Anadir regla
admin-linkfilter-th-pattern = Patron
admin-linkfilter-th-action = Accion
admin-linkfilter-th-added-by = Anadido por
admin-linkfilter-th-added = Anadido
admin-linkfilter-th-actions = Acciones
admin-linkfilter-delete = Eliminar
admin-linkfilter-empty = No hay reglas. Anade una arriba para empezar a filtrar.

## Mod log
admin-modlog-title = Registro de moderacion
admin-modlog-heading = Registro de moderacion
admin-modlog-th-who = Quien
admin-modlog-th-action = Accion
admin-modlog-th-target = Objetivo
admin-modlog-th-reason = Motivo
admin-modlog-th-when = Cuando

## Webhook deliveries
admin-deliveries-title = Entregas de webhook
admin-deliveries-heading = Entregas
admin-deliveries-webhook = webhook
admin-deliveries-back = Volver
admin-deliveries-th-event = Evento
admin-deliveries-th-attempt = Intento
admin-deliveries-th-status = Estado
admin-deliveries-th-scheduled = Programado
admin-deliveries-th-delivered = Entregado
admin-deliveries-pending = pendiente
admin-deliveries-empty = No hay entregas registradas.

## Outgoing webhooks
admin-webhooks-title = Webhooks salientes
admin-webhooks-heading = Webhooks salientes
admin-webhooks-intro-1 = Registra una URL para recibir un
admin-webhooks-intro-2 = firmado cuando se disparen eventos coincidentes. El cuerpo es
admin-webhooks-intro-3 = y cada solicitud lleva
admin-webhooks-intro-4 = sobre el cuerpo en bruto, con la clave secreta de firma del webhook, mas
admin-webhooks-intro-5 = Verifica ambos. Las entregas fallidas se reintentan con retroceso; tras fallos repetidos el webhook se desactiva automaticamente.
admin-webhooks-secret-prefix = Secreto de firma para el webhook
admin-webhooks-secret-suffix = - copialo ahora, no se mostrara de nuevo.
admin-webhooks-create-heading = Crear un webhook
admin-webhooks-scope = Ambito
admin-webhooks-scope-global = global
admin-webhooks-scope-enclave = enclave
admin-webhooks-scope-room = sala
admin-webhooks-scope-id = Id de ambito (enclave/sala)
admin-webhooks-scope-id-placeholder = (vacio para global)
admin-webhooks-delivery-url = URL de entrega
admin-webhooks-events = Eventos
admin-webhooks-create-button = Crear webhook
admin-webhooks-th-scope = Ambito
admin-webhooks-th-events = Eventos
admin-webhooks-th-url = URL
admin-webhooks-th-status = Estado
admin-webhooks-th-actions = Acciones
admin-webhooks-status-disabled = desactivado
admin-webhooks-status-active = activo
admin-webhooks-fails = fallos
admin-webhooks-history = Historial
admin-webhooks-rotate = Rotar secreto
admin-webhooks-enable = Activar
admin-webhooks-disable = Desactivar
admin-webhooks-delete = Eliminar
admin-webhooks-empty = Aun no hay webhooks salientes.

## Quarantine
admin-quarantine-title = Cuarentena
admin-quarantine-heading = Mensajes en cuarentena
admin-quarantine-intro = Mensajes retenidos por el filtro de enlaces a la espera de revision por un moderador. Aprobar libera el mensaje en la sala; rechazar lo elimina de forma suave.
admin-quarantine-th-author = Autor
admin-quarantine-th-room = Sala
admin-quarantine-th-body = Cuerpo
admin-quarantine-th-matched = Coincidencia
admin-quarantine-th-held = Retenido
admin-quarantine-th-actions = Acciones
admin-quarantine-approve = Aprobar
admin-quarantine-reject = Rechazar
admin-quarantine-empty = No hay nada en la cola.

## Room row
admin-roomrow-topic-placeholder = Tema
admin-roomrow-save = Guardar
admin-roomrow-username-placeholder = Nombre de usuario
admin-roomrow-invite = Invitar
admin-roomrow-regen = Regenerar
admin-roomrow-delete = Eliminar
admin-roomrow-delete-confirm-prefix = Eliminar la sala
admin-roomrow-delete-confirm-suffix = Esto elimina todos sus mensajes.

## Rooms
admin-rooms-title = Salas
admin-rooms-heading = Salas
admin-rooms-intro = Crea salas dentro de un enclave a traves de la pagina de inicio de cada enclave. Esta vista es de solo lectura y esta pensada para la moderacion global.
admin-rooms-th-name = Nombre
admin-rooms-th-type = Tipo
admin-rooms-th-members = Miembros
admin-rooms-th-invite = Invitacion
admin-rooms-th-actions = Acciones

## Settings
admin-settings-title = Ajustes
admin-settings-maintenance-on-title = El modo de mantenimiento esta ACTIVADO.
admin-settings-maintenance-on-body = Los usuarios que no sean administradores ven una pagina de mantenimiento 503. Desactivalo abajo para restaurar el acceso.
admin-settings-maintenance-heading = Modo de mantenimiento
admin-settings-maintenance-enable = Activar el modo de mantenimiento
admin-settings-maintenance-enable-note = Los no administradores ven una pagina 503; los administradores mantienen acceso completo para que puedas volver a desactivarlo cuando termines.
admin-settings-maintenance-message-label = Mensaje mostrado a los usuarios
admin-settings-maintenance-message-placeholder = De vuelta a las 17:00 UTC; actualizando la base de datos.
admin-settings-maintenance-save = Guardar modo de mantenimiento
admin-settings-smtp-heading = SMTP (correo saliente)
admin-settings-smtp-note-1 = SMTP se configura mediante variables de entorno, no en la interfaz de administracion. Define
admin-settings-smtp-note-2 = y el par opcional
admin-settings-smtp-note-3 = , luego reinicia el servidor. Consulta
admin-settings-smtp-note-4 = para la lista completa.
admin-settings-smtp-note-5 = Las versiones anteriores mostraban aqui un formulario SMTP que escribia en
admin-settings-smtp-note-6 = pero que el sistema de correo nunca leia; el formulario se ha eliminado y la siguiente migracion limpia cualquier fila obsoleta.
admin-settings-imap-heading = Entrada de correo (sondeo IMAP)
admin-settings-imap-saved = Ajustes de IMAP guardados. Reinicia el servidor para aplicar la nueva configuracion.
admin-settings-imap-intro-1 = Lets-chat sondea este buzon cada 5 minutos; los mensajes dirigidos a
admin-settings-imap-intro-2 = se publican en su sala destino como el actor de correo sintetico. Configura un buzon dedicado en tu proveedor y apunta este formulario a el. La contrasena se cifra en reposo bajo
admin-settings-imap-intro-3 = Tras guardar, reinicia el servidor: la puerta de arranque lee esta fila al inicio, no en cada ciclo.
admin-settings-imap-host = Host IMAP
admin-settings-imap-port = Puerto IMAP
admin-settings-imap-port-note = 993 para IMAPS (recomendado). El interruptor de TLS de abajo deberia coincidir.
admin-settings-imap-use-tls = Usar TLS (puerto 993)
admin-settings-imap-username = Nombre de usuario IMAP
admin-settings-imap-password = Contrasena IMAP (solo escritura)
admin-settings-imap-password-keep = deja en blanco para conservar la existente
admin-settings-imap-password-unset = aun no configurada
admin-settings-imap-folder = Carpeta a sondear
admin-settings-imap-ingress-domain = Dominio de entrada
admin-settings-imap-ingress-domain-note-1 = La mitad
admin-settings-imap-ingress-domain-note-2 = de la direccion a la que escribe un remitente externo. Las direcciones de buzon por sala se convierten en
admin-settings-imap-dead-letter = Carpeta de mensajes muertos (opcional)
admin-settings-imap-dead-letter-note-1 = Cuando se define, los mensajes descartados se copian con UID-COPY a esta carpeta IMAP antes de marcarse como
admin-settings-imap-dead-letter-note-2 = en el origen. Debes crear la carpeta en tu proveedor IMAP; lets-chat no la crea automaticamente. Vacio = desactivado (los descartes solo se diagnostican por el registro).
admin-settings-imap-enable = Activar el sondeo IMAP
admin-settings-imap-enable-note = Desactivado hasta que hayas verificado la configuracion. El bucle de sondeo se niega a arrancar si faltan campos; revisa los registros del servidor tras reiniciar.
admin-settings-imap-save = Guardar ajustes de IMAP
admin-settings-uploads-heading = Subidas
admin-settings-uploads-generated-prefix = Generadas
admin-settings-uploads-generated-suffix = vista(s) previa(s).
admin-settings-uploads-purged-prefix = Purgadas
admin-settings-uploads-purged-suffix = subida(s) huerfana(s).
admin-settings-uploads-disk-size = Tamano en disco
admin-settings-uploads-orphan-rows = Filas huerfanas (subidas no adjuntas a un mensaje)
admin-settings-uploads-regenerate = Regenerar miniaturas
admin-settings-uploads-regenerate-note = Puede tardar varios minutos en despliegues grandes. Espera a que la pagina se recargue; no vuelvas a hacer clic.
admin-settings-uploads-purge = Purgar huerfanas ahora
admin-settings-defaults-heading = Valores predeterminados para nuevos usuarios
admin-settings-defaults-digest = Los nuevos usuarios empiezan con el resumen por correo activado
admin-settings-defaults-digest-note = Desactivado por defecto por privacidad. Solo afecta a futuros registros; los usuarios existentes no cambian. El resumen por correo tambien requiere SMTP configurado mediante variables de entorno.
admin-settings-defaults-save = Guardar valor predeterminado

## Slash commands
admin-slash-title = Comandos de barra
admin-slash-heading = Comandos de barra
admin-slash-intro-1 = Los comandos integrados vienen con la aplicacion. Los comandos personalizados te dejan anadir los tuyos:
admin-slash-intro-2 = sustituye
admin-slash-intro-3 = en una plantilla;
admin-slash-intro-4 = publica los argumentos como JSON a una URL y publica el cuerpo de la respuesta. Los comandos solo para administradores solo pueden ejecutarlos los administradores.
admin-slash-builtin-heading = Integrados
admin-slash-custom-heading = Personalizados
admin-slash-name-label = Nombre (sin barra)
admin-slash-kind-label = Tipo
admin-slash-description-label = Descripcion
admin-slash-description-placeholder = Publicar la plantilla de standup
admin-slash-target-label = Objetivo (texto de plantilla o URL de webhook)
admin-slash-admin-only = Solo administradores
admin-slash-add-command = Anadir comando
admin-slash-th-command = Comando
admin-slash-th-kind = Tipo
admin-slash-th-target = Objetivo
admin-slash-th-admin-only = Solo administradores
admin-slash-th-actions = Acciones
admin-slash-yes = si
admin-slash-no = no
admin-slash-delete = Eliminar
admin-slash-empty = Aun no hay comandos personalizados.

## User row
admin-userrow-role-user = usuario
admin-userrow-role-moderator = moderador
admin-userrow-role-admin = administrador
admin-userrow-save = Guardar
admin-userrow-banned = Baneado
admin-userrow-active = Activo
admin-userrow-muted = Silenciado
admin-userrow-unlimited = ilimitado
admin-userrow-unban = Desbanear
admin-userrow-ban = Banear
admin-userrow-unmute = Quitar silencio
admin-userrow-mute = Silenciar
admin-userrow-delete = Eliminar
admin-userrow-delete-confirm-prefix = Eliminar permanentemente a
admin-userrow-delete-confirm-suffix = Esto no se puede deshacer.

## Users
admin-users-title = Usuarios
admin-users-heading = Usuarios
admin-users-th-username = Nombre de usuario
admin-users-th-role = Rol
admin-users-th-status = Estado
admin-users-th-storage = Almacenamiento (usado / cuota MiB)
admin-users-th-actions = Acciones
