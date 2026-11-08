//! Freeze diagnostics: the main thread ticks a clock every half second, and a watcher thread
//! notes when it stops. A stall of 2 s or more is written to ~/Library/Logs/Sirus/freezes.log
//! with how long it lasted; past 4 s the watcher also has macOS `sample` the process for two
//! seconds once, so the log shows where the main thread was stuck. At most one sample every
//! ten minutes, and the log is trimmed past 2 MB.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::App;

const TICK: Duration = Duration::from_millis(500);
const STALL: Duration = Duration::from_secs(2);
const SAMPLE_AFTER: Duration = Duration::from_secs(4);
const SAMPLE_GAP: Duration = Duration::from_secs(600);
const LOG_LIMIT: u64 = 2 * 1024 * 1024;
/// Longer silences are the Mac sleeping, not a freeze.
const ASLEEP: Duration = Duration::from_secs(60);

static LAST_TICK: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn log_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(std::path::PathBuf::from(home).join("Library/Logs/Sirus"))
}

fn write(line: &str) {
    let Some(dir) = log_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("freezes.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = std::fs::rename(&path, dir.join("freezes.old.log"));
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{line}");
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// What the watcher should do after seeing the main thread silent for `silent`.
#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    Nothing,
    Sample,
}

pub(crate) fn step(silent: Duration, sampled_ago: Option<Duration>, sampled_now: bool) -> Step {
    if silent >= SAMPLE_AFTER
        && silent < ASLEEP
        && !sampled_now
        && sampled_ago.is_none_or(|ago| ago >= SAMPLE_GAP)
    {
        Step::Sample
    } else {
        Step::Nothing
    }
}

pub(crate) fn start(cx: &mut App) {
    if std::env::var_os("SIRUS_NO_FREEZE_WATCH").is_some() {
        return;
    }
    LAST_TICK.store(now_ms(), Ordering::Relaxed);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(TICK).await;
            LAST_TICK.store(now_ms(), Ordering::Relaxed);
        }
    })
    .detach();
    let _ = std::thread::Builder::new()
        .name("freeze-watch".into())
        .spawn(|| {
            let pid = std::process::id();
            let mut stalled_since: Option<u64> = None;
            let mut sampled_at: Option<Instant> = None;
            let mut sampled_now = false;
            loop {
                std::thread::sleep(TICK);
                let last = LAST_TICK.load(Ordering::Relaxed);
                let silent = Duration::from_millis(now_ms().saturating_sub(last));
                if silent >= STALL {
                    stalled_since.get_or_insert(last);
                    if step(silent, sampled_at.map(|at| at.elapsed()), sampled_now) == Step::Sample
                    {
                        sampled_now = true;
                        sampled_at = Some(Instant::now());
                        let file =
                            log_dir().map(|d| d.join(format!("freeze-{}.sample.txt", now_ms())));
                        if let Some(file) = file {
                            let _ = std::process::Command::new("/usr/bin/sample")
                                .args([&pid.to_string(), "2", "-mayDie", "-file"])
                                .arg(&file)
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status();
                            write(&format!(
                                "{} sample of the stuck main thread: {}",
                                stamp(),
                                file.display()
                            ));
                        }
                    }
                } else if let Some(since) = stalled_since.take() {
                    let lasted = last.saturating_sub(since);
                    sampled_now = false;
                    if lasted >= ASLEEP.as_millis() as u64 {
                        continue;
                    }
                    write(&format!(
                        "{} main thread stalled for {:.1} s",
                        stamp(),
                        lasted as f64 / 1000.
                    ));
                    sampled_now = false;
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_once_per_long_stall_and_not_too_often() {
        let s = Duration::from_secs;
        assert_eq!(step(s(3), None, false), Step::Nothing);
        assert_eq!(step(s(5), None, false), Step::Sample);
        assert_eq!(step(s(9), None, true), Step::Nothing);
        assert_eq!(step(s(5), Some(s(60)), false), Step::Nothing);
        assert_eq!(step(s(5), Some(s(601)), false), Step::Sample);
        assert_eq!(step(s(120), None, false), Step::Nothing);
    }
}
