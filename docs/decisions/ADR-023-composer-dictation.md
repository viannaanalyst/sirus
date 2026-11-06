# ADR-023: Composer dictation through the Web Speech API

**Status:** Superseded by [ADR-024](ADR-024-dictation-removed-native-path.md)

## Context

Users want to dictate prompts into the composer with a microphone button
next to Send. Its implementation records WAV audio and ships it to the provider
session for server-side transcription, which Sirus does not have: we do
not talk to vendor transcription APIs and do not store vendor credentials.

## Decision

- Dictation uses the platform Web Speech API (`SpeechRecognition` /
  `webkitSpeechRecognition`) exposed by WKWebView on macOS 14.5+, wrapped in
  `src/lib/voice-dictation.ts`. The composer renders a mic control
  (`ComposerDictationButton`) only when the API exists; live interim text shows
  beside the control and final transcripts append to the composer draft at the
  cursor focus.
- The language follows `AppSettings.locale` (`pt-BR` / `en-US`). Sessions are
  app-controlled: starting is an explicit click, `onend`/`onerror` stop the
  control, and unmount aborts the session. Nothing is recorded to disk; the
  transcript only becomes composer text.
- Microphone access is granted by the OS at first use. Tauri/wry already
  implements `WKUIDelegate.requestMediaCapturePermissionForOrigin`, and
  `src-tauri/Info.plist` adds `NSMicrophoneUsageDescription` and
  `NSSpeechRecognitionUsageDescription` so the TCC prompt has a reason string in
  the bundled app.

## Consequences

- **Positive:** dictated prompts with no new provider credentials, no audio
  persistence, and no transcription server; the control disappears where the API
  is unavailable, so the typed path is unchanged.
- **Negative:** recognition is performed by Apple's speech service (network
  round-trips, availability varies), macOS versions before 14.5 (or a denied
  permission) have no dictation, and we depend on WKWebView exposing the
  prefixed API.
- **Accepted trade-off:** audio leaves the machine through Apple's service while
  dictating; the app does not store or forward the audio itself.

## Alternatives considered

- Native `SFSpeechRecognizer` + AVAudioEngine in Rust: heavier, needs new
  framework bindings and streaming events for no user-visible gain over the
  WebView API.
- Server transcription: requires vendor audio endpoints and
  credentials the project deliberately does not handle.
- Recording audio attachments only: does not satisfy dictating prompt text.
