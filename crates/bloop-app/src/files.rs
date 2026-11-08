//! The file rules of the settings folder: a bounded read, the write of a missing file, and an atomic replace.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Why Bloop cannot use the text of a file. It contains no file content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileError {
    /// The file has more bytes than its limit.
    TooLarge,
    /// The file exists, but Bloop cannot read it as UTF-8 text. Text that is not UTF-8 gives `InvalidData`.
    Unreadable(io::ErrorKind),
}

/// Reads the UTF-8 text of the file at `path`, with at most `limit` bytes.
/// Returns `None` when the file does not exist.
///
/// It reads at most one byte more than `limit`. Thus a large file never fills the memory.
///
/// # Errors
///
/// `TooLarge` when the file has more than `limit` bytes, and `Unreadable` for the other failures.
pub(crate) fn read_bounded(path: &Path, limit: u64) -> Result<Option<String>, FileError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(FileError::Unreadable(error.kind())),
    };
    let mut reader = file.take(limit.saturating_add(1));
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| FileError::Unreadable(error.kind()))?;
    if reader.limit() == 0 {
        return Err(FileError::TooLarge);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| FileError::Unreadable(io::ErrorKind::InvalidData))
}

/// Writes `text` to a new file at `path`, and creates its folder when necessary.
/// Returns `false`, without a change, when the file exists.
///
/// The exclusive creation is the existence check. Thus no other write can occur between a check and the write.
/// If the write fails after the creation, it removes the partial file, so that the next start does not read it.
pub(crate) fn write_if_missing(path: &Path, text: &str) -> io::Result<bool> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(error),
    };
    if let Err(error) = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        if let Err(cleanup) = fs::remove_file(path) {
            tracing::warn!(kind = ?cleanup.kind(), "the partial file is not removed");
        }
        return Err(error);
    }
    Ok(true)
}

/// Writes `text` to a temporary file in the folder of `path`, then renames it to `path`.
/// Each write owns its temporary file. A failed write or rename attempts to remove that file.
pub(crate) fn replace(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let (temporary, mut file) = create_temporary(path)?;
    let written = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| fs::rename(&temporary, path));
    if result.is_err()
        && let Err(cleanup) = fs::remove_file(&temporary)
    {
        tracing::warn!(kind = ?cleanup.kind(), "the temporary file is not removed");
    }
    result
}

/// Creates a private file next to `path`, without opening or truncating an existing file.
/// A reused process identifier can meet a file left by an interrupted write, so collisions get a new name.
fn create_temporary(path: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..16 {
        // Only uniqueness matters. The counter publishes no other state.
        let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
        let temporary = path.with_added_extension(format!("tmp.{}.{sequence}", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused temporary file name",
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::fs;
    use std::io::{self, Write as _};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{FileError, create_temporary, read_bounded, replace, write_if_missing};

    /// Creates an empty folder for one test of the settings files. The test removes it at the end.
    pub(crate) fn scratch_dir(test: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("bloop-test-{test}-{}-{nanos}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_file_gives_no_text() {
        let dir = scratch_dir("missing");

        assert_eq!(read_bounded(&dir.join("config.toml"), 8), Ok(None));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_at_the_limit_is_read_and_one_byte_more_is_too_large() {
        let dir = scratch_dir("limit");
        let path = dir.join("config.toml");

        fs::write(&path, "é234567").unwrap();
        assert_eq!(read_bounded(&path, 8), Ok(Some("é234567".to_owned())));
        fs::write(&path, "é2345678").unwrap();
        assert_eq!(read_bounded(&path, 8), Err(FileError::TooLarge));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_over_the_limit_is_too_large_also_when_the_limit_cuts_a_character() {
        let dir = scratch_dir("cut");
        let path = dir.join("config.toml");
        fs::write(&path, "1234567é").unwrap();

        assert_eq!(read_bounded(&path, 7), Err(FileError::TooLarge));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_is_not_utf_8_is_unreadable() {
        let dir = scratch_dir("utf8");
        let path = dir.join("config.toml");
        fs::write(&path, b"schema = \xff\n").unwrap();

        assert_eq!(
            read_bounded(&path, 64),
            Err(FileError::Unreadable(io::ErrorKind::InvalidData))
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_if_missing_writes_one_time_and_keeps_an_existing_file() {
        let dir = scratch_dir("missing-write");
        let path = dir.join("Bloop").join("config.toml");

        assert!(write_if_missing(&path, "schema = 1\n").unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "schema = 1\n");
        fs::write(&path, b"# mine\r\nschema = 1").unwrap();
        assert!(!write_if_missing(&path, "schema = 1\n").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"# mine\r\nschema = 1");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_creates_the_folder_and_replaces_the_file_without_a_temporary_file() {
        let dir = scratch_dir("replace");
        let path = dir.join("Bloop").join("config.toml");

        replace(&path, "schema = 1\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "schema = 1\n");
        replace(&path, "schema = 1\n\n[refine]\nenabled = true\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "schema = 1\n\n[refine]\nenabled = true\n"
        );
        assert!(!path.with_added_extension("tmp").exists());
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn replace_does_not_touch_an_existing_shared_temporary_file() {
        let dir = scratch_dir("replace-existing-temporary");
        let path = dir.join("config.toml");
        let existing = path.with_added_extension("tmp");
        fs::write(&existing, "another write").unwrap();

        replace(&path, "schema = 1\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "schema = 1\n");
        assert_eq!(fs::read_to_string(&existing).unwrap(), "another write");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlapping_writers_cannot_change_a_file_that_another_writer_published() {
        let dir = scratch_dir("replace-overlap");
        let path = dir.join("config.toml");
        let (first_path, mut first) = create_temporary(&path).unwrap();
        let (second_path, mut second) = create_temporary(&path).unwrap();

        first.write_all(b"first").unwrap();
        drop(first);
        fs::rename(&first_path, &path).unwrap();
        second.write_all(b"second").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"first");
        drop(second);
        fs::rename(&second_path, &path).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_rename_removes_its_temporary_file() {
        let dir = scratch_dir("replace-failed-rename");
        let path = dir.join("config.toml");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), "existing content").unwrap();

        assert!(replace(&path, "schema = 1\n").is_err());

        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(
            fs::read_to_string(path.join("keep")).unwrap(),
            "existing content"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
