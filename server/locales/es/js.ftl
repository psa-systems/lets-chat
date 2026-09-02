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
js-call-declined-by = %name% rechazó la llamada
js-call-missed-from = Llamada perdida de %name%
js-call-no-answer = Sin respuesta
js-call-ended = Llamada finalizada
js-call-peer-left = %name% salió de la llamada
js-call-no-mic-camera = No se pudo acceder al micrófono o la cámara.
js-call-no-camera = No se pudo acceder a la cámara.
js-call-no-mic = No se pudo acceder al micrófono.
# LC-764: se muestra cuando un cambio de silencio no logra cambiar la pista del microfono.
js-call-mic-toggle-failed = No se pudo cambiar tu micrófono. Intenta silenciar y activar de nuevo.
js-call-no-screenshare = Tu navegador no admite compartir pantalla.
js-call-a-contact = Un contacto
js-call-request-control = Solicitar control
js-call-requesting = Solicitando...
js-call-stop-controlling = Dejar de controlar
js-call-control-no-answer = Solicitud de control sin respuesta
js-call-control-denied = Solicitud de control denegada
js-call-control-unavailable = El control remoto no está disponible en esta llamada

# LC-853: flujo de consentimiento de control remoto en huddles (huddle_control.js)
js-huddle-control-granted = Control concedido
js-huddle-control-busy = Otra persona ya tiene el control
js-huddle-control-ended = Control finalizado
js-huddle-control-requested = solicitó el control de tu pantalla
js-huddle-control-active-suffix = está controlando tu pantalla

# Bandeja sin conexión (outbox.js)
js-outbox-offline-queued = Sin conexión: %n% mensaje(s) en cola
js-outbox-offline-idle = Sin conexión: los mensajes se enviarán al reconectar
js-outbox-sending = Enviando %n% mensaje(s) en cola...
js-outbox-queued-offline = %n% mensaje(s) en cola sin conexión
js-outbox-you-are-offline = Estás sin conexión
js-outbox-delivering = Entregando %n% mensaje(s) en cola...
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
js-upload-bad-type = %name% no es un tipo de archivo admitido (solo imagenes y PDF).
js-upload-too-large = %name% pesa %size%, supera el limite de %limit%.
js-voice-rec-unsupported = Este navegador no admite la grabación de voz
js-voice-rec-mic-denied = Acceso al micrófono denegado
js-voice-rec-mic-help = Permite el acceso al micrófono en los ajustes del sitio de tu navegador y vuelve a intentarlo.
js-voice-rec-start-failed = No se pudo iniciar la grabación. Comprueba que ninguna otra app esté usando el micrófono.
js-voice-rec-empty = La grabación estaba vacía
js-voice-recording = Grabando
js-voice-rec-stop = Detener
js-voice-rec-cancel = Cancelar
js-voice-play = Reproducir mensaje de voz
js-voice-re-record = Volver a grabar
js-voice-remove = Quitar
js-voice-retry = Reintentar
js-voice-uploading = Subiendo mensaje de voz...

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
# LC-768: interruptor de desenfoque de fondo en el selector de dispositivos de
# llamada, y el aviso cuando se desactiva para proteger la fluidez.
js-device-blur = Difuminar mi fondo
js-device-blur-slow = Se desactivó el desenfoque de fondo para mantener el vídeo fluido.

js-voice-you = Tú
js-voice-waiting = Esperando a que se unan otros...
js-voice-presenting = Presentando
js-conn-good = Conexión: buena
js-conn-degraded = Conexión: degradada
js-conn-reconnecting = Reconectando...
js-conn-failed = Conexión perdida
js-settings-save-error = No se pudo guardar. Inténtalo de nuevo.

# LC-496: grabador de videoclips asincronos
js-clip-choose = Grabar un clip:
js-clip-camera = Camara
js-clip-screen = Pantalla
js-clip-unsupported = La grabacion de video no es compatible con este navegador
js-clip-denied = Acceso a camara o pantalla denegado
js-clip-denied-help = Permite el acceso en la configuracion del sitio de tu navegador y vuelve a intentarlo.
js-clip-start-failed = No se pudo iniciar la grabacion.
js-clip-empty = La grabacion estaba vacia
js-clip-uploading = Subiendo clip...

# LC-611: huddle ring banner.
js-huddle-ring-started = inició una reunión rápida en
js-huddle-ring-join = Unirse
js-huddle-ring-ignore = Ignorar

# LC-650: shown as a toast when an AI action fails (htmx does not swap the 4xx).
js-ai-failed = La solicitud de IA falló. Inténtalo de nuevo.
# LC-654: inline retry button on the AI summary error state.
js-ai-retry = Reintentar
# LC-655: after Accepting a writing-assistant rewrite, an inline Undo restores the draft.
js-compose-applied = Aplicado a tu mensaje
js-compose-undo = Deshacer
# LC-822: huddle en ventana aparte (huddle_popout.js): etiqueta del boton en
# cada estado, el marcador que queda en la sala y la nota en el dock de otra sala.
js-huddle-pop-out = Sacar
js-huddle-bring-back = Traer de vuelta
js-huddle-popped-out = El huddle esta en su propia ventana
js-huddle-busy = Estas en un huddle en otra sala
# LC-825: anuncios para lectores de pantalla de las reacciones entrantes en una
# llamada, agrupados y limitados por call_reactions.js.
js-react-announce = %name% reacciono %emoji%
js-react-announce-many = %name% y %n% mas reaccionaron %emoji%
# LC-867: aviso cuando una navegacion (cambio de enclave/sala) se pierde por la
# red (htmx:sendError / htmx:timeout), para que un clic fallido no sea un no-op
# silencioso (nav.js).
js-nav-failed = No se pudo cargar - problema de conexion. Intentalo de nuevo.
# LC-866: aviso cuando la captura del microfono para transcripcion no abre tras
# cambiar el motor Rapido/Preciso (el motor del navegador aun no soltaba el
# dispositivo), para que un cambio que detuvo la captura no sea silencioso.
js-stt-mic-failed = No se pudo iniciar la transcripcion. Intentalo de nuevo.
