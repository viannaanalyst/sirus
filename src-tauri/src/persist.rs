use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::{AppData, Message};
use crate::transcript_store::{Counts, Origin};

/// Per-session transcripts live next to `state.json` (ADR-047).
const SESSIONS_DIR: &str = "sessions";

/// The index records each transcript's counts, so views and checks never read an
/// unloaded transcript for them (ADR-099).
const MESSAGE_COUNT: &str = "messageCount";
const TRANSCRIPT_LENGTH: &str = "transcriptLength";
/// The last reply's preview for the sidebar's hover card (ADR-139), recorded beside the
/// counts so an unloaded transcript is never read for it.
pub(crate) const LAST_REPLY_PREVIEW: &str = "lastReplyPreview";

/// Where a state path's data lives (ADR-114): its SQLite database, or the JSON files of
/// earlier versions (kept only until they are migrated, and as the fallback when a
/// migration cannot be verified).
static BACKENDS: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();

fn set_backend(path: &Path, database: bool) {
    let mut all = BACKENDS.get_or_init(Mutex::default).lock();
    if !all.contains_key(path) && all.len() >= 64 {
        all.clear();
    }
    all.insert(path.to_path_buf(), database);
}

/// Whether the state path is stored in SQLite.
pub(crate) fn uses_database(path: &Path) -> bool {
    if let Some(known) = BACKENDS.get_or_init(Mutex::default).lock().get(path) {
        return *known;
    }
    crate::store::exists(path)
}

/// Where the JSON files are copied, untouched, before their first migration.
pub(crate) const JSON_BACKUP_DIR: &str = "json-backup-before-sqlite";

/// Loads the state (ADR-114): from the database when there is one; otherwise the JSON
/// files of earlier versions are loaded as before, copied aside untouched and moved into
/// a new database, which is read back and compared before it is used. A fresh install
/// starts with an empty database.
pub fn load_or_create(path: &Path) -> Result<AppData> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    #[cfg(test)]
    if tests::pinned_json(path) {
        set_backend(path, false);
        return if path.exists() {
            load_json(path)
        } else {
            let data = AppData::default();
            save(path, &data)?;
            Ok(data)
        };
    }
    if crate::store::exists(path) {
        if let Some(index) = crate::store::read_index(path)? {
            set_backend(path, true);
            return load_from(path, index, Source::Database);
        }
    }
    if path.exists() {
        set_backend(path, false);
        backup_json(path)?;
        let data = load_json(path)?;
        return Ok(migrate(path, data));
    }
    crate::store::open(path)?;
    set_backend(path, true);
    let data = AppData::default();
    save(path, &data)?;
    Ok(data)
}

/// Copies `state.json` and `sessions/` aside once, before the first migration.
fn backup_json(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let backup = parent.join(JSON_BACKUP_DIR);
    if backup.exists() {
        return Ok(());
    }
    let staging = parent.join(format!("{JSON_BACKUP_DIR}.tmp"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(staging.join(SESSIONS_DIR))?;
    if let Some(name) = path.file_name() {
        fs::copy(path, staging.join(name))?;
    }
    if let Ok(entries) = fs::read_dir(sessions_dir(path)) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_file()) {
                fs::copy(
                    entry.path(),
                    staging.join(SESSIONS_DIR).join(entry.file_name()),
                )?;
            }
        }
    }
    fs::rename(&staging, &backup)?;
    Ok(())
}

/// Moves JSON-loaded data into a new database. Every transcript is read, written, read
/// back and compared; on any difference the database is removed and the JSON files stay
/// in use, so a failed migration never loses or hides anything.
fn migrate(path: &Path, data: AppData) -> AppData {
    match migrate_into_database(path, &data) {
        Ok(()) => {
            set_backend(path, true);
            tracing::info!("the state moved from JSON files into SQLite");
        }
        Err(error) => {
            tracing::error!(%error, "the state could not move into SQLite; JSON files stay in use");
            crate::store::discard(path);
            set_backend(path, false);
        }
    }
    data
}

fn migrate_into_database(path: &Path, data: &AppData) -> Result<()> {
    crate::store::discard(path);
    let mut transcripts: Vec<(String, u64, Vec<Message>)> = Vec::new();
    for session in &data.sessions {
        let messages = match session.messages.loaded() {
            Some(messages) => messages.to_vec(),
            None => {
                let file = sessions_dir(path).join(transcript_name(&session.id));
                match fs::read(&file) {
                    Ok(bytes) => serde_json::from_slice::<Transcript>(&bytes)
                        .ok()
                        .filter(|transcript| transcript.session_id == session.id)
                        .map(|transcript| transcript.messages)
                        .ok_or_else(|| {
                            Error::new(
                                "persist",
                                "a transcript could not be read for the migration",
                            )
                        })?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                    Err(error) => return Err(error.into()),
                }
            }
        };
        transcripts.push((session.id.clone(), session.messages.revision(), messages));
    }
    let index = encode_state(data)?;
    let writes: Vec<crate::store::TranscriptWrite<'_>> = transcripts
        .iter()
        .map(|(id, revision, messages)| {
            crate::store::TranscriptWrite::full(id, *revision, messages)
        })
        .collect();
    crate::store::write(path, next_generation(), &index, &writes)?;
    // Read back and compare before anything relies on it.
    let stored = crate::store::read_index(path)?
        .ok_or_else(|| Error::new("persist", "the migrated state is empty"))?;
    if stored != index {
        return Err(Error::new("persist", "the migrated index differs"));
    }
    for (id, _, messages) in &transcripts {
        // Large command output is stored apart (ADR-147): the rows hold its preview.
        let stored: Vec<_> = messages
            .iter()
            .map(|message| crate::tool_output::split(message).0)
            .collect();
        let expected = crate::activity::persisting(|| serde_json::to_value(&stored))?;
        let read = crate::store::read_messages_only(path, id)
            .ok_or_else(|| Error::new("persist", "a migrated transcript cannot be read"))?;
        let read = crate::activity::persisting(|| serde_json::to_value(&read))?;
        if read != expected {
            return Err(Error::new("persist", "a migrated transcript differs"));
        }
    }
    // Loaded transcripts are now saved at their revision.
    for session in &data.sessions {
        if session.messages.is_loaded() {
            crate::store::mark_saved(path, &session.id, session.messages.revision());
        }
    }
    Ok(())
}

/// Where `load_from` reads transcripts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Json,
    Database,
}

/// Loads the JSON layout of earlier versions (ADR-047, ADR-099).
fn load_json(path: &Path) -> Result<AppData> {
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Err(Error::new(
            "persist",
            "state file is empty; refusing to overwrite it",
        ));
    }
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|err| Error::new("persist", format!("invalid state: {err}")))?;
    load_from(path, value, Source::Json)
}

/// Builds the state from its index and recovers it (interrupted runs, stale streaming
/// flags, settings defaults), reading only transcripts that need it.
fn load_from(path: &Path, value: serde_json::Value, source: Source) -> Result<AppData> {
    let original_settings = value.get("settings").cloned();
    // Archiving was removed (ADR-173): its conversations are deleted once the app state exists.
    crate::archive_removal::note(path, crate::archive_removal::legacy_of(&value));
    let entries = value
        .get("sessions")
        .and_then(|sessions| sessions.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default();
    // A session entry that still carries `messages` is the legacy single-file layout.
    let inline = entries
        .iter()
        .filter(|session| session.get("messages").is_some())
        .filter_map(|session| session.get("id")?.as_str().map(str::to_owned))
        .collect::<HashSet<_>>();
    let stored = stored_counts(entries);
    let mut previews = stored_previews(entries);
    let mut data: AppData = serde_json::from_value(value)
        .map_err(|err| Error::new("persist", format!("invalid state: {err}")))?;
    let dir = sessions_dir(path);
    let mut known = HashMap::new();
    // A transcript set aside as unreadable may still name thumbnail files.
    let mut set_aside = false;
    // Counts were not recorded yet (state written before ADR-099): checkpoint them.
    let mut counted = false;
    // Previews were not recorded yet (state written before ADR-139's hover card).
    let mut previewed = false;
    for session in &mut data.sessions {
        if inline.contains(&session.id) {
            continue;
        }
        let name = transcript_name(&session.id);
        known.insert(name.clone(), FileState::default());
        let origin = Origin::new(path, &session.id);
        // A session that was running at shutdown is recovered below, with its transcript.
        let active = session.status.is_active();
        if let (false, Some(counts)) = (active, stored.get(&session.id)) {
            session.messages.set_unloaded(origin, *counts);
            let preview = match previews.remove(&session.id) {
                Some(preview) => Some(preview),
                // Read once from the newest rows, then recorded with the counts.
                None if source == Source::Database && counts.conversation > 0 => {
                    let found = crate::store::read_reply_preview(path, &session.id);
                    previewed |= found.is_some();
                    found
                }
                None => None,
            };
            session.messages.set_stored_preview(preview);
            continue;
        }
        let (messages, hash) = if source == Source::Database {
            (crate::store::read_messages(path, &session.id)?, None)
        } else {
            let file = dir.join(&name);
            let existed = file.exists();
            let read = read_transcript(&file, &session.id)?;
            set_aside |= existed && read.is_none();
            read.map_or((Vec::new(), None), |(m, h)| (m, Some(h)))
        };
        if active {
            session.messages.install(messages);
            if source == Source::Database {
                crate::store::mark_saved(path, &session.id, session.messages.revision());
            }
            known.insert(
                name,
                FileState {
                    revision: Some(session.messages.revision()),
                    hash,
                },
            );
        } else {
            let preview = crate::reply_preview::of(&messages);
            session.messages.set_unloaded(origin, Counts::of(&messages));
            session.messages.set_stored_preview(preview);
            counted = true;
        }
    }
    if source == Source::Json {
        remove_orphans(&dir, &data);
        seed_written(path, known);
    }
    crate::sidebar::prune(&mut data);
    crate::context_text::prune(&mut data);
    crate::appearance::normalize(&mut data.settings);
    // Closed enum migration (legacy System → Dark), defaults and numeric bounds
    // are checkpointed once so retained preferences already use the new schema.
    let mut recovered = original_settings.as_ref() != Some(&serde_json::to_value(&data.settings)?);
    recovered |= counted || previewed;
    if !inline.is_empty() {
        // One-time split: keep the legacy file before rewriting it without transcripts.
        let backup = path.with_extension("json.pre-split-backup");
        if !backup.exists() {
            fs::copy(path, &backup)?;
        }
        recovered = true;
    }
    // A normal quit saves running turns as stopped, so what is still active was cut by a
    // crash; those cut while the agent worked may continue (ADR-118).
    let interrupted: Vec<String> = data
        .sessions
        .iter()
        .filter(|session| crate::crash_resume::cut_mid_turn(session))
        .map(|session| session.id.clone())
        .collect();
    crate::crash_resume::note_interrupted(interrupted);
    for session in &mut data.sessions {
        if session.status.is_active() {
            session.status = crate::models::SessionStatus::Stopped;
            session.last_error = Some("Execution was interrupted when Sirus Code closed.".into());
            recovered = true;
        }
        recovered |= crate::project_scripts::recover(session);
        // Only active-at-shutdown and legacy inline transcripts are loaded here; the
        // others get the same recovery when they are read (`transcript_store::install`).
        if !session.messages.is_loaded() {
            continue;
        }
        crate::activity::recover(session);
        // Older transcripts embedded image thumbnails; they move to files once.
        recovered |= crate::thumbnails::migrate(path, &mut session.messages);
        if session.messages.iter().any(|message| message.streaming) {
            for message in &mut session.messages {
                message.streaming = false;
            }
            recovered = true;
        }
    }
    recovered |= crate::team::recover(&mut data);
    if recovered {
        save(path, &data)?;
    }
    if !set_aside {
        crate::thumbnails::remove_orphans_later(path, &data);
    }
    crate::diagnostics::observe(path, &data);
    Ok(data)
}

/// Last reply previews the index recorded per session id.
fn stored_previews(entries: &[serde_json::Value]) -> HashMap<String, String> {
    entries
        .iter()
        .filter_map(|entry| {
            Some((
                entry.get("id")?.as_str()?.to_owned(),
                entry.get(LAST_REPLY_PREVIEW)?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// Counts the index recorded per session id.
fn stored_counts(entries: &[serde_json::Value]) -> HashMap<String, Counts> {
    entries
        .iter()
        .filter_map(|entry| {
            let count = |key: &str| {
                entry
                    .get(key)?
                    .as_u64()
                    .and_then(|count| usize::try_from(count).ok())
            };
            Some((
                entry.get("id")?.as_str()?.to_owned(),
                Counts {
                    messages: count(MESSAGE_COUNT)?,
                    conversation: count(TRANSCRIPT_LENGTH)?,
                },
            ))
        })
        .collect()
}

thread_local! {
    static OMIT_TRANSCRIPTS: Cell<bool> = const { Cell::new(false) };
}

/// `skip_serializing_if` for `Session::messages`. `state.json`, `load_state` and
/// session events omit transcripts: each has its own file and its own loader.
pub fn omit_transcripts<T>(_: &T) -> bool {
    OMIT_TRANSCRIPTS.with(Cell::get)
}

/// Runs `serialize` with `Session::messages` omitted on this thread.
pub fn without_transcripts<R>(serialize: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            OMIT_TRANSCRIPTS.with(|flag| flag.set(self.0));
        }
    }
    let _restore = Restore(OMIT_TRANSCRIPTS.with(|flag| flag.replace(true)));
    serialize()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptRef<'a> {
    session_id: &'a str,
    messages: &'a [Message],
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transcript {
    session_id: String,
    messages: Vec<Message>,
}

fn sessions_dir(path: &Path) -> PathBuf {
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(SESSIONS_DIR)
}

/// State content never chooses a path: unusual IDs are hashed into the name.
fn transcript_name(id: &str) -> String {
    if (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        format!("{id}.json")
    } else {
        use sha2::Digest;
        format!("h-{:x}.json", sha2::Sha256::digest(id.as_bytes()))
    }
}

fn origin_file(origin: &Origin) -> PathBuf {
    sessions_dir(&origin.state_path).join(transcript_name(&origin.session_id))
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// A missing file is an empty transcript. Unparseable or foreign content is set
/// aside (never overwritten); other read errors refuse to load the state.
fn read_transcript(file: &Path, session_id: &str) -> Result<Option<(Vec<Message>, u64)>> {
    let bytes = match fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Error::new(
                "persist",
                format!("cannot read a session transcript: {error}"),
            ))
        }
    };
    match serde_json::from_slice::<Transcript>(&bytes) {
        Ok(transcript) if transcript.session_id == session_id => {
            Ok(Some((transcript.messages, content_hash(&bytes))))
        }
        _ => {
            let aside = file.with_extension(format!(
                "corrupt-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|time| time.as_secs())
                    .unwrap_or_default()
            ));
            fs::rename(file, &aside)?;
            tracing::error!("an unreadable session transcript was kept aside");
            Ok(None)
        }
    }
}

/// A transcript read on demand, recovered like a startup load: stale streaming
/// flags cleared and embedded thumbnails moved to files.
pub struct Read {
    pub messages: Vec<Message>,
    /// Hash of the file as read; `None` when there was no file.
    pub hash: Option<u64>,
    /// Recovery changed it: it is unsaved until the next save.
    pub changed: bool,
}

/// Reads an unloaded transcript to load it (ADR-099).
pub fn read_origin(origin: &Origin) -> Result<Read> {
    let (mut messages, hash) = if uses_database(&origin.state_path) {
        (
            crate::store::read_messages(&origin.state_path, &origin.session_id)?,
            None,
        )
    } else {
        match read_transcript(&origin_file(origin), &origin.session_id)? {
            Some((messages, hash)) => (messages, Some(hash)),
            None => (Vec::new(), None),
        }
    };
    let mut changed = crate::thumbnails::migrate(&origin.state_path, &mut messages);
    for message in messages.iter_mut().filter(|message| message.streaming) {
        message.streaming = false;
        changed = true;
    }
    Ok(Read {
        messages,
        hash,
        changed,
    })
}

/// A scan's read: never sets anything aside or migrates. `None` when unreadable.
pub fn read_only(origin: &Origin) -> Option<Vec<Message>> {
    if uses_database(&origin.state_path) {
        return crate::store::read_messages_only(&origin.state_path, &origin.session_id);
    }
    let bytes = match fs::read(origin_file(origin)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(Vec::new()),
        Err(_) => return None,
    };
    serde_json::from_slice::<Transcript>(&bytes)
        .ok()
        .filter(|transcript| transcript.session_id == origin.session_id)
        .map(|transcript| transcript.messages)
}

/// The file's bytes for literal scans; an absent file is empty.
pub fn read_raw(origin: &Origin) -> Option<Vec<u8>> {
    if uses_database(&origin.state_path) {
        return crate::store::read_raw(&origin.state_path, &origin.session_id);
    }
    match fs::read(origin_file(origin)) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
        Err(_) => None,
    }
}

/// Transcript files (and interrupted temp files) of sessions the state no longer lists.
fn remove_orphans(dir: &Path, data: &AppData) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let live = data
        .sessions
        .iter()
        .map(|session| transcript_name(&session.id))
        .collect::<HashSet<_>>();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let ours = name.ends_with(".json") || name.ends_with(".json.tmp");
        if ours && !live.contains(&name) && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Snapshot order. Taken while the caller holds the state lock, so a higher
/// generation is always a newer state.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// What is on disk for one transcript file: the revision of the session content it
/// holds (`None` when unknown, e.g. a file not read since startup) and its hash.
#[derive(Default, Clone, Copy)]
struct FileState {
    revision: Option<u64>,
    hash: Option<u64>,
}

/// What is on disk for one state file: its generation and each transcript file.
#[derive(Default)]
struct Written {
    generation: u64,
    transcripts: HashMap<String, FileState>,
}
/// The lock also serializes writers.
static WRITTEN: OnceLock<Mutex<HashMap<PathBuf, Written>>> = OnceLock::new();

fn slot<'a>(all: &'a mut HashMap<PathBuf, Written>, path: &Path) -> &'a mut Written {
    // Production has one state file; keep disposable test roots bounded.
    if !all.contains_key(path) && all.len() >= 64 {
        all.clear();
    }
    all.entry(path.to_path_buf()).or_default()
}

fn seed_written(path: &Path, transcripts: HashMap<String, FileState>) {
    let mut all = WRITTEN.get_or_init(Mutex::default).lock();
    slot(&mut all, path).transcripts = transcripts;
}

/// A transcript just read matches its file: it is saved at `revision`.
pub fn mark_saved(path: &Path, session_id: &str, revision: u64, hash: Option<u64>) {
    if uses_database(path) {
        crate::store::mark_saved(path, session_id, revision);
        return;
    }
    let mut all = WRITTEN.get_or_init(Mutex::default).lock();
    slot(&mut all, path).transcripts.insert(
        transcript_name(session_id),
        FileState {
            revision: Some(revision),
            hash,
        },
    );
}

/// Whether each `(session id, revision)` is what its file holds.
pub fn saved(path: &Path, sessions: &[(&str, u64)]) -> Vec<bool> {
    if uses_database(path) {
        return crate::store::saved(path, sessions);
    }
    let mut all = WRITTEN.get_or_init(Mutex::default).lock();
    let disk = slot(&mut all, path);
    sessions
        .iter()
        .map(|(id, revision)| {
            disk.transcripts
                .get(&transcript_name(id))
                .is_some_and(|file| file.revision == Some(*revision))
        })
        .collect()
}

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

/// The index without messages, with each session's transcript counts.
fn encode_state(data: &AppData) -> Result<serde_json::Value> {
    let mut value = without_transcripts(|| serde_json::to_value(data))?;
    if let Some(entries) = value
        .get_mut("sessions")
        .and_then(serde_json::Value::as_array_mut)
    {
        for (entry, session) in entries.iter_mut().zip(&data.sessions) {
            let counts = session.messages.counts();
            entry[MESSAGE_COUNT] = counts.messages.into();
            entry[TRANSCRIPT_LENGTH] = counts.conversation.into();
            if let Some(preview) = session.messages.reply_preview() {
                entry[LAST_REPLY_PREVIEW] = preview.into();
            }
        }
    }
    Ok(value)
}

/// Writes one transcript unless the file already holds `revision` or the same bytes.
fn write_transcript(
    dir: &Path,
    disk: &mut Written,
    session_id: &str,
    revision: u64,
    messages: &[Message],
) -> Result<bool> {
    let name = transcript_name(session_id);
    let file = disk.transcripts.entry(name.clone()).or_default();
    if file.revision == Some(revision) {
        return Ok(false);
    }
    // The live thought (ADR-101) stays out of transcript files.
    let bytes = crate::activity::persisting(|| {
        serde_json::to_vec(&TranscriptRef {
            session_id,
            messages,
        })
    })?;
    let hash = content_hash(&bytes);
    let wrote = file.hash != Some(hash);
    if wrote {
        atomic_write(&dir.join(&name), &bytes)?;
    }
    *file = FileState {
        revision: Some(revision),
        hash: Some(hash),
    };
    Ok(wrote)
}

/// Saves synchronously; callers hold the state lock and may roll back on error.
/// Writes the index and only the transcripts that are loaded and changed since they
/// were last written (ADR-099): unloaded ones keep their files untouched, and a save
/// of settings, drafts or metadata writes `state.json` alone. Files of sessions the
/// index no longer lists are removed afterwards.
pub fn save(path: &Path, data: &AppData) -> Result<()> {
    if uses_database(path) {
        let index = encode_state(data)?;
        let transcripts: Vec<crate::store::TranscriptWrite<'_>> = data
            .sessions
            .iter()
            .filter_map(|session| {
                let messages = session.messages.loaded()?;
                Some(crate::store::TranscriptWrite::full(
                    &session.id,
                    session.messages.revision(),
                    messages,
                ))
            })
            .collect();
        if crate::store::write(path, next_generation(), &index, &transcripts)? {
            // The rows hold these revisions now: checkpoints count changes from here.
            for session in &data.sessions {
                if session.messages.loaded().is_some() {
                    session.messages.mark_handed();
                }
            }
        }
        crate::diagnostics::observe(path, data);
        return Ok(());
    }
    let state = serde_json::to_vec(&encode_state(data)?)?;
    let generation = next_generation();
    let dir = sessions_dir(path);
    let mut all = WRITTEN.get_or_init(Mutex::default).lock();
    let disk = slot(&mut all, path);
    for session in &data.sessions {
        let Some(messages) = session.messages.loaded() else {
            continue;
        };
        write_transcript(
            &dir,
            disk,
            &session.id,
            session.messages.revision(),
            messages,
        )?;
    }
    atomic_write(path, &state)?;
    let live = data
        .sessions
        .iter()
        .map(|session| transcript_name(&session.id))
        .collect::<HashSet<_>>();
    let removed = disk
        .transcripts
        .keys()
        .filter(|name| !live.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for name in removed {
        match fs::remove_file(dir.join(&name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%error, "cannot remove a deleted session transcript");
                continue;
            }
        }
        disk.transcripts.remove(&name);
    }
    disk.generation = generation;
    drop(all);
    crate::diagnostics::observe(path, data);
    Ok(())
}

/// One transcript in a checkpoint: the messages from `offset` on (all of them
/// unless the rows already hold `base`, ADR-099).
struct Tail {
    session_id: String,
    revision: u64,
    offset: usize,
    base: Option<u64>,
    messages: Vec<Message>,
}

/// A checkpoint's encoded view: the index and the named transcripts. In SQLite the
/// index is partial (sessions and small app-state fields, [`encode_checkpoint_index`]).
struct Encoded {
    generation: u64,
    state: serde_json::Value,
    partial: bool,
    transcripts: Vec<Tail>,
}

/// Writes a checkpoint: its transcripts (when changed), then the index. A snapshot
/// older than what is already on disk is dropped, so a delayed checkpoint never
/// replaces a newer save. Other transcripts' files are kept. Returns the sessions
/// whose tail could not be written (they need a full checkpoint).
fn write(path: &Path, encoded: &Encoded) -> Result<Vec<String>> {
    if uses_database(path) {
        let transcripts: Vec<crate::store::TranscriptWrite<'_>> = encoded
            .transcripts
            .iter()
            .map(|tail| crate::store::TranscriptWrite {
                session_id: &tail.session_id,
                revision: tail.revision,
                messages: &tail.messages,
                offset: tail.offset,
                base: tail.base,
            })
            .collect();
        return if encoded.partial {
            crate::store::write_checkpoint(path, encoded.generation, &encoded.state, &transcripts)
        } else {
            crate::store::write(path, encoded.generation, &encoded.state, &transcripts)
                .map(|_| Vec::new())
        };
    }
    let state = serde_json::to_vec(&encoded.state)?;
    let mut all = WRITTEN.get_or_init(Mutex::default).lock();
    let disk = slot(&mut all, path);
    if disk.generation > encoded.generation {
        return Ok(Vec::new());
    }
    let dir = sessions_dir(path);
    for tail in &encoded.transcripts {
        // The JSON files are always written whole.
        debug_assert_eq!(tail.offset, 0);
        write_transcript(&dir, disk, &tail.session_id, tail.revision, &tail.messages)?;
    }
    atomic_write(path, &state)?;
    disk.generation = encoded.generation;
    Ok(Vec::new())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(tmp, path)?;
    Ok(())
}

/// Sessions whose transcripts changed since the last checkpoint, per state file.
static CHECKPOINT_SESSIONS: OnceLock<Mutex<HashMap<PathBuf, HashSet<String>>>> = OnceLock::new();

/// Coalesced, best-effort checkpoint for streaming output: at most one write per
/// second across every stream. Under the lock it encodes the sessions' index entries
/// and clones, of the transcripts that streamed, only the messages that changed since
/// they were last handed to storage (ADR-099); encoding, hashing and writing them
/// happen outside it. Other transcripts, settings and item tables are left as they
/// are on disk. Final states keep using [`save`], which writes everything that changed.
pub fn checkpoint_soon(state: &Arc<AppState>, session_id: &str) {
    {
        let mut all = CHECKPOINT_SESSIONS.get_or_init(Mutex::default).lock();
        if !all.contains_key(&state.data_path) && all.len() >= 64 {
            all.clear();
        }
        all.entry(state.data_path.clone())
            .or_default()
            .insert(session_id.to_owned());
    }
    if state.checkpoint_pending.swap(true, Ordering::AcqRel) {
        return;
    }
    let state = state.clone();
    std::thread::spawn(move || {
        std::thread::sleep(CHECKPOINT_DELAY);
        // Cleared before the snapshot, so later output schedules another checkpoint.
        state.checkpoint_pending.store(false, Ordering::Release);
        let snapshot = {
            let data = state.data.lock();
            let sessions = CHECKPOINT_SESSIONS
                .get_or_init(Mutex::default)
                .lock()
                .remove(&state.data_path)
                .unwrap_or_default();
            snapshot_sessions(&state.data_path, &data, &sessions, next_generation())
        };
        match snapshot.and_then(|encoded| write(&state.data_path, &encoded)) {
            // Another write landed in between: these go whole in the next checkpoint.
            Ok(stale) => {
                for session_id in stale {
                    checkpoint_soon(&state, &session_id);
                }
            }
            Err(error) => tracing::error!(%error, "cannot checkpoint streamed output"),
        }
    });
}

/// Taken under the state lock. In SQLite it is proportional to what streamed, not to
/// the transcripts' length: the index is the sessions' entries and the small app-state
/// fields, and each named transcript contributes the messages from the first one that
/// changed since it was last handed to storage, when the rows still hold that revision
/// (otherwise all of them, compared row by row as a save does). The JSON files keep
/// full snapshots.
fn snapshot_sessions(
    path: &Path,
    data: &AppData,
    sessions: &HashSet<String>,
    generation: u64,
) -> Result<Encoded> {
    let database = uses_database(path);
    let named = data
        .sessions
        .iter()
        .filter(|session| sessions.contains(&session.id) && session.messages.loaded().is_some())
        .collect::<Vec<_>>();
    let stored = if database {
        crate::store::revisions(
            path,
            &named
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
        )
    } else {
        vec![None; named.len()]
    };
    let transcripts = named
        .into_iter()
        .zip(stored)
        .filter_map(|(session, stored)| {
            let messages = session.messages.loaded()?;
            let offset = session.messages.changed_from(stored).min(messages.len());
            let tail = Tail {
                session_id: session.id.clone(),
                revision: session.messages.revision(),
                offset,
                base: stored.filter(|_| offset > 0),
                messages: messages[offset..].to_vec(),
            };
            session.messages.mark_handed();
            Some(tail)
        })
        .collect();
    Ok(Encoded {
        generation,
        state: if database {
            encode_checkpoint_index(data)?
        } else {
            encode_state(data)?
        },
        partial: database,
        transcripts,
    })
}

/// A checkpoint's index: every session's entry with its counts and the small
/// app-state fields, as [`encode_state`] has them. Settings, projects and review
/// guides change only through commands that save, so a checkpoint leaves
/// their rows alone instead of encoding them every second.
fn encode_checkpoint_index(data: &AppData) -> Result<serde_json::Value> {
    let mut sessions = without_transcripts(|| serde_json::to_value(&data.sessions))?;
    if let Some(entries) = sessions.as_array_mut() {
        for (entry, session) in entries.iter_mut().zip(&data.sessions) {
            let counts = session.messages.counts();
            entry[MESSAGE_COUNT] = counts.messages.into();
            entry[TRANSCRIPT_LENGTH] = counts.conversation.into();
            if let Some(preview) = session.messages.reply_preview() {
                entry[LAST_REPLY_PREVIEW] = preview.into();
            }
        }
    }
    let mut index = serde_json::Map::new();
    index.insert("sessions".into(), sessions);
    index.insert(
        "providerAccounts".into(),
        serde_json::to_value(&data.provider_accounts)?,
    );
    index.insert(
        "selectedProviderAccounts".into(),
        serde_json::to_value(&data.selected_provider_accounts)?,
    );
    index.insert(
        "composerDrafts".into(),
        serde_json::to_value(&data.composer_drafts)?,
    );
    index.insert(
        "contextTexts".into(),
        serde_json::to_value(&data.context_texts)?,
    );
    index.insert("ciAutoFix".into(), serde_json::to_value(&data.ci_auto_fix)?);
    index.insert("prWatches".into(), serde_json::to_value(&data.pr_watches)?);
    Ok(serde_json::Value::Object(index))
}

const CHECKPOINT_DELAY: Duration = Duration::from_secs(1);

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        static JSON_ONLY: std::cell::RefCell<HashSet<PathBuf>> = std::cell::RefCell::new(HashSet::new());
    }

    /// Keeps a test on the JSON layout of earlier versions (the migration's fallback).
    fn pin_json(path: &Path) {
        JSON_ONLY.with(|paths| paths.borrow_mut().insert(path.to_path_buf()));
        set_backend(path, false);
    }

    pub(super) fn pinned_json(path: &Path) -> bool {
        JSON_ONLY.with(|paths| paths.borrow().contains(path))
    }
    #[test]
    fn a_delayed_older_snapshot_never_replaces_a_newer_save() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let older = snapshot_sessions(
            &path,
            &AppData::default(),
            &HashSet::new(),
            next_generation(),
        )
        .unwrap();
        let mut newer_data = AppData::default();
        newer_data
            .composer_drafts
            .insert("session:s".into(), "newer".into());
        save(&path, &newer_data).unwrap();
        write(&path, &older).unwrap();
        let loaded = load_or_create(&path).unwrap();
        assert_eq!(loaded.composer_drafts["session:s"], "newer");
    }

    fn session(id: &str, texts: &[&str]) -> crate::models::Session {
        let messages = texts
            .iter()
            .enumerate()
            .map(|(index, text)| serde_json::json!({"id":format!("{id}-{index}"),"sessionId":id,"role":"agent","content":text,"createdAt":"t","streaming":false}))
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::json!({"id":id,"title":"Task","projectId":"p","agent":"codex","status":"completed","createdAt":"t","lastActivityAt":"t","worktree":{"path":"/fixture","branch":"main","isolated":false},"lastError":null,"messages":messages})).unwrap()
    }

    #[test]
    fn transcripts_live_in_their_own_files_and_only_changed_ones_are_rewritten() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        pin_json(&path);
        let mut data = AppData {
            sessions: vec![session("a", &["first"]), session("b", &["other"])],
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        let index: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(index["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s.get("messages").is_none()));
        let dir = temp.0.join("sessions");
        assert!(dir.join("a.json").exists() && dir.join("b.json").exists());
        // Over IPC the transcript stays inline.
        assert!(serde_json::to_value(&data.sessions[0])
            .unwrap()
            .get("messages")
            .is_some());

        // An unchanged transcript is not rewritten: a marker in its file survives the save.
        let marker = fs::read(dir.join("b.json")).unwrap();
        fs::write(
            dir.join("b.json"),
            [&marker[..marker.len() - 1], b" }"].concat(),
        )
        .unwrap();
        data.sessions[0].messages[0].content.push_str(" and more");
        save(&path, &data).unwrap();
        assert!(fs::read(dir.join("b.json")).unwrap().ends_with(b" }"));

        let loaded = load_or_create(&path).unwrap();
        assert_eq!(loaded.sessions[0].messages[0].content, "first and more");
        assert_eq!(loaded.sessions[1].messages[0].content, "other");

        // A removed session's file goes once the index no longer lists it.
        data.sessions.remove(1);
        save(&path, &data).unwrap();
        assert!(!dir.join("b.json").exists());
    }

    #[test]
    fn the_last_reply_preview_stays_beside_the_counts_of_an_unloaded_transcript() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let data = AppData {
            sessions: vec![session("a", &["First reply.", "**Done**: tests pass."])],
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        let loaded = load_or_create(&path).unwrap();
        assert!(!loaded.sessions[0].messages.is_loaded());
        assert_eq!(
            loaded.sessions[0].messages.reply_preview().as_deref(),
            Some("Done: tests pass.")
        );
        let meta = crate::transcript_view::state_meta(&loaded).unwrap();
        assert_eq!(meta["sessions"][0][LAST_REPLY_PREVIEW], "Done: tests pass.");
        assert!(
            !loaded.sessions[0].messages.is_loaded(),
            "the view never reads the transcript"
        );

        // An index written before previews were recorded reads the newest rows once and keeps it.
        crate::store::forget(&path);
        crate::store::raw(&path)
            .execute(
                "UPDATE sessions SET json = json_remove(json, '$.lastReplyPreview')",
                [],
            )
            .unwrap();
        let backfilled = load_or_create(&path).unwrap();
        assert_eq!(
            backfilled.sessions[0].messages.reply_preview().as_deref(),
            Some("Done: tests pass.")
        );
        let stored: String = crate::store::raw(&path)
            .query_row("SELECT json FROM sessions WHERE id = 'a'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(stored.contains("Done: tests pass."), "{stored}");
    }

    #[test]
    fn a_streaming_checkpoint_writes_only_the_named_transcripts() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let dir = temp.0.join("sessions");
        let mut data = AppData {
            sessions: vec![session("a", &["first"]), session("b", &["other"])],
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        data.sessions[0].messages[0].content.push_str(" streamed");
        // A change the checkpoint was not told about waits for the next full save.
        data.sessions[1].messages[0].content.push_str(" unsaved");
        data.sessions.push(session("c", &["new"]));
        let named = HashSet::from(["a".to_owned()]);
        let snapshot = snapshot_sessions(&path, &data, &named, next_generation()).unwrap();
        assert_eq!(
            snapshot.transcripts.len(),
            1,
            "only the streaming session is cloned"
        );
        write(&path, &snapshot).unwrap();
        let loaded = load_or_create(&path).unwrap();
        assert_eq!(loaded.sessions.len(), 3, "the index is current");
        assert_eq!(loaded.sessions[0].messages[0].content, "first streamed");
        assert_eq!(loaded.sessions[1].messages[0].content, "other");
        assert!(loaded.sessions[2].messages.iter().next().is_none());
        assert!(dir.join("b.json").exists(), "other transcripts are kept");
        save(&path, &data).unwrap();
        let loaded = load_or_create(&path).unwrap();
        assert_eq!(loaded.sessions[1].messages[0].content, "other unsaved");
        assert_eq!(loaded.sessions[2].messages[0].content, "new");
    }

    fn loaded_ids(data: &AppData) -> Vec<&str> {
        data.sessions
            .iter()
            .filter(|session| session.messages.is_loaded())
            .map(|session| session.id.as_str())
            .collect()
    }

    #[test]
    fn startup_loads_only_interrupted_transcripts_and_never_writes_unloaded_ones_empty() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        pin_json(&path);
        let dir = temp.0.join("sessions");
        let mut data = AppData {
            sessions: vec![
                session("a", &["first", "second"]),
                session("b", &["other"]),
                session("c", &["partial"]),
            ],
            ..AppData::default()
        };
        data.sessions[2].status = crate::models::SessionStatus::Running;
        save(&path, &data).unwrap();
        let original = fs::read(dir.join("a.json")).unwrap();
        let loaded = load_or_create(&path).unwrap();
        assert_eq!(
            loaded_ids(&loaded),
            ["c"],
            "only the interrupted turn loads"
        );
        assert_eq!(
            loaded.sessions[2].status,
            crate::models::SessionStatus::Stopped
        );
        // Counts come from the index, without reading the transcript.
        assert_eq!(loaded.sessions[0].messages.len(), 2);
        assert_eq!(loaded.sessions[0].messages.conversation_len(), 2);
        assert!(!loaded.sessions[0].messages.is_loaded());
        save(&path, &loaded).unwrap();
        assert_eq!(fs::read(dir.join("a.json")).unwrap(), original);
        assert_eq!(
            load_or_create(&path).unwrap().sessions[0].messages.len(),
            2,
            "the index keeps the counts of unloaded transcripts"
        );

        // A transcript that cannot be read stays read-only: its empty view is never saved.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut loaded = load_or_create(&path).unwrap();
            let file = dir.join("a.json");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
            if fs::read(&file).is_err() {
                let mut extra = loaded.sessions[1].messages[0].clone();
                extra.id = "new".into();
                loaded.sessions[0].messages.push(extra);
                assert!(loaded.sessions[0].messages.failed());
                save(&path, &loaded).unwrap();
            }
            fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(fs::read(&file).unwrap(), original);
            let index: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(index["sessions"][0]["messageCount"], 2);
        }
    }

    #[test]
    fn saves_write_only_changed_transcripts_and_a_settings_save_writes_only_the_index() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let dir = temp.0.join("sessions");
        let mut data = AppData {
            sessions: vec![session("a", &["first"]), session("b", &["other"])],
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        // A marker survives only while its file is not rewritten.
        let mark = |name: &str| {
            let bytes = fs::read(dir.join(name)).unwrap();
            fs::write(dir.join(name), [&bytes[..bytes.len() - 1], b" }"].concat()).unwrap();
        };
        let marked = |name: &str| fs::read(dir.join(name)).unwrap().ends_with(b" }");
        mark("a.json");
        mark("b.json");
        data.composer_drafts
            .insert("session:a".into(), "draft".into());
        data.settings.ui_font_size = 15;
        save(&path, &data).unwrap();
        assert!(
            marked("a.json") && marked("b.json"),
            "settings touch no transcript"
        );
        let index: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(index["composerDrafts"]["session:a"], "draft");
        // Reading is not a change.
        assert_eq!(data.sessions[1].messages.iter().count(), 1);
        data.sessions[0].title = "Renamed".into();
        save(&path, &data).unwrap();
        assert!(marked("a.json") && marked("b.json"));
        data.sessions[0].messages[0].content.push_str(" and more");
        save(&path, &data).unwrap();
        assert!(!marked("a.json"), "the changed transcript is written");
        assert!(marked("b.json"), "the unchanged one is not");
        assert_eq!(
            load_or_create(&path).unwrap().sessions[0].messages[0].content,
            "first and more"
        );
    }

    #[test]
    fn transcripts_load_on_demand_exactly_as_saved() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = AppData {
            sessions: vec![
                session("a", &["one", "two", "three"]),
                session("b", &["other"]),
                session("c", &[]),
            ],
            ..AppData::default()
        };
        data.sessions[0].messages[1].activity = Some(crate::activity::TurnActivity::new(
            crate::models::AgentProviderId::Codex,
            Some("model".into()),
        ));
        data.sessions[0].messages[1]
            .activity
            .as_mut()
            .unwrap()
            .ended_at = Some(5);
        // An inline visual (ADR-123) is saved and read back with its message row.
        data.sessions[0].messages[1]
            .visuals
            .push(crate::models::MessageVisual {
                id: "v1".into(),
                title: "Build times".into(),
                html: "<svg viewBox=\"0 0 10 10\"></svg>".into(),
                source: None,
                created_at: "t".into(),
                revision: 2,
            });
        save(&path, &data).unwrap();
        let state = Mutex::new(load_or_create(&path).unwrap());
        assert!(loaded_ids(&state.lock()).is_empty());
        let guard = crate::transcript_store::lock_loaded(&state, &path, &["a", "b", "c"]);
        assert_eq!(loaded_ids(&guard), ["a", "b", "c"]);
        assert_eq!(guard.sessions[0].messages[1].visuals[0].revision, 2);
        for (loaded, saved) in guard.sessions.iter().zip(&data.sessions) {
            assert_eq!(
                serde_json::to_value(&loaded.messages).unwrap(),
                serde_json::to_value(&saved.messages).unwrap()
            );
        }
        // Loaded content matches its file, so it is saved and can be released again.
        let revisions = guard
            .sessions
            .iter()
            .map(|session| (session.id.as_str(), session.messages.revision()))
            .collect::<Vec<_>>();
        assert!(saved(&path, &revisions).into_iter().all(|saved| saved));
    }

    #[test]
    fn eviction_keeps_active_unsaved_and_recently_used_transcripts() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let ids = (0..10).map(|index| format!("s{index}")).collect::<Vec<_>>();
        let data = AppData {
            sessions: ids.iter().map(|id| session(id, &["text"])).collect(),
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        let state = Mutex::new(load_or_create(&path).unwrap());
        let all = ids.iter().map(String::as_str).collect::<Vec<_>>();
        let mut data = crate::transcript_store::lock_loaded(&state, &path, &all);
        assert_eq!(
            loaded_ids(&data).len(),
            10,
            "requested transcripts all load"
        );
        data.sessions[0].status = crate::models::SessionStatus::Running;
        data.sessions[1].messages[0].content.push_str(" unsaved");
        let released = crate::transcript_store::evict(&mut data, &path, &[]);
        assert_eq!(released, 2);
        assert_eq!(
            loaded_ids(&data),
            ["s0", "s1", "s4", "s5", "s6", "s7", "s8", "s9"],
            "active and unsaved stay with the six most recently used"
        );
        assert_eq!(data.sessions[2].messages.len(), 1, "counts stay");
        save(&path, &data).unwrap();
        assert_eq!(crate::transcript_store::evict(&mut data, &path, &[]), 1);
        assert_eq!(
            loaded_ids(&data),
            ["s0", "s4", "s5", "s6", "s7", "s8", "s9"]
        );
        // A released transcript reads back with its saved change.
        drop(data);
        let data = crate::transcript_store::lock_loaded(&state, &path, &["s1"]);
        assert_eq!(data.sessions[1].messages[0].content, "text unsaved");
    }

    /// Before/after numbers for docs/PERFORMANCE.md:
    /// `cargo test --release measure_lazy_transcripts -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn measure_lazy_transcripts() {
        use std::time::Instant;
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(24);
        let texts = (0..60).map(|_| text.as_str()).collect::<Vec<_>>();
        let data = AppData {
            sessions: (0..300)
                .map(|index| session(&format!("s{index}"), &texts))
                .collect(),
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        let dir = temp.0.join("sessions");
        let on_disk = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.metadata().unwrap().len())
            .sum::<u64>();
        let started = Instant::now();
        let lazy = load_or_create(&path).unwrap();
        let lazy_load = started.elapsed();
        let resident = |data: &AppData| {
            data.sessions
                .iter()
                .filter_map(|session| session.messages.loaded())
                .map(|list| serde_json::to_vec(list).unwrap().len())
                .sum::<usize>()
        };
        let lazy_resident = resident(&lazy);
        // Before: every transcript read and kept at startup.
        let started = Instant::now();
        let state = Mutex::new(lazy);
        let all = (0..300)
            .map(|index| format!("s{index}"))
            .collect::<Vec<_>>();
        let eager = crate::transcript_store::lock_loaded(
            &state,
            &path,
            &all.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let eager_load = started.elapsed() + lazy_load;
        let eager_resident = resident(&eager);
        // Before: every save encoded and hashed every transcript.
        let started = Instant::now();
        for session in &eager.sessions {
            let bytes = serde_json::to_vec(&TranscriptRef {
                session_id: &session.id,
                messages: session.messages.loaded().unwrap(),
            })
            .unwrap();
            content_hash(&bytes);
        }
        let old_save = started.elapsed();
        let started = Instant::now();
        save(&path, &eager).unwrap();
        let new_save = started.elapsed();
        let index = fs::metadata(&path).unwrap().len();
        println!(
            "transcripts on disk {on_disk} B, index {index} B\n\
             startup: lazy {lazy_load:?} keeping {lazy_resident} B; eager {eager_load:?} keeping {eager_resident} B\n\
             metadata save: before (encode all) {old_save:?}, now {new_save:?}"
        );
    }

    #[test]
    fn a_legacy_single_file_is_backed_up_and_split_once() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut legacy = serde_json::to_value(AppData::default()).unwrap();
        legacy["sessions"] =
            serde_json::json!([serde_json::to_value(session("old", &["kept"])).unwrap()]);
        let raw = serde_json::to_string_pretty(&legacy).unwrap();
        fs::write(&path, &raw).unwrap();
        let loaded = load_or_create(&path).unwrap();
        assert_eq!(loaded.sessions[0].messages[0].content, "kept");
        assert_eq!(
            fs::read_to_string(path.with_extension("json.pre-split-backup")).unwrap(),
            raw
        );
        let index: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(index["sessions"][0].get("messages").is_none());
        assert!(temp.0.join("sessions/old.json").exists());
        assert_eq!(
            load_or_create(&path).unwrap().sessions[0].messages[0].content,
            "kept"
        );
    }

    #[test]
    fn damaged_orphan_and_unsafe_transcripts_never_lose_data_or_choose_paths() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let data = AppData {
            sessions: vec![session("good", &["ok"]), session("../escape", &["safe"])],
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        let dir = temp.0.join("sessions");
        let hashed = transcript_name("../escape");
        assert!(hashed.starts_with("h-") && dir.join(&hashed).exists());
        assert!(!temp.0.join("escape.json").exists());

        fs::write(dir.join("good.json"), b"{not json").unwrap();
        fs::write(dir.join("ghost.json"), b"{}").unwrap();
        let loaded = load_or_create(&path).unwrap();
        // Read when used: the damaged file is set aside then, never overwritten.
        assert!(loaded.sessions[0].messages.iter().next().is_none());
        assert_eq!(loaded.sessions[1].messages[0].content, "safe");
        assert!(!dir.join("ghost.json").exists(), "orphans are removed");
        let aside = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("good.corrupt-")
            })
            .expect("damaged transcript is kept aside");
        assert_eq!(fs::read(aside.path()).unwrap(), b"{not json");
    }
    #[test]
    fn embedded_thumbnails_move_to_files_on_load_and_orphans_go() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        pin_json(&path);
        let id = uuid::Uuid::new_v4().to_string();
        let orphan = uuid::Uuid::new_v4().to_string();
        let mut data = AppData {
            sessions: vec![session("s", &[])],
            ..AppData::default()
        };
        data.sessions[0].messages = vec![serde_json::from_value(serde_json::json!({
            "id":"m","sessionId":"s","role":"user","content":"look","createdAt":"t","streaming":false,
            "attachments":[{"id":id,"name":"a.png","kind":"file","thumbnail":crate::thumbnails::data_url(b"jpeg")}]
        }))
        .unwrap()]
        .into();
        save(&path, &data).unwrap();
        crate::thumbnails::store(&path, &orphan, b"old").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let loaded = load_or_create(&path).unwrap();
        // The transcript migrates when it is read, and is saved with the next save.
        let attachment = &loaded.sessions[0].messages[0].attachments[0];
        assert!(attachment.has_thumbnail && attachment.thumbnail.is_none());
        assert_eq!(crate::thumbnails::read(&path, &id).unwrap(), b"jpeg");
        save(&path, &loaded).unwrap();
        // Orphans go on a background scan after load.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while crate::thumbnails::read(&path, &orphan).is_some() {
            assert!(std::time::Instant::now() < deadline, "orphan removed");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(crate::thumbnails::read(&path, &id).unwrap(), b"jpeg");
        let saved = fs::read_to_string(temp.0.join("sessions/s.json")).unwrap();
        assert!(!saved.contains("base64") && saved.contains("\"hasThumbnail\":true"));
    }
    #[test]
    fn general_preferences_survive_native_save_and_reload() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let settings = serde_json::from_value(serde_json::json!({
            "defaultAgent": "pi", "defaultModel": "pi::fixture",
            "sidebarProjectSortOrder": "created_at", "sidebarThreadSortOrder": "updated_at",
            "showEnvironmentUsage": false, "showEnvironmentRepository": false,
            "showEnvironmentEditor": false, "locale": "en", "uiFontSize": 14
        }))
        .unwrap();
        let data = AppData {
            settings,
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        let loaded = load_or_create(&path).unwrap();
        assert_eq!(
            serde_json::to_value(loaded.settings).unwrap(),
            serde_json::to_value(data.settings).unwrap()
        );
        assert!(!path.with_extension("json.tmp").exists());
    }
    #[test]
    fn appearance_choices_survive_save_reload_and_legacy_values_checkpoint() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        pin_json(&path);
        let settings = serde_json::from_value(serde_json::json!({
            "theme": "system", "darkWindowTranslucent": true, "lightWindowTranslucent": false,
            "darkWindowOpacity": 70, "lightWindowOpacity": 92, "darkSidebarTranslucent": true,
            "lightSidebarTranslucent": true, "darkSidebarOpacity": 25,
            "lightSidebarOpacity": 100, "translucentOpacity": 70,
            "systemUiFont": false, "uiFont": "dmSans", "uiFontSize": 18,
            "codeFont": "sfMono", "codeFontSize": 22,
            "terminalFont": "jetbrains", "terminalFontSize": 10,
            "fontSmoothing": false, "dockIcon": "smokedGlass"
        }))
        .unwrap();
        let data = AppData {
            settings,
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        assert_eq!(
            serde_json::to_value(load_or_create(&path).unwrap().settings).unwrap(),
            serde_json::to_value(data.settings).unwrap()
        );
        fs::write(&path, r#"{"projects":[],"sessions":[],"settings":{"theme":"system","uiFontSize":100,"codeFontSize":1,"terminalFontSize":999,"darkSidebarOpacity":1,"lightSidebarOpacity":999,"translucentOpacity":0}}"#).unwrap();
        let migrated = load_or_create(&path).unwrap();
        assert_eq!(migrated.settings.theme, crate::models::ThemePref::System);
        assert_eq!(migrated.settings.ui_font_size, 13);
        assert_eq!(migrated.settings.code_font_size, 13);
        assert_eq!(migrated.settings.terminal_font_size, 13);
        assert_eq!(migrated.settings.dark_sidebar_opacity, 72);
        assert_eq!(migrated.settings.light_sidebar_opacity, 38);
        assert_eq!(migrated.settings.translucent_opacity, 85);
        migrated.settings.validate_controls().unwrap();
        let checkpoint: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(checkpoint["settings"]["theme"], "system");
        assert_eq!(checkpoint["settings"]["translucentOpacity"], 85);
        assert_eq!(checkpoint["settings"]["dockIcon"], "default");
        fs::write(&path, r#"{"projects":[],"sessions":[],"settings":{"theme":"translucent","translucentOpacity":43,"lightSidebarTranslucent":true}}"#).unwrap();
        let glass = load_or_create(&path).unwrap();
        assert_eq!(glass.settings.theme, crate::models::ThemePref::Dark);
        assert!(glass.settings.dark_window_translucent && glass.settings.light_sidebar_translucent);
        assert_eq!(glass.settings.dark_window_opacity, 43);
        let checkpoint: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(checkpoint["settings"]["theme"], "dark");
        assert_eq!(checkpoint["settings"]["darkWindowOpacity"], 43);
    }

    #[test]
    fn native_thread_survives_restart_but_pending_approval_does_not() {
        let mut session: crate::models::Session = serde_json::from_value(serde_json::json!({
            "id":"s","title":"Task","projectId":"p","agent":"codex","status":"waiting",
            "createdAt":"time","lastActivityAt":"time","worktree":{"path":"/fixture","branch":"main","isolated":false},"messages":[],
            "nativeThread":{"threadId":"exact-vendor-thread","sessionId":"s","projectId":"p","cwd":"/fixture","model":null},
            "pendingRequests":[{"requestId":"stale","generation":"old","turnId":"turn","itemId":"item","kind":{"type":"command","command":"echo hi","cwd":"/fixture","reason":null}}]
        })).unwrap();
        let serialized = serde_json::to_value(&session).unwrap();
        assert_eq!(
            serialized["nativeThread"]["threadId"],
            "exact-vendor-thread"
        );
        assert_eq!(serialized["pendingRequests"], serde_json::json!([]));
        session.status = crate::models::SessionStatus::Completed;
        assert_eq!(
            serde_json::to_value(session).unwrap()["nativeThread"]["sessionId"],
            "s"
        );
    }
    #[test]
    fn interrupted_sessions_recover_without_losing_messages() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data: AppData = serde_json::from_value(serde_json::json!({
            "projects": [], "settings": {},
            "sessions": [{"id":"one","title":"Task","projectId":"project","agent":"codex","status":"running","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/unused","branch":"main","isolated":false},"lastError":null,"messages":[{"id":"message","sessionId":"one","role":"agent","content":"partial output","createdAt":"time","streaming":true}]}]
        })).unwrap();
        let mut failed = data.sessions[0].clone();
        failed.id = "failed".into();
        failed.status = crate::models::SessionStatus::Failed;
        failed.last_error = Some("Output limit".into());
        data.sessions.push(failed);
        save(&path, &data).unwrap();
        let recovered = load_or_create(&path).unwrap();
        assert_eq!(
            recovered.sessions[0].status,
            crate::models::SessionStatus::Stopped
        );
        assert_eq!(recovered.sessions[0].messages[0].content, "partial output");
        assert!(!recovered.sessions[0].messages[0].streaming);
        assert!(!recovered.sessions[1].messages[0].streaming);
        assert_eq!(
            recovered.sessions[1].status,
            crate::models::SessionStatus::Failed
        );
        assert_eq!(
            recovered.sessions[1].last_error.as_deref(),
            Some("Output limit")
        );
        assert_eq!(
            load_or_create(&path).unwrap().sessions[0].status,
            crate::models::SessionStatus::Stopped
        );
    }
    #[test]
    fn corrupted_state_is_reported_without_overwriting_it() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        fs::write(&path, "{bad state").unwrap();
        assert!(load_or_create(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{bad state");
        fs::write(&path, "").unwrap();
        assert!(load_or_create(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }

    /// A new process: open connections, caches and backend choices are forgotten.
    fn restart(path: &Path) {
        crate::store::forget(path);
        BACKENDS.get_or_init(Mutex::default).lock().remove(path);
    }

    #[test]
    fn json_state_moves_into_sqlite_once_with_an_untouched_backup() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let data = AppData {
            sessions: vec![session("a", &["first", "second"]), session("b", &["other"])],
            ..AppData::default()
        };
        // An earlier version's files.
        save(&path, &data).unwrap();
        assert!(!crate::store::exists(&path));
        let original = fs::read(&path).unwrap();
        let original_a = fs::read(temp.0.join("sessions/a.json")).unwrap();
        restart(&path);

        let mut loaded = load_or_create(&path).unwrap();
        assert!(crate::store::exists(&path), "the database exists");
        let backup = temp.0.join(JSON_BACKUP_DIR);
        assert_eq!(fs::read(backup.join("state.json")).unwrap(), original);
        assert_eq!(
            fs::read(backup.join("sessions/a.json")).unwrap(),
            original_a
        );
        assert_eq!(crate::store::message_count(&path, "a").unwrap(), 2);
        assert_eq!(loaded.sessions[0].messages[1].content, "second");

        // From now on the database is written; the JSON files stay as they were.
        loaded.sessions[0].messages[0].content = "changed".into();
        save(&path, &loaded).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        restart(&path);
        let again = load_or_create(&path).unwrap();
        assert_eq!(again.sessions[0].messages[0].content, "changed");
        assert_eq!(again.sessions[1].messages[0].content, "other");
        assert!(fs::read_dir(&backup).is_ok(), "the backup is kept");
    }

    #[test]
    fn review_guides_live_in_their_own_rows_and_stay_out_of_the_ui_index() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        data.sessions = vec![session("a", &["one"])];
        let content = crate::review_guides::validate(&serde_json::json!({
            "title": "Guide", "steps": [{ "title": "Step", "body": "Why", "anchors": [{ "path": "src/a.ts", "start": 2, "end": 4 }] }]
        }))
        .unwrap();
        let target = crate::review_guides::GuideTarget::Session {
            session_id: "a".into(),
        };
        crate::review_guides::write(&mut data, target.clone(), content, None, "t");
        save(&path, &data).unwrap();
        assert!(crate::store::exists(&path));
        restart(&path);
        let loaded = load_or_create(&path).unwrap();
        let guide = crate::review_guides::find(&loaded, &target).unwrap();
        assert_eq!(guide.steps[0].anchors[0].end, 4);
        let meta = crate::transcript_view::state_meta(&loaded).unwrap();
        assert!(meta.get("reviewGuides").is_none());
    }

    #[test]
    fn a_save_rewrites_only_messages_that_changed() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        data.sessions = vec![
            session("a", &["one", "two", "three"]),
            session("b", &["other"]),
        ];
        save(&path, &data).unwrap();
        // Marks on rows that must not be rewritten.
        let raw = crate::store::raw(&path);
        for (session_id, position) in [("a", 0), ("b", 0)] {
            raw.execute(
                "UPDATE messages SET json = replace(json, '\"streaming\":false', '\"streaming\":false ') WHERE session_id = ?1 AND position = ?2",
                rusqlite::params![session_id, position],
            )
            .unwrap();
        }
        data.sessions[0].messages[2].content.push_str(" and more");
        data.sessions[0].messages.push(serde_json::from_value(serde_json::json!({"id":"a-3","sessionId":"a","role":"agent","content":"four","createdAt":"t","streaming":false})).unwrap());
        save(&path, &data).unwrap();
        let marked = |session_id: &str, position: i64| -> bool {
            raw.query_row(
                "SELECT json FROM messages WHERE session_id = ?1 AND position = ?2",
                rusqlite::params![session_id, position],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
            .contains("false ")
        };
        assert!(
            marked("a", 0) && marked("b", 0),
            "unchanged rows are not rewritten"
        );
        restart(&path);
        let loaded = load_or_create(&path).unwrap();
        let texts: Vec<&str> = loaded.sessions[0]
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(texts, ["one", "two", "three and more", "four"]);

        // A shorter transcript and a removed session leave no rows behind.
        let mut data = loaded;
        data.sessions[0].messages.truncate(1);
        data.sessions.remove(1);
        save(&path, &data).unwrap();
        assert_eq!(crate::store::message_count(&path, "a").unwrap(), 1);
        assert_eq!(crate::store::message_count(&path, "b").unwrap(), 0);
        restart(&path);
        assert_eq!(load_or_create(&path).unwrap().sessions.len(), 1);
    }

    #[test]
    fn an_unfinished_write_leaves_the_last_saved_state_and_interrupted_runs_recover() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        data.sessions = vec![session("a", &["saved"])];
        data.sessions[0].status = crate::models::SessionStatus::Running;
        data.sessions[0].messages[0].streaming = true;
        save(&path, &data).unwrap();
        // A crash in the middle of a write: the transaction never commits.
        {
            let raw = crate::store::raw(&path);
            raw.execute_batch("BEGIN; UPDATE messages SET json = 'broken'; DELETE FROM sessions;")
                .unwrap();
            drop(raw);
        }
        restart(&path);
        let loaded = load_or_create(&path).unwrap();
        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.sessions[0].messages[0].content, "saved");
        // The run the crash cut is stopped and no longer streams.
        assert_eq!(
            loaded.sessions[0].status,
            crate::models::SessionStatus::Stopped
        );
        assert!(loaded.sessions[0].last_error.is_some());
        assert!(!loaded.sessions[0].messages[0].streaming);
    }

    #[test]
    fn a_migration_that_cannot_be_verified_keeps_the_json_files_in_use() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let data = AppData {
            sessions: vec![session("a", &["first"]), session("b", &["other"])],
            ..AppData::default()
        };
        save(&path, &data).unwrap();
        // A transcript the migration cannot read.
        fs::write(temp.0.join("sessions/b.json"), b"{ not json").unwrap();
        restart(&path);
        let loaded = load_or_create(&path).unwrap();
        assert!(!crate::store::exists(&path), "no half-migrated database");
        assert!(!uses_database(&path));
        assert_eq!(loaded.sessions[0].messages[0].content, "first");
        assert!(temp
            .0
            .join(JSON_BACKUP_DIR)
            .join("sessions/b.json")
            .exists());
    }

    #[test]
    fn unloaded_transcripts_are_read_on_demand_and_old_checkpoints_are_dropped() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        data.sessions = vec![session("a", &["lazy"])];
        save(&path, &data).unwrap();
        restart(&path);
        let loaded = load_or_create(&path).unwrap();
        assert!(
            !loaded.sessions[0].messages.is_loaded(),
            "finished transcripts load later"
        );
        let origin = crate::transcript_store::Origin::new(&path, "a");
        assert_eq!(read_origin(&origin).unwrap().messages[0].content, "lazy");
        assert!(String::from_utf8(read_raw(&origin).unwrap())
            .unwrap()
            .contains("\"lazy\""));

        let older = snapshot_sessions(
            &path,
            &AppData::default(),
            &HashSet::new(),
            next_generation(),
        )
        .unwrap();
        let mut newer = AppData::default();
        newer
            .composer_drafts
            .insert("session:s".into(), "newer".into());
        save(&path, &newer).unwrap();
        write(&path, &older).unwrap();
        restart(&path);
        assert_eq!(
            load_or_create(&path).unwrap().composer_drafts["session:s"],
            "newer"
        );
    }

    /// Timing of a save after one new message in a long conversation (run by hand:
    /// `cargo test --lib persist::tests::bench -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench_one_new_message_in_a_long_conversation() {
        let body = "x".repeat(1500);
        let texts: Vec<String> = (0..3000).map(|n| format!("{n} {body}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        for database in [false, true] {
            let temp = crate::git::tests::Repo::new();
            let path = temp.0.join("state.json");
            let mut data = if database {
                load_or_create(&path).unwrap()
            } else {
                pin_json(&path);
                AppData::default()
            };
            data.sessions = vec![session("big", &refs)];
            save(&path, &data).unwrap();
            let started = std::time::Instant::now();
            for round in 0..20 {
                data.sessions[0].messages.push(serde_json::from_value(serde_json::json!({"id":format!("new-{round}"),"sessionId":"big","role":"agent","content":"one more","createdAt":"t","streaming":false})).unwrap());
                save(&path, &data).unwrap();
            }
            println!(
                "{}: {:.1} ms per save of a ~4.5 MB conversation",
                if database { "SQLite" } else { "JSON" },
                started.elapsed().as_secs_f64() * 1000.0 / 20.0
            );
        }
    }

    fn message(session_id: &str, id: &str, text: &str) -> Message {
        serde_json::from_value(serde_json::json!({"id":id,"sessionId":session_id,"role":"agent","content":text,"createdAt":"t","streaming":false})).unwrap()
    }

    /// Every message row, in order, as `(session, position, id, JSON)`.
    fn message_rows(path: &Path) -> Vec<(String, i64, String, String)> {
        let raw = crate::store::raw(path);
        let mut statement = raw
            .prepare(
                "SELECT session_id, position, id, json FROM messages ORDER BY session_id, position",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        rows
    }

    #[test]
    fn a_streaming_checkpoint_writes_only_the_messages_that_changed() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        let texts: Vec<String> = (0..200).map(|n| format!("message {n}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        data.sessions = vec![session("a", &refs), session("b", &["other"])];
        save(&path, &data).unwrap();
        // Marks on every stored row: a row that is rewritten loses its mark.
        let raw = crate::store::raw(&path);
        raw.execute_batch(
            "UPDATE messages SET json = replace(json, '\"streaming\":false', '\"streaming\":false ');
             UPDATE settings SET json = json || ' ';
             UPDATE projects SET json = json || ' ';",
        )
        .unwrap();
        // Streaming: the newest message grows and a new one is appended.
        let streamed = data.sessions[0]
            .messages
            .rev_find_mut(|message| message.id == "a-199")
            .unwrap();
        streamed.content.push_str(" and more");
        data.sessions[0]
            .messages
            .append(message("a", "a-200", "next"));
        data.settings.steer_while_running = !data.settings.steer_while_running;
        let named = HashSet::from(["a".to_owned()]);
        let snapshot = snapshot_sessions(&path, &data, &named, next_generation()).unwrap();
        assert_eq!(snapshot.transcripts.len(), 1);
        assert_eq!(
            snapshot.transcripts[0].offset, 199,
            "only the tail is copied"
        );
        assert_eq!(snapshot.transcripts[0].messages.len(), 2);
        assert!(
            snapshot.state.get("settings").is_none(),
            "settings are not encoded"
        );
        assert!(write(&path, &snapshot).unwrap().is_empty());
        let rows = message_rows(&path);
        let a: Vec<_> = rows.iter().filter(|row| row.0 == "a").collect();
        assert_eq!(a.len(), 201);
        assert!(
            a[..199].iter().all(|row| row.3.contains("false ")),
            "unchanged rows are not rewritten"
        );
        assert!(a[199].3.contains("message 199 and more") && a[200].3.contains("next"));
        let settings: String = raw
            .query_row("SELECT json FROM settings", [], |row| row.get(0))
            .unwrap();
        assert!(settings.ends_with(' '), "the settings row is left alone");

        // Nothing changed since: the next checkpoint copies nothing.
        let snapshot = snapshot_sessions(&path, &data, &named, next_generation()).unwrap();
        assert!(snapshot.transcripts[0].messages.is_empty());
        write(&path, &snapshot).unwrap();

        // An edit through the whole list (an older message) makes the next one whole.
        data.sessions[0].messages[3].content.push_str(" edited");
        let snapshot = snapshot_sessions(&path, &data, &named, next_generation()).unwrap();
        assert_eq!(snapshot.transcripts[0].offset, 0);
        assert_eq!(snapshot.transcripts[0].messages.len(), 201);
        write(&path, &snapshot).unwrap();
        let rows = message_rows(&path);
        assert!(rows[3].3.contains("message 3 edited"));
        assert!(
            rows[0].3.contains("false "),
            "still only changed rows are written"
        );

        restart(&path);
        let loaded = load_or_create(&path).unwrap();
        let loaded = crate::transcript_store::lock_loaded(&Mutex::new(loaded), &path, &["a"])
            .sessions[0]
            .messages
            .to_vec();
        let expected = data.sessions[0].messages.to_vec();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
    }

    #[test]
    fn checkpointed_tails_store_the_same_rows_as_a_full_save() {
        let temp = crate::git::tests::Repo::new();
        let streamed = temp.0.join("streamed/state.json");
        let whole = temp.0.join("whole/state.json");
        let mut data = load_or_create(&streamed).unwrap();
        data.sessions = vec![session("a", &["one", "two"]), session("b", &["other"])];
        save(&streamed, &data).unwrap();
        let named = HashSet::from(["a".to_owned(), "b".to_owned()]);
        for round in 0..12 {
            if round % 4 == 0 {
                data.sessions[0]
                    .messages
                    .append(message("a", &format!("a-new-{round}"), ""));
            }
            data.sessions[0]
                .messages
                .rev_find_mut(|message| message.role == crate::models::MessageRole::Agent)
                .unwrap()
                .content
                .push_str(&format!(" chunk {round}"));
            if round == 5 {
                data.sessions[1]
                    .messages
                    .append(message("b", "b-1", "late"));
                // A save between checkpoints.
                save(&streamed, &data).unwrap();
            }
            if round == 9 {
                // A copy that diverges from what is stored is written whole.
                let copy = data.sessions[0].clone();
                data.sessions[0]
                    .messages
                    .append(message("a", "a-gone", "gone"));
                save(&streamed, &data).unwrap();
                data.sessions[0] = copy;
                data.sessions[0]
                    .messages
                    .rev_find_mut(|message| message.role == crate::models::MessageRole::Agent)
                    .unwrap()
                    .content
                    .push_str(" after the copy");
                let snapshot =
                    snapshot_sessions(&streamed, &data, &named, next_generation()).unwrap();
                assert_eq!(snapshot.transcripts[0].offset, 0);
                write(&streamed, &snapshot).unwrap();
                continue;
            }
            let snapshot = snapshot_sessions(&streamed, &data, &named, next_generation()).unwrap();
            write(&streamed, &snapshot).unwrap();
        }
        let mut fresh = load_or_create(&whole).unwrap();
        fresh.sessions = data.sessions.clone();
        save(&whole, &fresh).unwrap();
        assert_eq!(message_rows(&streamed), message_rows(&whole));
        assert!(!message_rows(&whole).iter().any(|row| row.2 == "a-gone"));
        // And a full save over the checkpoints changes nothing.
        save(&streamed, &data).unwrap();
        assert_eq!(message_rows(&streamed), message_rows(&whole));
        let sessions = |path: &Path| -> Vec<(String, String, i64)> {
            let raw = crate::store::raw(path);
            let mut statement = raw
                .prepare("SELECT id, json, message_count FROM sessions ORDER BY position")
                .unwrap();
            let rows = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            rows
        };
        assert_eq!(sessions(&streamed), sessions(&whole));
    }

    #[test]
    fn a_tail_that_no_longer_fits_the_rows_is_not_written() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        data.sessions = vec![session("a", &["one", "two"])];
        save(&path, &data).unwrap();
        let tail = [message("a", "a-2", "three")];
        let write = crate::store::TranscriptWrite {
            session_id: "a",
            revision: crate::transcript_store::next_revision(),
            messages: &tail,
            offset: 2,
            base: Some(crate::transcript_store::next_revision()),
        };
        let stale = crate::store::write_checkpoint(
            &path,
            next_generation(),
            &encode_checkpoint_index(&data).unwrap(),
            &[write],
        )
        .unwrap();
        assert_eq!(stale, ["a"]);
        assert_eq!(crate::store::message_count(&path, "a").unwrap(), 2);
    }

    #[test]
    fn the_checkpoint_index_is_the_full_index_without_settings_and_item_tables() {
        let mut data = AppData {
            sessions: vec![session("a", &["one"])],
            ..AppData::default()
        };
        data.composer_drafts
            .insert("session:a".into(), "draft".into());
        let mut full = encode_state(&data).unwrap();
        let object = full.as_object_mut().unwrap();
        for key in ["settings", "projects", "reviewGuides"] {
            assert!(object.remove(key).is_some());
        }
        assert_eq!(full, encode_checkpoint_index(&data).unwrap());
    }

    /// Timing of a streaming checkpoint while the last message of a long conversation
    /// grows (run by hand: `cargo test --lib persist::tests::bench_streaming -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn bench_streaming_checkpoint_in_a_long_conversation() {
        let body = "x".repeat(1500);
        let texts: Vec<String> = (0..3000).map(|n| format!("{n} {body}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        data.sessions = vec![session("big", &refs)];
        save(&path, &data).unwrap();
        let named = HashSet::from(["big".to_owned()]);
        let (mut locked, mut total) = (0.0, 0.0);
        for _ in 0..20 {
            data.sessions[0]
                .messages
                .rev_find_mut(|message| message.id == "big-2999")
                .unwrap()
                .content
                .push_str(" streamed");
            let started = std::time::Instant::now();
            let snapshot = snapshot_sessions(&path, &data, &named, next_generation()).unwrap();
            locked += started.elapsed().as_secs_f64();
            write(&path, &snapshot).unwrap();
            total += started.elapsed().as_secs_f64();
        }
        println!(
            "checkpoint of a ~4.5 MB conversation: {:.2} ms under the lock, {:.2} ms in all",
            locked * 1000.0 / 20.0,
            total * 1000.0 / 20.0
        );
    }

    #[test]
    fn astro_tables_and_conversations_from_older_versions_are_dropped_or_kept_as_sessions() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        data.sessions = vec![session("a", &["hi"])];
        save(&path, &data).unwrap();
        // A database written by an Astro-era version: its tables, and an Astro conversation.
        {
            let raw = crate::store::raw(&path);
            raw.execute_batch(
                "CREATE TABLE astros (id TEXT PRIMARY KEY, position INTEGER NOT NULL, json TEXT NOT NULL);
                 INSERT INTO astros VALUES ('x', 0, '{}');
                 DELETE FROM schema_migrations WHERE version = 2;
                 UPDATE sessions SET json = json_set(json, '$.astro', 'x', '$.launchedBy', json('{\"astroId\":\"x\",\"hidden\":true}'));
                 INSERT INTO app_state VALUES ('automationRuns', '[]');",
            )
            .unwrap();
        }
        restart(&path);
        let loaded = load_or_create(&path).unwrap();
        assert_eq!(
            loaded.sessions.len(),
            1,
            "the Astro conversation stays as a session"
        );
        let raw = crate::store::raw(&path);
        let tables: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'astros'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
        let leftovers: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM app_state WHERE key = 'automationRuns'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leftovers, 0);
    }

    fn project(id: &str, look: serde_json::Value) -> crate::models::Project {
        serde_json::from_value(serde_json::json!({"id":id,"name":id,"path":format!("/fixture/{id}"),"addedAt":"t","lastOpenedAt":"t","look":look})).unwrap()
    }

    #[test]
    fn project_astro_icons_come_back_from_the_json_backup_once() {
        let temp = crate::git::tests::Repo::new();
        let path = temp.0.join("state.json");
        let mut data = load_or_create(&path).unwrap();
        data.projects = vec![
            project("bare", serde_json::json!({})),
            project("tinted", serde_json::json!({"color":"green"})),
            project("emoji", serde_json::json!({"emoji":"💰"})),
            project("unknown", serde_json::json!({})),
            project("new", serde_json::json!({})),
        ];
        save(&path, &data).unwrap();
        let backup = temp.0.join(JSON_BACKUP_DIR);
        fs::create_dir_all(&backup).unwrap();
        fs::write(
            backup.join("state.json"),
            serde_json::json!({"projects":[
                {"id":"bare","look":{"astro":{"icon":"planeta","style":"metal"}}},
                {"id":"tinted","look":{"astro":{"icon":"buraco","style":"neon"},"color":"red"}},
                {"id":"emoji","look":{"astro":{"icon":"foguete","style":"metal"}}},
                {"id":"unknown","look":{"astro":{"icon":"../x","style":"metal"}}},
            ]})
            .to_string(),
        )
        .unwrap();
        // A database from the version that dropped them: migration 3 has not run yet.
        crate::store::raw(&path)
            .execute("DELETE FROM schema_migrations WHERE version = 3", [])
            .unwrap();
        restart(&path);
        let loaded = load_or_create(&path).unwrap();
        let look = |id: &str| {
            loaded
                .projects
                .iter()
                .find(|project| project.id == id)
                .unwrap()
                .look
                .clone()
        };
        let astro = |icon: &str, style: &str| crate::models::ProjectAstroIcon {
            icon: icon.into(),
            style: style.into(),
        };
        assert_eq!(look("bare").astro, Some(astro("planeta", "metal")));
        let tinted = look("tinted");
        assert_eq!(tinted.astro, Some(astro("buraco", "neon")));
        assert_eq!(
            tinted.color.as_deref(),
            Some("green"),
            "today's colour stays"
        );
        assert_eq!(look("emoji").astro, None, "a chosen emoji is kept");
        assert_eq!(look("emoji").emoji.as_deref(), Some("💰"));
        assert_eq!(look("unknown").astro, None);
        assert_eq!(look("new").astro, None);

        // It runs once: an icon removed afterwards does not come back.
        let mut data = loaded;
        data.projects[0].look.astro = None;
        save(&path, &data).unwrap();
        restart(&path);
        assert_eq!(load_or_create(&path).unwrap().projects[0].look.astro, None);
    }

    #[test]
    fn a_missing_or_broken_json_backup_restores_nothing() {
        for backup in [None, Some("not json"), Some("{\"projects\":{}}")] {
            let temp = crate::git::tests::Repo::new();
            let path = temp.0.join("state.json");
            let mut data = load_or_create(&path).unwrap();
            data.projects = vec![project("bare", serde_json::json!({}))];
            save(&path, &data).unwrap();
            if let Some(text) = backup {
                let dir = temp.0.join(JSON_BACKUP_DIR);
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("state.json"), text).unwrap();
            }
            crate::store::raw(&path)
                .execute("DELETE FROM schema_migrations WHERE version = 3", [])
                .unwrap();
            restart(&path);
            let loaded = load_or_create(&path).unwrap();
            assert_eq!(
                loaded.projects[0].look,
                crate::models::ProjectLook::default()
            );
        }
    }
}
