//! `TestLevel`: a synthetic voice level for the orb until Phase 2 adds the microphone.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use bloop_core::domain::LevelMeter;

const PERIOD: Duration = Duration::from_millis(20);

/// Writes a synthetic level into the meter during a capture.
///
/// Each capture has its own writer thread. After `stop`, the thread writes silence and ends within one period.
pub(crate) struct TestLevel {
    meter: LevelMeter,
    running: Option<Arc<AtomicBool>>,
}

impl TestLevel {
    pub(crate) fn new(meter: LevelMeter) -> Self {
        Self {
            meter,
            running: None,
        }
    }

    /// Starts a writer. It stops the previous writer first.
    pub(crate) fn start(&mut self) {
        self.stop();
        let running = Arc::new(AtomicBool::new(true));
        let writer = Arc::clone(&running);
        let meter = self.meter.clone();
        let spawned = thread::Builder::new()
            .name("test-level".to_owned())
            .spawn(move || write_level(&meter, &writer));
        match spawned {
            Ok(_) => self.running = Some(running),
            Err(error) => tracing::error!(%error, "the test level did not start"),
        }
    }

    pub(crate) fn stop(&mut self) {
        if let Some(running) = self.running.take() {
            // The flag is one independent value. No other data is published through it.
            running.store(false, Ordering::Relaxed);
        }
    }
}

impl Drop for TestLevel {
    fn drop(&mut self) {
        self.stop();
    }
}

fn write_level(meter: &LevelMeter, running: &AtomicBool) {
    let start = Instant::now();
    while running.load(Ordering::Relaxed) {
        meter.set(level_at(start.elapsed()));
        thread::sleep(PERIOD);
    }
    meter.set(0.0);
}

/// About five syllables each second, in phrases of 2.5 seconds with a short silence between them.
fn level_at(elapsed: Duration) -> f32 {
    let time = elapsed.as_secs_f32();
    let syllables = (TAU * 2.5 * time).sin().abs();
    let phrase = 0.5 + 0.5 * (TAU * 0.4 * time).sin();
    0.005 + 0.25 * syllables * phrase
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use bloop_core::domain::LevelMeter;

    use super::{TestLevel, level_at};

    fn wait_until(condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "the condition stayed false");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_level_stays_in_the_meter_range() {
        for millis in (0..5_000).step_by(10) {
            let level = level_at(Duration::from_millis(millis));
            assert!((0.0..=1.0).contains(&level), "{millis} ms: {level}");
        }
    }

    #[test]
    fn stop_returns_the_meter_to_silence() {
        let meter = LevelMeter::default();
        let mut test_level = TestLevel::new(meter.clone());

        test_level.start();
        wait_until(|| meter.get() > 0.0);
        test_level.stop();

        wait_until(|| meter.get() <= 0.0);
    }

    #[test]
    fn drop_stops_the_writer() {
        let meter = LevelMeter::default();
        let mut test_level = TestLevel::new(meter.clone());
        test_level.start();
        wait_until(|| meter.get() > 0.0);

        drop(test_level);

        wait_until(|| meter.get() <= 0.0);
    }
}
