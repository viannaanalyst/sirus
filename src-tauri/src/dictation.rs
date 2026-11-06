//! Native macOS dictation: bounded microphone capture into a private temporary
//! file, transcribed once on stop with the macOS 26 `DictationTranscriber`.
//! There are no live events; `stop_dictation` returns the final transcript.
//! A phone's own recording (the iPhone home-screen app, where web speech recognition
//! never delivers words) is transcribed the same way by `transcribe_audio`.
//! Non-macOS builds expose stubs so the command surface stays stable.

use crate::error::Result;

#[cfg(target_os = "macos")]
mod platform {
    use std::path::PathBuf;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::Bool;
    use objc2::AnyThread;
    use objc2_avf_audio::{
        AVAudioApplication, AVAudioApplicationRecordPermission, AVAudioEngine, AVAudioFile,
        AVAudioPCMBuffer, AVAudioTime,
    };
    use objc2_foundation::{NSString, NSURL};
    use parking_lot::Mutex;
    use speech::{
        AuthorizationStatus, DictationContentHint, DictationTranscriber,
        DictationTranscriberOptions, DictationTranscriptionOption, SpeechError, SpeechRecognizer,
    };

    use crate::error::{Error, Result};

    type TapBlock = RcBlock<dyn Fn(NonNull<AVAudioPCMBuffer>, NonNull<AVAudioTime>)>;

    /// Recording is bounded so a forgotten session cannot fill the disk.
    const MAX_RECORDING_SECONDS: usize = 300;
    const TEMP_PREFIX: &str = "sirus-dictation-";

    struct Session {
        engine: Retained<AVAudioEngine>,
        file: Retained<AVAudioFile>,
        path: PathBuf,
        locale: String,
        _tap: TapBlock,
    }

    // Control of one AVAudioEngine is serialized by this mutex; Apple's engine
    // and file APIs are safe to drive from any one thread.
    unsafe impl Send for Session {}

    static SESSION: Mutex<Option<Session>> = Mutex::new(None);

    fn denied_message() -> Error {
        Error::new(
            "invalid",
            "Speech recognition permission was denied. Enable it in System Settings → Privacy & Security → Speech Recognition.",
        )
    }

    fn language_unavailable_message() -> Error {
        Error::new(
            "invalid",
            "Speech recognition is unavailable for this language.",
        )
    }

    pub fn status() -> &'static str {
        match SpeechRecognizer::authorization_status() {
            AuthorizationStatus::Authorized => "available",
            AuthorizationStatus::Denied => "denied",
            AuthorizationStatus::Restricted => "restricted",
            _ => "notDetermined",
        }
    }

    fn authorize_speech() -> Result<()> {
        let status = SpeechRecognizer::authorization_status();
        if status == AuthorizationStatus::Authorized {
            return Ok(());
        }
        if status == AuthorizationStatus::Denied || status == AuthorizationStatus::Restricted {
            return Err(denied_message());
        }
        match SpeechRecognizer::request_authorization() {
            Ok(AuthorizationStatus::Authorized) => Ok(()),
            Ok(_) => Err(denied_message()),
            Err(SpeechError::TimedOut(_)) => Err(Error::new(
                "invalid",
                "Speech recognition permission timed out.",
            )),
            Err(_) => Err(denied_message()),
        }
    }

    fn authorize_microphone() -> Result<()> {
        let application = unsafe { AVAudioApplication::sharedInstance() };
        let permission = unsafe { application.recordPermission() };
        if permission == AVAudioApplicationRecordPermission::Granted {
            return Ok(());
        }
        if permission == AVAudioApplicationRecordPermission::Denied {
            return Err(Error::new("invalid", "Microphone permission was denied. Enable it in System Settings → Privacy & Security → Microphone."));
        }
        let (sender, receiver) = mpsc::channel();
        let handler: RcBlock<dyn Fn(Bool)> = RcBlock::new(move |granted: Bool| {
            let _ = sender.send(granted.as_bool());
        });
        unsafe { AVAudioApplication::requestRecordPermissionWithCompletionHandler(&handler) };
        let granted = receiver
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| Error::new("invalid", "Microphone permission timed out."))?;
        if granted {
            Ok(())
        } else {
            Err(Error::new("invalid", "Microphone permission was denied. Enable it in System Settings → Privacy & Security → Microphone."))
        }
    }

    /// Removes recording files left behind by a crash or force-quit.
    fn cleanup_stale_files() {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(TEMP_PREFIX) && (name.ends_with(".caf") || name.ends_with(".m4a")) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    fn stop_engine(session: &Session) {
        unsafe {
            session.engine.stop();
            session.engine.inputNode().removeTapOnBus(0);
            session.file.close();
        }
    }

    pub fn start(locale: &str) -> Result<()> {
        discard();
        authorize_speech()?;
        authorize_microphone()?;
        let resolved = DictationTranscriber::supported_locale_equivalent_to(locale)
            .map_err(|_| language_unavailable_message())?
            .ok_or_else(language_unavailable_message)?;
        cleanup_stale_files();
        let path = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}.caf", uuid::Uuid::new_v4()));
        unsafe {
            let engine = AVAudioEngine::new();
            let input = engine.inputNode();
            let format = input.outputFormatForBus(0);
            if format.sampleRate() <= 0.0 || format.channelCount() == 0 {
                return Err(Error::new(
                    "invalid",
                    "No microphone input device is available.",
                ));
            }
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            let settings = format.settings();
            let file = AVAudioFile::initForWriting_settings_error(
                AVAudioFile::alloc(),
                &url,
                &settings,
            )
            .map_err(|error| Error::new("native", error.localizedDescription().to_string()))?;
            let file_for_tap = file.clone();
            let frames = Arc::new(AtomicUsize::new(0));
            let frames_for_tap = Arc::clone(&frames);
            let max_frames = (format.sampleRate() * MAX_RECORDING_SECONDS as f64) as usize;
            let tap: TapBlock = RcBlock::new(
                move |buffer: NonNull<AVAudioPCMBuffer>, _time: NonNull<AVAudioTime>| {
                    let buffer = buffer.as_ref();
                    let written = frames_for_tap.load(Ordering::Relaxed);
                    if written >= max_frames {
                        return;
                    }
                    frames_for_tap.fetch_add(buffer.frameLength() as usize, Ordering::Relaxed);
                    if let Err(error) = file_for_tap.writeFromBuffer_error(buffer) {
                        tracing::warn!(
                            error = %error.localizedDescription(),
                            "dictation: audio write failed"
                        );
                    }
                },
            );
            input.installTapOnBus_bufferSize_format_block(
                0,
                1024,
                Some(&format),
                RcBlock::as_ptr(&tap),
            );
            engine.prepare();
            engine
                .startAndReturnError()
                .map_err(|error| Error::new("native", error.localizedDescription().to_string()))?;
            tracing::info!(locale = resolved, "dictation: recording started");
            *SESSION.lock() = Some(Session {
                engine,
                file,
                path,
                locale: resolved,
                _tap: tap,
            });
        }
        Ok(())
    }

    fn discard() {
        if let Some(session) = SESSION.lock().take() {
            stop_engine(&session);
            let _ = std::fs::remove_file(&session.path);
            drop(session);
        }
    }

    fn map_speech_error(error: SpeechError) -> Error {
        match error {
            SpeechError::NotAuthorized(_) => denied_message(),
            SpeechError::RecognizerUnavailable(_) => language_unavailable_message(),
            _ => Error::new("native", "Dictation failed."),
        }
    }

    fn transcribe(path: &std::path::Path, locale: &str) -> Result<Option<String>> {
        let transcriber = DictationTranscriber::with_options(
            locale,
            DictationTranscriberOptions::new()
                .with_content_hints([DictationContentHint::ShortForm])
                .with_transcription_options([DictationTranscriptionOption::Punctuation]),
        );
        let results = transcriber
            .transcribe_in_path(path)
            .map_err(map_speech_error)?;
        let text = |final_only: bool| -> Vec<String> {
            results
                .iter()
                .filter(|result| !final_only || result.is_final)
                .map(|result| result.text.trim().to_owned())
                .filter(|text| !text.is_empty())
                .collect()
        };
        let mut parts = text(true);
        if parts.is_empty() {
            parts = text(false);
        }
        if parts.is_empty() {
            return Ok(None);
        }
        Ok(Some(parts.join(" ")))
    }

    /// A recording made elsewhere (AAC in MP4, as an iPhone records it), transcribed here.
    pub fn transcribe_recording(locale: &str, audio: &[u8]) -> Result<Option<String>> {
        authorize_speech()?;
        let resolved = DictationTranscriber::supported_locale_equivalent_to(locale)
            .map_err(|_| language_unavailable_message())?
            .ok_or_else(language_unavailable_message)?;
        let path = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}.m4a", uuid::Uuid::new_v4()));
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options
                .open(&path)
                .and_then(|mut file| file.write_all(audio))
                .map_err(|_| Error::new("native", "Dictation failed."))?;
        }
        // Into the same linear PCM file the Mac's own recording makes, which the transcriber reads.
        let pcm = path.with_extension("caf");
        let converted = std::process::Command::new("/usr/bin/afconvert")
            .args(["-f", "caff", "-d", "LEI16"])
            .arg(&path)
            .arg(&pcm)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        let _ = std::fs::remove_file(&path);
        if !converted {
            let _ = std::fs::remove_file(&pcm);
            return Err(Error::new("invalid", "The recording could not be read."));
        }
        let outcome = transcribe(&pcm, &resolved);
        let _ = std::fs::remove_file(&pcm);
        outcome
    }

    pub fn transcription_available() -> bool {
        !matches!(
            SpeechRecognizer::authorization_status(),
            AuthorizationStatus::Denied | AuthorizationStatus::Restricted
        )
    }

    pub fn stop(cancel: bool) -> Result<Option<String>> {
        let Some(session) = SESSION.lock().take() else {
            return Ok(None);
        };
        stop_engine(&session);
        let path = session.path.clone();
        let locale = session.locale.clone();
        drop(session);
        if cancel {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        }
        let outcome = transcribe(&path, &locale);
        let _ = std::fs::remove_file(&path);
        match &outcome {
            Ok(Some(text)) => tracing::info!(text_len = text.len(), "dictation: transcript ready"),
            Ok(None) => tracing::info!("dictation: no speech detected"),
            Err(error) => tracing::warn!(error = %error, "dictation: transcription failed"),
        }
        outcome
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use crate::error::{Error, Result};

    pub fn status() -> &'static str {
        "unsupported"
    }

    pub fn start(_locale: &str) -> Result<()> {
        Err(Error::new(
            "invalid",
            "Dictation is only available on macOS.",
        ))
    }

    pub fn stop(_cancel: bool) -> Result<Option<String>> {
        Ok(None)
    }

    pub fn transcribe_recording(_locale: &str, _audio: &[u8]) -> Result<Option<String>> {
        Err(Error::new(
            "invalid",
            "Dictation is only available on macOS.",
        ))
    }

    pub fn transcription_available() -> bool {
        false
    }
}

pub fn dictation_status() -> String {
    platform::status().to_string()
}

pub async fn start_dictation(locale: String) -> Result<()> {
    let locale = if locale == "en" { "en-US" } else { "pt-BR" }.to_string();
    crate::commands::native_task(move || platform::start(&locale)).await
}

pub async fn stop_dictation(cancel: bool) -> Result<Option<String>> {
    crate::commands::native_task(move || platform::stop(cancel)).await
}

/// A phone's recording, base64: at most this many bytes once decoded (about ten minutes of AAC).
const MAX_RECORDING_BYTES: usize = 12 * 1024 * 1024;

/// Whether this computer can transcribe a recording sent to it (`transcribe_audio`).
pub fn transcription_status() -> String {
    if platform::transcription_available() {
        "available"
    } else {
        "unsupported"
    }
    .to_string()
}

/// Transcribes a recording a phone made (AAC in MP4) with this Mac's speech recognition.
pub async fn transcribe_audio(locale: String, audio: String) -> Result<Option<String>> {
    use base64::Engine as _;
    let locale = if locale == "en" { "en-US" } else { "pt-BR" }.to_string();
    if audio.len() > MAX_RECORDING_BYTES / 3 * 4 + 4 {
        return Err(crate::error::Error::new(
            "invalid",
            "The recording is too long.",
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(audio.as_bytes())
        .map_err(|_| crate::error::Error::new("invalid", "The recording could not be read."))?;
    if bytes.is_empty() {
        return Ok(None);
    }
    crate::commands::native_task(move || platform::transcribe_recording(&locale, &bytes)).await
}
