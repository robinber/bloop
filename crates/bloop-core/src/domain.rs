//! Domain values shared by the state machine and the adapters.

use std::fmt;
use std::num::{NonZeroU32, NonZeroUsize};
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

/// Audio for the speech provider: mono signed 16-bit little-endian PCM at the sample rate of the session,
/// without a file header. A chunk is never empty.
///
/// `Debug` shows only the number of bytes, never the audio.
pub struct AudioChunk(Vec<u8>);

impl AudioChunk {
    /// Returns the number of samples in one chunk at `rate`, in Hz: 100 ms of audio.
    /// Below 10 Hz, a chunk holds one sample.
    #[must_use]
    pub const fn length(rate: NonZeroU32) -> NonZeroUsize {
        match NonZeroUsize::new((rate.get() / 10) as usize) {
            Some(length) => length,
            None => NonZeroUsize::MIN,
        }
    }

    /// Encodes samples at `rate`, in Hz, as consecutive chunks of [`length`](Self::length) samples, in order.
    /// The last chunk can be shorter.
    ///
    /// A sample from -1 to 1 uses the full 16-bit range. Values outside it saturate, and NaN encodes as 0.
    pub fn split(samples: &[f32], rate: NonZeroU32) -> impl Iterator<Item = Self> {
        samples.chunks(Self::length(rate).get()).map(|part| {
            let mut pcm = Vec::with_capacity(part.len() * size_of::<i16>());
            for &sample in part {
                pcm.extend_from_slice(&pcm16(sample).to_le_bytes());
            }
            Self(pcm)
        })
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

#[expect(
    clippy::cast_possible_truncation,
    reason = "a float to integer cast saturates, and NaN gives 0"
)]
fn pcm16(sample: f32) -> i16 {
    (sample * f32::from(i16::MAX)).round() as i16
}

/// The measures of a finished capture. It contains no audio.
#[derive(Debug, Clone, Copy)]
pub struct CaptureSummary {
    /// The duration of the captured audio.
    pub duration: Duration,
    /// The highest level of the capture, from 0 to 1.
    pub peak: f32,
}

/// The application that had the focus at key release, as the platform read it.
/// It is for the log and for diagnostics.
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
    use std::num::NonZeroU32;

    use super::{AudioChunk, FinalText, LevelMeter, Transcript};

    const RATE: NonZeroU32 = NonZeroU32::new(16_000).unwrap();

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
        let chunk = AudioChunk::split(&[0.5, -0.5], RATE).next();

        assert_eq!(format!("{transcript:?}"), "Transcript { chars: 12 }");
        assert_eq!(format!("{chunk:?}"), "Some(AudioChunk { bytes: 4 })");
    }

    #[test]
    fn samples_encode_as_little_endian_pcm16_and_saturate() {
        let samples = [0.0, 0.5, 1.0, -1.0, 2.0, -2.0, f32::NAN];

        let pcm: Vec<u8> = AudioChunk::split(&samples, RATE)
            .flat_map(|chunk| chunk.as_bytes().to_vec())
            .collect();

        assert_eq!(
            pcm,
            [
                0, 0, 0x00, 0x40, 0xFF, 0x7F, 0x01, 0x80, 0xFF, 0x7F, 0x00, 0x80, 0, 0
            ]
        );
    }

    #[test]
    fn split_keeps_the_order_and_holds_100_ms_at_the_rate_in_a_chunk() {
        for (rate, length) in [(48_000, 4_800), (24_000, 2_400), (9, 1)] {
            let rate = NonZeroU32::new(rate).unwrap();
            let mut samples = vec![0.0; length * 2];
            samples.push(1.0);

            let chunks: Vec<Vec<u8>> = AudioChunk::split(&samples, rate)
                .map(|chunk| chunk.as_bytes().to_vec())
                .collect();

            assert_eq!(AudioChunk::length(rate).get(), length, "{rate} Hz");
            assert_eq!(chunks.len(), 3, "{rate} Hz");
            assert!(
                chunks[..2].iter().all(|pcm| pcm.len() == length * 2),
                "{rate} Hz"
            );
            assert_eq!(chunks[2], [0xFF, 0x7F], "{rate} Hz");
        }
        assert!(AudioChunk::split(&[], NonZeroU32::MIN).next().is_none());
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
