//! macOS integration of Bloop: `AppKit`, event tap, orb, paste, menu, and Keychain.
//!
//! This is the only crate where `unsafe` code is approved, as `AGENTS.md` describes.

pub mod app;
pub mod events;
pub mod keys;
pub mod menu;
pub mod orb;
pub mod permissions;
pub mod scheduler;

pub use objc2::MainThreadMarker;
