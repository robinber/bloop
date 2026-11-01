//! Log output to `~/Library/Logs/Bloop/bloop.log`.
//!
//! At start, a log file larger than [`LOG_LIMIT`] moves to `bloop.log.1` and replaces the older one.
//! Thus Bloop keeps the current file and one previous file.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};

/// Size in bytes above which the log file moves to `bloop.log.1` at start.
/// 5 MiB holds tens of thousands of event lines for diagnosis and keeps the disk use small.
const LOG_LIMIT: u64 = 5 * 1024 * 1024;

/// Starts the logger.
///
/// If the rotation fails, Bloop adds to the current file and logs a warning.
pub(crate) fn init() -> Result<()> {
    let home = std::env::home_dir().context("find the home directory")?;
    let dir = home.join("Library/Logs/Bloop");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join("bloop.log");
    let rotation = rotate(&path, LOG_LIMIT);
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    tracing_subscriber::fmt()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .try_init()
        .map_err(|error| anyhow!(error))
        .context("start the logger")?;
    if let Err(error) = rotation {
        tracing::warn!(error = %format!("{error:#}"), "the log file did not rotate");
    }
    Ok(())
}

/// Moves `path` to `path` plus `.1` when the file is larger than `limit` bytes.
/// The move replaces an older `.1` file. A missing file is not an error.
fn rotate(path: &Path, limit: u64) -> Result<()> {
    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("read the metadata of {}", path.display()));
        }
    };
    if size <= limit {
        return Ok(());
    }
    let previous = path.with_added_extension("1");
    fs::rename(path, &previous)
        .with_context(|| format!("rename {} to {}", path.display(), previous.display()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const LIMIT: u64 = 8;

    fn scratch_dir(test: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "bloop-logging-{test}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn keeps_a_file_at_or_below_the_limit() {
        let dir = scratch_dir("below");
        let log = dir.join("bloop.log");
        let previous = dir.join("bloop.log.1");

        fs::write(&log, b"1234567").unwrap();
        rotate(&log, LIMIT).unwrap();
        assert_eq!(fs::read(&log).unwrap(), b"1234567");
        assert!(!previous.exists());

        fs::write(&log, b"12345678").unwrap();
        rotate(&log, LIMIT).unwrap();
        assert_eq!(fs::read(&log).unwrap(), b"12345678");
        assert!(!previous.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn moves_a_file_above_the_limit() {
        let dir = scratch_dir("above");
        let log = dir.join("bloop.log");
        let previous = dir.join("bloop.log.1");
        fs::write(&log, b"123456789").unwrap();

        rotate(&log, LIMIT).unwrap();

        assert!(!log.exists());
        assert_eq!(fs::read(&previous).unwrap(), b"123456789");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replaces_the_older_previous_file() {
        let dir = scratch_dir("replace");
        let log = dir.join("bloop.log");
        let previous = dir.join("bloop.log.1");
        fs::write(&previous, b"old").unwrap();
        fs::write(&log, b"123456789").unwrap();

        rotate(&log, LIMIT).unwrap();

        assert!(!log.exists());
        assert_eq!(fs::read(&previous).unwrap(), b"123456789");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ignores_a_missing_file() {
        let dir = scratch_dir("missing");
        let log = dir.join("bloop.log");

        rotate(&log, LIMIT).unwrap();

        assert!(!log.exists());
        assert!(!dir.join("bloop.log.1").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reports_a_failed_rename_and_keeps_the_file() {
        let dir = scratch_dir("rename");
        let log = dir.join("bloop.log");
        let previous = dir.join("bloop.log.1");
        fs::create_dir(&previous).unwrap();
        fs::write(previous.join("blocker"), b"x").unwrap();
        fs::write(&log, b"123456789").unwrap();

        let error = rotate(&log, LIMIT).unwrap_err();

        assert!(format!("{error:#}").starts_with("rename "));
        assert_eq!(fs::read(&log).unwrap(), b"123456789");
        fs::remove_dir_all(&dir).unwrap();
    }
}
