//! Microphone capture and incremental PCM conversion of Bloop.

#![forbid(unsafe_code)]

pub mod buffer;
pub mod capture;
pub mod convert;
mod device;
