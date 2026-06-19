# LC-361: strings used by client-side JS (calls, voice channels, uploads,
# offline outbox, reminders, device labels). Rendered into window.__lcI18n by
# base.html; the JS reads them with an English fallback. en/js.ftl and
# es/js.ftl must stay in lockstep (the i18n_catalog gate enforces parity).
# `%name%` / `%status%` / `%n%` are literal placeholders the JS substitutes.

# Calls (call.js) + voice channels (voice.js)
js-call-mute = Mute
js-call-unmute = Unmute
js-call-start-video = Start video
js-call-stop-video = Stop video
js-call-share-screen = Share screen
js-call-stop-sharing = Stop sharing
js-call-calling = Calling %name%...
js-call-connecting = Connecting...
js-call-reconnecting = Reconnecting...
js-call-connection-failed = Connection failed
js-call-declined = Call declined
js-call-ended = Call ended
js-call-no-mic-camera = Could not access your microphone or camera.
js-call-no-camera = Could not access your camera.
js-call-no-mic = Could not access your microphone.
js-call-no-screenshare = Screen sharing is not supported by your browser.
js-call-a-contact = A contact
js-call-request-control = Request control
js-call-requesting = Requesting...
js-call-stop-controlling = Stop controlling
js-call-control-no-answer = Control request not answered
js-call-control-denied = Control request denied

# Offline outbox (outbox.js)
js-outbox-offline-queued = Offline - %n% message(s) queued
js-outbox-offline-idle = Offline - messages will send when you reconnect
js-outbox-sending = Sending %n% queued message(s)…

# Uploads + voice recording (composer.html)
js-upload-uploading-file = Uploading %name%...
js-upload-uploading-voice = Uploading voice message...
js-upload-attached = Attached: %name% (%size%)
js-upload-voice-attached = Voice message attached
js-upload-failed = Upload failed
js-upload-failed-status = Upload failed: %status%
js-upload-remove-attachment = Remove attachment
js-voice-rec-unsupported = Voice recording is not supported in this browser
js-voice-rec-mic-denied = Microphone access denied
js-voice-rec-start-failed = Could not start recording
js-voice-rec-empty = Recording was empty

# Reminders (reminders/picker.html)
js-reminder-invalid-time = Pick a valid time.
js-reminder-set-failed = Could not set reminder (%status%)
js-reminder-network-error = Network error.

# Device labels (devices.js)
js-device-microphone = Microphone
js-device-camera = Camera
js-device-speaker = Speaker
