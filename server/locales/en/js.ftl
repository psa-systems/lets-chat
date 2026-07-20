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
js-call-declined-by = %name% declined the call
js-call-missed-from = Missed call from %name%
js-call-no-answer = No answer
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
js-outbox-queued-offline = %n% message(s) queued while offline
js-outbox-you-are-offline = You are offline
js-outbox-delivering = Delivering %n% queued message(s)…
js-outbox-failed = Failed (%status%):
js-outbox-retry = Retry
js-outbox-discard = Discard

# Uploads + voice recording (composer.html)
js-upload-uploading-file = Uploading %name%...
js-upload-uploading-voice = Uploading voice message...
js-upload-attached = Attached: %name% (%size%)
js-upload-voice-attached = Voice message attached
js-upload-failed = Upload failed
js-upload-failed-status = Upload failed: %status%
js-upload-remove-attachment = Remove attachment
js-upload-bad-type = %name% is not a supported file type (images and PDF only).
js-upload-too-large = %name% is %size%, over the %limit% limit.
js-voice-rec-unsupported = Voice recording is not supported in this browser
js-voice-rec-mic-denied = Microphone access denied
js-voice-rec-mic-help = Allow microphone access in your browser's site settings, then try again.
js-voice-rec-start-failed = Could not start recording. Check that no other app is using the mic.
js-voice-rec-empty = Recording was empty
js-voice-recording = Recording
js-voice-rec-stop = Stop
js-voice-rec-cancel = Cancel
js-voice-play = Play voice message
js-voice-re-record = Re-record
js-voice-remove = Remove
js-voice-retry = Retry
js-voice-uploading = Uploading voice message...

# Reminders (reminders/picker.html)
js-reminder-invalid-time = Pick a valid time.
js-reminder-set-failed = Could not set reminder (%status%)
js-reminder-network-error = Network error.

# Device labels (devices.js)
js-device-microphone = Microphone
js-device-camera = Camera
js-device-speaker = Speaker
js-device-system-default = System default
js-device-dialog-title = Call devices
js-device-close = Close
js-device-permission-hint = Allow microphone or camera access to see device names.
js-device-show-names = Show device names

js-voice-you = You
js-voice-waiting = Waiting for others to join...
js-voice-presenting = Presenting
js-conn-good = Connection: good
js-conn-degraded = Connection: degraded
js-conn-reconnecting = Reconnecting...
js-conn-failed = Connection lost
js-settings-save-error = Could not save. Please try again.

# LC-496: async video-clip recorder
js-clip-choose = Record a clip:
js-clip-camera = Camera
js-clip-screen = Screen
js-clip-unsupported = Video recording is not supported in this browser
js-clip-denied = Camera or screen access denied
js-clip-denied-help = Allow access in your browser's site settings, then try again.
js-clip-start-failed = Could not start recording.
js-clip-empty = Recording was empty
js-clip-uploading = Uploading clip...

# LC-611: huddle ring banner.
js-huddle-ring-started = started a huddle in
js-huddle-ring-join = Join
js-huddle-ring-ignore = Ignore
