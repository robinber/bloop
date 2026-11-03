//! AI provider adapters of Bloop, with one module for each vendor.

#![forbid(unsafe_code)]

#[cfg(feature = "mistral")]
pub mod mistral;
