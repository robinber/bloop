//! macOS integration of Bloop: `AppKit`, event tap, orb, paste, menu, and Keychain.
//!
//! This is the only crate where `unsafe` code is approved, as `AGENTS.md` describes.

pub mod app;
pub mod menu;

pub use objc2::MainThreadMarker;
