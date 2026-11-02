//! macOS integration of Bloop: `AppKit`, event tap, orb, paste, menu, and Keychain.
//!
//! This is the only crate where `unsafe` code is approved, as `AGENTS.md` describes.

pub mod app;
pub mod events;
pub mod keychain;
pub mod keys;
pub mod menu;
pub mod orb;
pub mod paste;
pub mod permissions;
pub mod scheduler;
pub mod sleep;

pub use objc2::MainThreadMarker;
