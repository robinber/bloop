//! `Microphone`: captures the input device with cpal and sends mono samples to the sink.
//!
//! The capture uses the default input, except when `device::preferred_input` names another one.

use std::fmt;
use std::num::{NonZeroU16, NonZeroU32};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bloop_core::domain::{CaptureSummary, Generation, LevelMeter};
use bloop_core::error::{AppError, Permission};
use bloop_core::ports::platform::{AudioInput, AudioSink, EventSink};
use bloop_core::session::Event;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, InputCallbackInfo, Stream};

use crate::device;

/// The number of frames that the data callback mixes and pushes at a time.
const SCRATCH_FRAMES: usize = 4096;

/// Captures the default input device, or the device that `device::preferred_input` names.
///
/// It operates on the main thread.
/// `Debug` shows the generation and the rate of the active capture, never its samples.
pub struct Microphone {
    meter: LevelMeter,
    events: Arc<dyn EventSink>,
    allowed: fn() -> bool,
    active: Option<Capture>,
}

struct Capture {
    generation: Generation,
    rate: NonZeroU32,
    stream: Stream,
    counters: Arc<Counters>,
}

/// The measures that the callbacks write and `stop` reads.
///
/// Each value is independent: no other data is published through it. The drop of the stream
/// waits for the last data callback, thus `stop` reads the final values with `Relaxed` loads.
#[derive(Default)]
struct Counters {
    frames: AtomicU64,
    peak: AtomicU32,
    overruns: AtomicU64,
}

impl fmt::Debug for Microphone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Microphone")
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for Capture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Capture")
            .field("generation", &self.generation)
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl Microphone {
    /// `allowed` reads the microphone permission before each capture.
    #[must_use]
    pub fn new(meter: LevelMeter, events: Arc<dyn EventSink>, allowed: fn() -> bool) -> Self {
        Self {
            meter,
            events,
            allowed,
            active: None,
        }
    }
}

impl AudioInput for Microphone {
    fn start(
        &mut self,
        generation: Generation,
        mut sink: Box<dyn AudioSink>,
    ) -> Result<NonZeroU32, AppError> {
        let begin = Instant::now();
        if self.active.is_some() {
            tracing::warn!("a capture is still active, it stops");
            self.cancel();
        }
        if !(self.allowed)() {
            return Err(AppError::PermissionMissing(Permission::Microphone));
        }
        let host = cpal::default_host();
        let device = match device::preferred_input() {
            Some(name) => input_named(&host, &name),
            None => host.default_input_device(),
        }
        .ok_or(AppError::Microphone)?;
        let config = device
            .default_input_config()
            .map_err(|error| unavailable(&error))?;
        let channels = NonZeroU16::new(config.channels()).ok_or(AppError::Microphone)?;
        let rate = NonZeroU32::new(config.sample_rate()).ok_or(AppError::Microphone)?;

        let counters = Arc::new(Counters::default());
        let meter = self.meter.clone();
        let data_counters = Arc::clone(&counters);
        let mut scratch = vec![0.0; SCRATCH_FRAMES];
        let data_callback = move |data: &[f32], _: &InputCallbackInfo| {
            for frames in data.chunks(usize::from(channels.get()) * SCRATCH_FRAMES) {
                let (mono, peak) = mix(frames, channels, &mut scratch);
                meter.set(peak);
                data_counters
                    .frames
                    .fetch_add(mono.len() as u64, Ordering::Relaxed);
                // The bits of floats from 0 to 1 have the same order as their values.
                data_counters
                    .peak
                    .fetch_max(peak.to_bits(), Ordering::Relaxed);
                if sink.try_push(mono).is_err() {
                    return;
                }
            }
        };
        let events = Arc::clone(&self.events);
        let error_counters = Arc::clone(&counters);
        let error_callback = move |error: cpal::Error| match error.kind() {
            ErrorKind::Xrun => {
                error_counters.overruns.fetch_add(1, Ordering::Relaxed);
            }
            _ => events.send(Event::CaptureFailed(generation, AppError::Microphone)),
        };
        let stream = device
            .build_input_stream(config.config(), data_callback, error_callback, None)
            .map_err(|error| unavailable(&error))?;
        stream.play().map_err(|error| unavailable(&error))?;

        tracing::info!(
            rate = rate.get(),
            channels = channels.get(),
            millis = begin.elapsed().as_millis(),
            "the microphone started"
        );
        self.active = Some(Capture {
            generation,
            rate,
            stream,
            counters,
        });
        Ok(rate)
    }

    fn stop(&mut self, generation: Generation) -> Result<CaptureSummary, AppError> {
        let Some(Capture {
            rate,
            stream,
            counters,
            ..
        }) = self
            .active
            .take_if(|capture| capture.generation == generation)
        else {
            if let Some(capture) = &self.active {
                tracing::error!(
                    started = ?capture.generation,
                    ?generation,
                    "the stop is for another generation, the active capture stops"
                );
                self.cancel();
            }
            return Err(AppError::Internal);
        };
        drop(stream);
        self.meter.set(0.0);
        let overruns = counters.overruns.load(Ordering::Relaxed);
        if overruns > 0 {
            tracing::warn!(overruns, "the microphone had overruns");
        }
        Ok(CaptureSummary {
            duration: duration(counters.frames.load(Ordering::Relaxed), rate),
            peak: f32::from_bits(counters.peak.load(Ordering::Relaxed)),
        })
    }

    fn cancel(&mut self) {
        if let Some(capture) = self.active.take() {
            drop(capture);
            self.meter.set(0.0);
        }
    }
}

/// Finds the input device of `host` with this name. Without it, the default input applies.
fn input_named(host: &cpal::Host, name: &str) -> Option<cpal::Device> {
    let found = match host.input_devices() {
        Ok(mut devices) => devices.find(|device| {
            device
                .description()
                .is_ok_and(|description| description.name() == name)
        }),
        Err(error) => {
            tracing::warn!(%error, "cpal did not list the input devices");
            None
        }
    };
    found.or_else(|| {
        tracing::warn!("the preferred input is not available, the capture uses the default input");
        host.default_input_device()
    })
}

/// Logs the reason of a failed start on the main thread.
fn unavailable(error: &cpal::Error) -> AppError {
    tracing::warn!(%error, "the microphone did not start");
    AppError::Microphone
}

/// Mixes the interleaved `frames` to the mean of their channels into `mono`.
///
/// Returns the mixed samples and their peak absolute value, from 0 to 1. A NaN sample is never the peak.
fn mix<'a>(frames: &[f32], channels: NonZeroU16, mono: &'a mut [f32]) -> (&'a [f32], f32) {
    let mut count = 0;
    let mut peak = 0.0_f32;
    for (frame, sample) in frames
        .chunks_exact(usize::from(channels.get()))
        .zip(mono.iter_mut())
    {
        *sample = frame.iter().sum::<f32>() / f32::from(channels.get());
        peak = peak.max(sample.abs());
        count += 1;
    }
    (&mono[..count], peak.min(1.0))
}

/// The duration of `frames` samples at `rate`, exact to the nanosecond below.
fn duration(frames: u64, rate: NonZeroU32) -> Duration {
    const NANOS_PER_SECOND: u64 = 1_000_000_000;
    let rate = u64::from(rate.get());
    // The remainder is below `u32::MAX`, thus its product with 10^9 stays below `u64::MAX`.
    Duration::from_secs(frames / rate)
        + Duration::from_nanos(frames % rate * NANOS_PER_SECOND / rate)
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU16, NonZeroU32};
    use std::sync::Arc;
    use std::time::Duration;

    use bloop_core::domain::LevelMeter;
    use bloop_core::ports::platform::EventSink;
    use bloop_core::session::Event;

    use super::{Microphone, duration, mix};

    struct NoEvents;

    impl EventSink for NoEvents {
        fn send(&self, _: Event) {}
    }

    #[test]
    fn the_mix_is_the_mean_of_the_channels_and_the_peak_ignores_nan() {
        let stereo = NonZeroU16::new(2).unwrap();
        let mut mono = [0.0; 4];

        let (samples, peak) = mix(&[0.25, 0.75, -0.5, -1.0], stereo, &mut mono);
        assert_eq!(samples, [0.5, -0.75]);
        assert_eq!(peak, 0.75);

        let (samples, peak) = mix(&[f32::NAN, 0.5, 0.25, 0.25], stereo, &mut mono);
        assert!(samples[0].is_nan());
        assert_eq!(samples[1], 0.25);
        assert_eq!(peak, 0.25);
    }

    #[test]
    fn the_duration_is_exact_for_any_sample_count() {
        let rate = |hertz| NonZeroU32::new(hertz).unwrap();

        assert_eq!(duration(96_000, rate(48_000)), Duration::from_secs(2));
        assert_eq!(duration(44_541, rate(44_100)), Duration::from_millis(1_010));
        assert_eq!(
            duration(u64::MAX, rate(48_000)),
            Duration::new(384_307_168_202_282, 325_312_500)
        );
    }

    #[test]
    fn the_debug_output_of_a_microphone_without_capture_shows_no_capture() {
        let microphone = Microphone::new(LevelMeter::default(), Arc::new(NoEvents), || true);

        assert_eq!(format!("{microphone:?}"), "Microphone { active: None, .. }");
    }
}
