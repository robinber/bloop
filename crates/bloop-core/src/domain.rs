//! Domain values shared by the state machine and the adapters.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

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

impl fmt::Debug for FinalText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FinalText")
            .field("chars", &self.0.chars().count())
            .finish()
    }
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
    use super::{FinalText, LevelMeter};

    #[test]
    fn debug_shows_the_length_but_not_the_text() {
        let text = FinalText::new("mot de passe secret".to_owned());

        let debug = format!("{text:?}");

        assert_eq!(debug, "FinalText { chars: 19 }");
        assert!(!debug.contains("secret"));
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
