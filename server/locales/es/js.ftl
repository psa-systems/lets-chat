# LC-361: traducciones al espanol de las cadenas usadas por el JS del cliente.
# Debe mantenerse en paridad con en/js.ftl (la prueba i18n_catalog lo exige).
# `%name%` / `%status%` / `%n%` son marcadores literales que el JS sustituye.

# Llamadas (call.js) + canales de voz (voice.js)
js-call-mute = Silenciar
js-call-unmute = Activar micrófono
js-call-start-video = Iniciar vídeo
js-call-stop-video = Detener vídeo
js-call-share-screen = Compartir pantalla
js-call-stop-sharing = Dejar de compartir
js-call-calling = Llamando a %name%...
js-call-connecting = Conectando...
js-call-reconnecting = Reconectando...
js-call-connection-failed = Conexión fallida
js-call-declined = Llamada rechazada
js-call-ended = Llamada finalizada
js-call-no-mic-camera = No se pudo acceder al micrófono o la cámara.
js-call-no-camera = No se pudo acceder a la cámara.
js-call-no-mic = No se pudo acceder al micrófono.
js-call-no-screenshare = Tu navegador no admite compartir pantalla.
js-call-a-contact = Un contacto
js-call-request-control = Solicitar control
js-call-requesting = Solicitando...
js-call-stop-controlling = Dejar de controlar
js-call-control-no-answer = Solicitud de control sin respuesta
js-call-control-denied = Solicitud de control denegada

# Bandeja sin conexión (outbox.js)
js-outbox-offline-queued = Sin conexión: %n% mensaje(s) en cola
js-outbox-offline-idle = Sin conexión: los mensajes se enviarán al reconectar
js-outbox-sending = Enviando %n% mensaje(s) en cola…
js-outbox-queued-offline = %n% mensaje(s) en cola sin conexión
js-outbox-you-are-offline = Estás sin conexión
js-outbox-delivering = Entregando %n% mensaje(s) en cola…
js-outbox-failed = Error (%status%):
js-outbox-retry = Reintentar
js-outbox-discard = Descartar

# Subidas + grabación de voz (composer.html)
js-upload-uploading-file = Subiendo %name%...
js-upload-uploading-voice = Subiendo mensaje de voz...
js-upload-attached = Adjunto: %name% (%size%)
js-upload-voice-attached = Mensaje de voz adjunto
js-upload-failed = Error al subir
js-upload-failed-status = Error al subir: %status%
js-upload-remove-attachment = Quitar adjunto
js-voice-rec-unsupported = Este navegador no admite la grabación de voz
js-voice-rec-mic-denied = Acceso al micrófono denegado
js-voice-rec-start-failed = No se pudo iniciar la grabación
js-voice-rec-empty = La grabación estaba vacía
js-voice-recording = ● Grabando
js-voice-rec-stop = Detener
js-voice-rec-cancel = Cancelar

# Recordatorios (reminders/picker.html)
js-reminder-invalid-time = Elige una hora válida.
js-reminder-set-failed = No se pudo crear el recordatorio (%status%)
js-reminder-network-error = Error de red.

# Etiquetas de dispositivos (devices.js)
js-device-microphone = Micrófono
js-device-camera = Cámara
js-device-speaker = Altavoz
js-device-system-default = Predeterminado del sistema
js-device-dialog-title = Dispositivos de llamada
js-device-close = Cerrar
js-device-permission-hint = Permite el acceso al micrófono o la cámara para ver los nombres de los dispositivos.
js-device-show-names = Mostrar nombres de dispositivos

js-voice-you = Tú
js-voice-waiting = Esperando a que se unan otros...
js-voice-presenting = Presentando
js-conn-good = Conexión: buena
js-conn-degraded = Conexión: degradada
js-conn-reconnecting = Reconectando...
js-conn-failed = Conexión perdida
js-settings-save-error = No se pudo guardar. Inténtalo de nuevo.
