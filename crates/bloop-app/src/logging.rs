//! Log output to `~/Library/Logs/Bloop/bloop.log`.

use std::fs::{self, OpenOptions};
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};

/// Starts the logger. Bloop adds to the file and keeps the previous lines.
pub(crate) fn init() -> Result<()> {
    let home = std::env::home_dir().context("find the home directory")?;
    let dir = home.join("Library/Logs/Bloop");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join("bloop.log");
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
        .context("start the logger")
}
