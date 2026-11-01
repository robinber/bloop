//! Domain values shared by the state machine and the adapters.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Identifies one session. The state machine ignores events for an unknown generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation(pub(crate) u64);

/// The text that Bloop pastes: the corrected text or the raw transcript.
///
/// `Debug` shows only the number of characters, never the text.
#[derive(Clone, PartialEq, Eq)]
pub struct FinalText(String);

impl FinalText {
    /// Wraps the text that a job produced.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<Transcript> for FinalText {
    fn from(transcript: Transcript) -> Self {
        Self(transcript.0)
    }
}

impl fmt::Debug for FinalText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        redacted_text(f, "FinalText", &self.0)
    }
}

/// The complete text that the speech provider returns at the end of a session.
///
/// `Debug` shows only the number of characters, never the text.
pub struct Transcript(String);

impl Transcript {
    /// Wraps the complete text of a session.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self(text)
    }

    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Transcript {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        redacted_text(f, "Transcript", &self.0)
    }
}

fn redacted_text(f: &mut fmt::Formatter<'_>, name: &str, text: &str) -> fmt::Result {
    f.debug_struct(name)
        .field("chars", &text.chars().count())
        .finish()
}

const BYTES_PER_SAMPLE: usize = 2;

/// Audio for the speech provider: mono signed 16-bit little-endian PCM at [`Self::SAMPLE_RATE`],
/// without a file header.
///
/// `Debug` shows only the number of bytes, never the audio.
pub struct AudioChunk(Vec<u8>);

impl AudioChunk {
    /// The sample rate of the audio, in Hz.
    pub const SAMPLE_RATE: u32 = 16_000;

    /// The largest chunk: 100 ms of audio.
    pub const MAX_BYTES: usize = 1_600 * BYTES_PER_SAMPLE;

    /// Wraps PCM bytes. Returns `None` if `pcm` is empty, ends with a partial sample,
    /// or is larger than [`Self::MAX_BYTES`].
    #[must_use]
    pub fn new(pcm: Vec<u8>) -> Option<Self> {
        let valid = !pcm.is_empty()
            && pcm.len().is_multiple_of(BYTES_PER_SAMPLE)
            && pcm.len() <= Self::MAX_BYTES;
        valid.then_some(Self(pcm))
    }

    /// Returns the PCM bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for AudioChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioChunk")
            .field("bytes", &self.0.len())
            .finish()
    }
}

/// The measures of a finished capture. It contains no audio.
#[derive(Debug, Clone, Copy)]
pub struct CaptureSummary {
    /// The duration of the captured audio.
    pub duration: Duration,
    /// The highest level of the capture, from 0 to 1.
    pub peak: f32,
}

/// The application that receives the text. The text steps can adapt the text to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetInfo {
    /// The bundle identifier, for example `com.apple.TextEdit`.
    pub bundle_id: Option<String>,
    /// The name of the application that macOS shows.
    pub app_name: Option<String>,
}

/// The current audio level, from 0 (silence) to 1. The audio thread writes it, and the orb reads it.
///
/// The clones share the same value.
#[derive(Debug, Clone, Default)]
pub struct LevelMeter(Arc<AtomicU32>);

impl LevelMeter {
    /// Stores the level. A value outside 0 to 1 is clamped, and NaN stores 0.
    pub fn set(&self, level: f32) {
        let level = if level.is_nan() {
            0.0
        } else {
            level.clamp(0.0, 1.0)
        };
        // The level is one independent value. No other data is published through it.
        self.0.store(level.to_bits(), Ordering::Relaxed);
    }

    /// Returns the last stored level, or 0 if no level was stored.
    #[must_use]
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioChunk, FinalText, LevelMeter, Transcript};

    #[test]
    fn debug_shows_the_length_but_not_the_text() {
        let text = FinalText::new("mot de passe secret".to_owned());

        let debug = format!("{text:?}");

        assert_eq!(debug, "FinalText { chars: 19 }");
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn transcript_and_audio_debug_show_only_their_size() {
        let transcript = Transcript::new("mot de passe".to_owned());
        let chunk = AudioChunk::new(vec![7; 4]);

        assert_eq!(format!("{transcript:?}"), "Transcript { chars: 12 }");
        assert_eq!(format!("{chunk:?}"), "Some(AudioChunk { bytes: 4 })");
    }

    #[test]
    fn a_chunk_holds_whole_samples_up_to_the_limit() {
        for len in [0, 3, AudioChunk::MAX_BYTES + 2] {
            assert!(AudioChunk::new(vec![0; len]).is_none(), "{len} bytes");
        }
        let chunk = AudioChunk::new(vec![0; AudioChunk::MAX_BYTES]);

        assert_eq!(
            chunk.map(|chunk| chunk.as_bytes().len()),
            Some(AudioChunk::MAX_BYTES)
        );
    }

    #[test]
    fn a_new_meter_reads_silence() {
        assert_eq!(LevelMeter::default().get().to_bits(), 0.0_f32.to_bits());
    }

    #[test]
    fn clones_share_the_level() {
        let meter = LevelMeter::default();
        let reader = meter.clone();

        meter.set(0.25);

        assert_eq!(reader.get().to_bits(), 0.25_f32.to_bits());
    }

    #[test]
    fn levels_outside_the_range_are_clamped() {
        let meter = LevelMeter::default();

        for (level, stored) in [
            (-0.5, 0.0),
            (1.5, 1.0),
            (f32::INFINITY, 1.0),
            (f32::NAN, 0.0),
        ] {
            meter.set(level);
            assert_eq!(meter.get().to_bits(), f32::to_bits(stored), "level {level}");
        }
    }
}
