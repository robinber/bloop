//! Domain types, session state machine, pipeline, and ports of Bloop.
//!
//! This crate has no I/O, no async runtime, and no platform API.

#![forbid(unsafe_code)]

pub mod dictionary;
pub mod domain;
pub mod error;
pub mod pipeline;
pub mod ports;
pub mod session;
