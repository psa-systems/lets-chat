# LC-188: Home, search, people search, activity, and voice page chrome. Keep
# in sync with es/misc.ftl.

## Home / welcome
home-page-title = Home
home-welcome = Welcome,
home-pick-dm-prefix = Pick a DM from the sidebar, or
home-create-join-link = create or join an enclave
home-pick-dm-suffix = to chat in rooms.
home-pending-invitations-prefix = Pending invitations:
home-pending-invitations-link = view
home-pending-invitations-suffix = .
# LC-372: welcome empty-state redesign.
home-subtitle = Pick up a conversation or start something new.
home-action-dm-title = Start a direct message
home-action-dm-desc = Search for someone to chat with.
home-action-discover-title = Discover or create an enclave
home-action-discover-desc = Join a community or spin up your own.
home-action-invitations-title = View invitations
home-action-invitations-desc = See the enclaves you've been invited to.
# LC-575: Home dashboard (catch-up) cards.
home-dash-heading = Catch up
home-dash-subtitle = What happened across your workspace since you were last here.
home-dash-catchup = Unread channels
home-dash-mentions = Mentions
home-dash-threads = Threads
home-dash-dms = Direct messages
home-dash-drafts = Drafts
home-dash-empty-catchup = No unread channels. You're all caught up.
home-dash-empty-mentions = No unread mentions.
home-dash-empty-threads = No followed threads with new replies.
home-dash-empty-dms = No unread direct messages.
home-dash-empty-drafts = No saved drafts.
home-dash-unread-aria = unread
home-dash-mentions-aria = mentions
home-dash-replies-aria = new replies
# LC-703 / LC-704: preview label when the newest unread message has no text
# body (attachment-only), so the row never renders blank. Per-type labels
# mirror the timeline's render precedence; -attachment is the generic fallback.
home-dash-preview-attachment = 📎 Attachment
home-dash-preview-image = 🖼 Image
home-dash-preview-video = 🎞 Video
home-dash-preview-voice = 🎤 Voice message
home-dash-preview-file = 📎 File
# LC-705: workspace-wide "Catch me up" AI summary card on the Home dashboard.
home-catchup-ai-heading = Catch me up
home-catchup-ai-scope = A quick AI recap of everything you missed across your workspace.
home-catchup-ai-caught-up = You're all caught up. Nothing unread across your workspace.
# LC-706: composed dashboard - greeting, quick actions, and the quiet
# "all caught up" strip that collapses empty sections.
home-greeting-morning = Good morning,
home-greeting-afternoon = Good afternoon,
home-greeting-evening = Good evening,
home-greeting-fallback = Welcome back,
home-quick-new-message = New message
home-quick-jump-room = Jump to a room
home-quick-resume-draft = Resume last draft
home-quiet-heading = All caught up
# LC-707: compact "your week" glance in the greeting - number rendered inline,
# these are the trailing labels.
home-glance-messages = messages this week
home-glance-kudos = kudos this week

## Message search results
search-no-results = No results.
# LC-312: saved searches
search-save = Save search
search-saved = Saved
search-saved-heading = Saved searches
search-saved-remove = Remove saved search
# LC-699: room-header search polish.
search-scope-room = This room
search-clear = Clear search
search-searching = Searching...

## People search results
people-no-results = No matching people.
people-you = (you)

## LC-393: call transcription
transcript-page-title = Call transcript
transcript-heading = Call transcript
transcript-in-progress = in progress
transcript-empty = No speech was captured.
transcript-toggle = Transcribe
transcript-toggle-on = Stop transcription
transcript-banner = This call is being transcribed
transcript-unsupported = Live transcription is not supported in this browser.
# LC-394: transcripts archive
transcript-index-title = Transcripts
transcript-index-heading = Call transcripts
transcript-index-empty = No call transcripts yet. Turn on transcription during a call to save one here.
transcript-kind-dm = Direct call
transcript-kind-voice = Voice channel
transcript-lines = lines
# LC-395: search + export
transcript-search-placeholder = Search transcripts...
transcript-search-empty = No transcripts match your search.
# LC-440: list management (delete, bulk-clean, filter)
transcript-index-empty-content = No transcripts with content yet.
transcript-delete = Delete transcript
transcript-delete-confirm = Delete this transcript? This can't be undone.
transcript-deleted = Transcript deleted
transcript-delete-empty = Delete empty
transcript-delete-empty-confirm = Delete all empty (0-line) transcripts? This can't be undone.
transcript-deleted-n = Deleted %count% empty transcripts
transcript-hide-empty = Hide empty
transcript-hide-empty-tip = Hide 0-line transcripts
transcript-download = Download
transcript-download-txt = Text (.txt)
transcript-download-vtt = WebVTT (.vtt)
# LC-629: the uncorrected recognition, preserved alongside the AI-corrected text
transcript-download-raw = Raw (.txt)
# LC-396: AI summary
transcript-summary-heading = AI summary
transcript-summary-generate = Summarize
transcript-summary-regenerate = Regenerate
transcript-summary-working = Summarizing...
# LC-659: scope subtitle, staged "reading" label, and disclaimer, to match the
# room "Catch me up" summary polish.
transcript-summary-scope = Summary of this call
transcript-summary-reading = Reading the transcript...
transcript-summary-disclaimer = AI-generated from this transcript. May be incomplete.
# LC-664: per-viewer "what did I miss" brief.
transcript-brief-heading = What did I miss
transcript-brief-scope = Personalized for you
transcript-brief-generate = Catch me up
transcript-brief-regenerate = Regenerate
transcript-brief-reading = Reading the transcript...
transcript-brief-disclaimer = AI-generated and personalized to you. May be incomplete.

## Activity page
activity-page-title = Activity
activity-heading = Activity
# LC-750 (F28): accessible name for the filter-pill <nav> landmark.
activity-filter-label = Activity filter
activity-tab-all = All
activity-tab-mentions = Mentions
activity-tab-replies = Replies
activity-tab-reactions = Reactions
activity-new = ↑ New activity - click to refresh
activity-empty = Nothing to see here. New mentions, replies, and reactions will show up as they happen.
activity-reacted-prefix = reacted
activity-reacted-suffix = to your message
activity-replied = replied to your message
activity-mentioned = mentioned you

## Voice page
voice-badge = voice
voice-join = Join voice
voice-mute = Mute
voice-start-video = Start video
voice-share-screen = Share screen
voice-leave = Leave
voice-devices = Call devices
voice-choose-devices = Choose call devices
voice-lobby-heading-one = Someone is in this call
voice-lobby-heading-other = people in this call
voice-lobby-empty-title = No one's here yet
voice-lobby-empty-sub = Be the first to start the call
voice-transcript-panel = Transcript
voice-transcribe-tip = Start or stop transcription
voice-transcript-tip = Toggle transcript panel
voice-transcript-close = Close transcript
voice-jump-live = Jump to live
voice-transcript-empty-title = No transcript yet
voice-transcript-empty-sub = Spoken audio will appear here once transcription is on.
# LC-590: shown in the drawer when a clip fails server-side transcription.
voice-transcript-clip-failed = Some audio could not be transcribed. Captions may be incomplete.
# LC-765: transcription is per-client local capture (each browser transcribes
# only its own mic, on that device). Turning it on now auto-activates it for
# everyone in the call, including late joiners (each client starts its own
# capture), so one person's Transcribe covers the whole call. Say the privacy
# fact and the auto-activation plainly.
voice-transcript-local-only = Only your own microphone is transcribed, on your device. Turning on Transcribe activates it for everyone in the call automatically, including people who join later.
voice-in-call = In call
# LC-493: ad-hoc huddles (group text rooms).
huddle-label = Huddle
huddle-start-tip = Start or join a huddle
huddle-join = Join huddle
huddle-in-call = In huddle
huddle-empty = No one's in the huddle yet.
# LC-700: collapse/expand the huddle dock (one label for both states).
huddle-collapse = Collapse or expand huddle
huddle-lobby-one = is in the huddle
huddle-lobby-other = in the huddle
# LC-494: stage control plane (speakers vs listeners + request-to-speak).
stage-label = Stage
stage-join = Join stage
stage-leave = Leave
stage-raise-hand = Raise hand
stage-lower-hand = Lower hand
stage-step-down = Stop speaking
stage-speakers = Speakers
stage-no-speakers = No speakers yet.
stage-listeners = Listeners
stage-no-listeners = No listeners yet.
stage-approve = Approve
stage-remove-speaker = Remove speaker
stage-audio-soon = Roles and request-to-speak are live. Audio is coming soon (needs a media server).
transcript-open = Open
transcript-copy = Copy
transcript-copied = Copied
