//! Incremental conversion of the capture to the sample rate of the speech provider.

use std::fmt;
use std::num::NonZeroU32;

use rubato::audioadapter::{Adapter, AdapterMut};
use rubato::audioadapter_buffers::owned::InterleavedOwned;
use rubato::{Fft, FixedSync, Indexing, ResampleError, Resampler, ResamplerConstructionError};

/// The sample rate of the converted audio, in Hz: the rate that Bloop sends to the speech provider.
pub const RATE: NonZeroU32 = NonZeroU32::new(16_000).unwrap();

/// The resampler processes blocks of 20 ms.
const BLOCKS_PER_SECOND: u32 = 50;

/// A failure of the resampler. It shows a bug in the converter, not a fault in the audio.
#[derive(Debug)]
pub enum ConvertError {
    /// The resampler refused the sample rates or the block length.
    Setup(ResamplerConstructionError),
    /// The resampler refused a buffer.
    Process(ResampleError),
}

impl From<ResamplerConstructionError> for ConvertError {
    fn from(error: ResamplerConstructionError) -> Self {
        Self::Setup(error)
    }
}

impl From<ResampleError> for ConvertError {
    fn from(error: ResampleError) -> Self {
        Self::Process(error)
    }
}

/// Converts the mono samples of one capture from the rate of the microphone to [`RATE`].
///
/// Over the whole capture, the output is the input resampled to [`RATE`], in time with the input.
/// After [`flush`](Self::flush), the capture gave `ceil(n × RATE / rate)` output samples
/// for `n` input samples at `rate`.
/// The output does not depend on how the calls to [`push`](Self::push) divide the input.
/// `Debug` shows the input and output rates, never the samples.
pub struct Converter(Kind);

enum Kind {
    /// The input is already at [`RATE`]. Thus the exact conversion is a copy, without delay.
    Copy,
    /// The FFT resampler. It filters the input before it lowers the rate, which prevents aliasing.
    Resample(Box<Resample>),
}

struct Resample {
    fft: Fft<f32>,
    rate: NonZeroU32,
    /// One input block, filled up to `filled`.
    input: InterleavedOwned<f32>,
    filled: usize,
    /// One output block.
    output: InterleavedOwned<f32>,
    /// The output samples of the initial resampler delay that the converter has yet to remove.
    skip: usize,
    received: usize,
    emitted: usize,
}

impl fmt::Debug for Converter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rate = match &self.0 {
            Kind::Copy => RATE,
            Kind::Resample(resample) => resample.rate,
        };
        f.debug_struct("Converter")
            .field("input_rate", &rate)
            .field("output_rate", &RATE)
            .finish_non_exhaustive()
    }
}

impl Converter {
    /// Creates the converter for mono samples at `rate`, in Hz.
    ///
    /// # Errors
    ///
    /// `Setup` when the resampler refuses the rates.
    pub fn new(rate: NonZeroU32) -> Result<Self, ConvertError> {
        if rate == RATE {
            return Ok(Self(Kind::Copy));
        }
        let block = (rate.get() / BLOCKS_PER_SECOND).max(1) as usize;
        let fft = Fft::new(
            rate.get() as usize,
            RATE.get() as usize,
            block,
            1,
            FixedSync::Both,
        )?;
        Ok(Self(Kind::Resample(Box::new(Resample {
            rate,
            input: InterleavedOwned::new(0.0, 1, fft.input_frames_next()),
            filled: 0,
            output: InterleavedOwned::new(0.0, 1, fft.output_frames_next()),
            skip: fft.output_delay(),
            received: 0,
            emitted: 0,
            fft,
        }))))
    }

    /// Converts `input` and appends the samples at [`RATE`] that are ready to `output`.
    /// The converter keeps the input of an incomplete block for the next call.
    ///
    /// # Errors
    ///
    /// `Process` when the resampler refuses a buffer.
    pub fn push(&mut self, input: &[f32], output: &mut Vec<f32>) -> Result<(), ConvertError> {
        match &mut self.0 {
            Kind::Copy => {
                output.extend_from_slice(input);
                Ok(())
            }
            Kind::Resample(resample) => resample.push(input, output),
        }
    }

    /// Ends the capture: appends the remaining samples at [`RATE`] to `output`.
    ///
    /// # Errors
    ///
    /// `Process` when the resampler refuses a buffer.
    pub fn flush(self, output: &mut Vec<f32>) -> Result<(), ConvertError> {
        match self.0 {
            Kind::Copy => Ok(()),
            Kind::Resample(mut resample) => resample.flush(output),
        }
    }
}

impl Resample {
    fn push(&mut self, mut input: &[f32], output: &mut Vec<f32>) -> Result<(), ConvertError> {
        self.received += input.len();
        while !input.is_empty() {
            let (copied, _) = self.input.copy_from_slice_to_channel(0, self.filled, input);
            self.filled += copied;
            input = &input[copied..];
            if self.filled == self.input.frames() {
                self.process(output)?;
            }
        }
        Ok(())
    }

    fn flush(&mut self, output: &mut Vec<f32>) -> Result<(), ConvertError> {
        let total = (self.received * RATE.get() as usize).div_ceil(self.rate.get() as usize);
        let end = output.len() + total.saturating_sub(self.emitted);
        while output.len() < end {
            self.process(output)?;
        }
        output.truncate(end);
        Ok(())
    }

    /// Resamples the input block, with silence after `filled`, and appends the output after the delay.
    fn process(&mut self, output: &mut Vec<f32>) -> Result<(), ConvertError> {
        let indexing = Indexing::new().partial_len(self.filled);
        let (_, produced) =
            self.fft
                .process_into_buffer(&self.input, &mut self.output, Some(&indexing))?;
        self.filled = 0;
        let skipped = self.skip.min(produced);
        self.skip -= skipped;
        let start = output.len();
        output.resize(start + produced - skipped, 0.0);
        self.output
            .copy_from_channel_to_slice(0, skipped, &mut output[start..]);
        self.emitted += produced - skipped;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;
    use std::num::NonZeroU32;

    use super::Converter;

    const AMPLITUDE: f32 = 0.5;
    const IRREGULAR: [usize; 5] = [1, 479, 37, 1_013, 160];

    /// The 1 kHz test tone, `elapsed` samples at `rate` after its start.
    fn tone(rate: u16, elapsed: usize) -> f32 {
        let phase = u16::try_from(elapsed * 1_000 % usize::from(rate)).unwrap();
        AMPLITUDE * (TAU * f32::from(phase) / f32::from(rate)).sin()
    }

    /// 250 ms of silence, then the tone, `length` samples at `rate` in total.
    fn silence_then_tone(rate: u16, length: usize) -> Vec<f32> {
        let onset = usize::from(rate) / 4;
        (0..length)
            .map(|index| {
                index
                    .checked_sub(onset)
                    .map_or(0.0, |elapsed| tone(rate, elapsed))
            })
            .collect()
    }

    /// Pushes `input` in slices whose lengths repeat `lengths`, then flushes.
    fn convert(rate: u16, input: &[f32], lengths: &[usize]) -> Vec<f32> {
        let mut converter = Converter::new(NonZeroU32::new(rate.into()).unwrap()).unwrap();
        let mut output = Vec::new();
        let mut rest = input;
        for &length in lengths.iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let (part, tail) = rest.split_at(length.min(rest.len()));
            converter.push(part, &mut output).unwrap();
            rest = tail;
        }
        converter.flush(&mut output).unwrap();
        output
    }

    #[test]
    fn a_tone_keeps_its_time_frequency_and_amplitude_at_16_khz() {
        // 750 ms and one sample: ceil(36_001 / 3) at 48 kHz, ceil(33_076 × 160 / 441) at 44.1 kHz,
        // ceil(18_001 × 2 / 3) at 24 kHz, and 6_001 × 2 at 8 kHz.
        for (rate, length) in [
            (48_000, 12_001),
            (44_100, 12_001),
            (24_000, 12_001),
            (8_000, 12_002),
        ] {
            let input = silence_then_tone(rate, usize::from(rate) * 3 / 4 + 1);
            let output = convert(rate, &input, &IRREGULAR);

            assert_eq!(output.len(), length, "{rate}");
            let onset = 4_000;
            let first = output
                .iter()
                .position(|sample| sample.abs() > AMPLITUDE / 2.0)
                .unwrap();
            assert!(
                first.abs_diff(onset) <= 4,
                "{rate}: the tone starts at {first}"
            );
            let error = (onset + 160..output.len() - 160)
                .map(|index| (output[index] - tone(16_000, index - onset)).abs())
                .fold(0.0, f32::max);
            assert!(error < 0.001, "{rate}: {error}");
        }
    }

    #[test]
    fn the_output_does_not_depend_on_how_the_pushes_divide_the_input() {
        for rate in [48_000, 44_100, 24_000, 8_000] {
            let input = silence_then_tone(rate, usize::from(rate) / 2 + 7);
            let whole = convert(rate, &input, &[input.len()]);

            assert_eq!(convert(rate, &input, &IRREGULAR), whole, "{rate}");
            assert_eq!(convert(rate, &input, &[1]), whole, "{rate}");
        }
    }

    #[test]
    fn audio_at_16_khz_passes_unchanged() {
        let input = silence_then_tone(16_000, 8_001);

        assert_eq!(convert(16_000, &input, &IRREGULAR), input);
    }

    #[test]
    fn a_capture_without_samples_gives_no_output() {
        for rate in [16_000, 24_000, 48_000] {
            let rate = NonZeroU32::new(rate).unwrap();
            let mut output = Vec::new();

            Converter::new(rate).unwrap().flush(&mut output).unwrap();
            let mut converter = Converter::new(rate).unwrap();
            converter.push(&[], &mut output).unwrap();
            converter.flush(&mut output).unwrap();

            assert!(output.is_empty(), "{rate}");
        }
    }

    #[test]
    fn the_debug_output_shows_the_rates_and_no_samples() {
        let mut converter = Converter::new(NonZeroU32::new(48_000).unwrap()).unwrap();
        converter.push(&[0.25; 100], &mut Vec::new()).unwrap();
        let copy = Converter::new(NonZeroU32::new(16_000).unwrap()).unwrap();

        assert_eq!(
            format!("{converter:?}"),
            "Converter { input_rate: 48000, output_rate: 16000, .. }"
        );
        assert_eq!(
            format!("{copy:?}"),
            "Converter { input_rate: 16000, output_rate: 16000, .. }"
        );
    }
}
