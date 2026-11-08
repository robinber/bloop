//! Log output to `~/Library/Logs/Bloop/bloop.log`.
//!
//! At start, a log file larger than [`LOG_LIMIT`] moves to `bloop.log.1` and replaces the older one.
//! Thus Bloop keeps the current file and one previous file.
//! The log also records each panic with its thread name and location.
//! It records the panic message only when the message is a literal.

use std::any::Any;
use std::fs::{self, OpenOptions};
use std::io;
use std::panic::{self, PanicHookInfo};
use std::path::Path;
use std::sync::Mutex;
use std::thread;

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
    install_panic_hook();
    if let Err(error) = rotation {
        tracing::warn!(error = %format!("{error:#}"), "the log file did not rotate");
    }
    Ok(())
}

/// Adds a panic hook that logs the panic and then calls the previous hook.
/// The previous hook keeps the message on stderr when a terminal exists.
fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
        let current = thread::current();
        let (payload, payload_bytes) = payload_text(info.payload());
        tracing::error!(
            thread = current.name().unwrap_or("unnamed"),
            location = info.location().map(tracing::field::display),
            payload,
            payload_bytes,
            "a thread panicked"
        );
        previous(info);
    }));
}

const FORMATTED_PAYLOAD: &str = "formatted message, not logged";

/// Returns the text to log for a panic payload and, for a formatted message, its length in bytes.
///
/// A `&'static str` payload is a literal: compile-time text that contains no dictated text.
/// A `String` payload is a formatted message, and its arguments can contain dictated text.
/// For example, the message of a bad `str` slice quotes a character of the string.
/// Thus a `String` gives only a fixed phrase and its length. Other types give a fixed phrase.
fn payload_text(payload: &(dyn Any + Send)) -> (&'static str, Option<usize>) {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        (text, None)
    } else if let Some(text) = payload.downcast_ref::<String>() {
        (FORMATTED_PAYLOAD, Some(text.len()))
    } else {
        ("payload is not text", None)
    }
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
    use super::*;
    use crate::files::tests::scratch_dir;

    const LIMIT: u64 = 8;

    #[test]
    fn keeps_a_file_at_or_below_the_limit() {
        let dir = scratch_dir("logging-below");
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
        let dir = scratch_dir("logging-above");
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
        let dir = scratch_dir("logging-replace");
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
        let dir = scratch_dir("logging-missing");
        let log = dir.join("bloop.log");

        rotate(&log, LIMIT).unwrap();

        assert!(!log.exists());
        assert!(!dir.join("bloop.log.1").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reports_a_failed_rename_and_keeps_the_file() {
        let dir = scratch_dir("logging-rename");
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

    #[test]
    fn logs_a_literal_payload_and_only_the_length_of_a_formatted_one() {
        let literal: Box<dyn Any + Send> = Box::new("literal text");
        let owned: Box<dyn Any + Send> = Box::new(String::from("dictée privée"));
        let other: Box<dyn Any + Send> = Box::new(42_u32);

        assert_eq!(payload_text(&*literal), ("literal text", None));
        let (text, bytes) = payload_text(&*owned);
        assert!(!text.contains("dictée"));
        assert_eq!(text, FORMATTED_PAYLOAD);
        assert_eq!(bytes, Some("dictée privée".len()));
        assert_eq!(payload_text(&*other), ("payload is not text", None));
    }

    #[test]
    fn logs_only_the_length_of_the_message_of_a_bad_slice_panic() {
        let dictated = String::from("é secret");
        let end = 1;

        let payload = panic::catch_unwind(|| dictated[..end].len()).unwrap_err();

        let message = payload.downcast_ref::<String>().unwrap();
        assert_eq!(
            payload_text(&*payload),
            (FORMATTED_PAYLOAD, Some(message.len()))
        );
    }
}
