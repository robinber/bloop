//! Bounded sample queue between the audio callback and the worker of one capture.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bloop_core::error::AudioQueueError;
use bloop_core::ports::platform::AudioSink;
use rtrb::chunks::ChunkError;
use rtrb::{Consumer, Producer, RingBuffer};

/// Creates a queue for `capacity` samples. It allocates all its memory here.
#[must_use]
pub fn new(capacity: usize) -> (Writer, Reader) {
    let (producer, consumer) = RingBuffer::new(capacity);
    let marks = Arc::new(Marks::default());
    let writer = Writer {
        producer,
        marks: Arc::clone(&marks),
    };
    (writer, Reader { consumer, marks })
}

#[derive(Debug, Default)]
struct Marks {
    end: AtomicBool,
    overflow: AtomicBool,
}

/// The side of the audio callback.
#[derive(Debug)]
pub struct Writer {
    producer: Producer<f32>,
    marks: Arc<Marks>,
}

impl AudioSink for Writer {
    fn try_push(&mut self, samples: &[f32]) -> Result<(), AudioQueueError> {
        match self.producer.push_entire_slice(samples) {
            Ok(()) => Ok(()),
            Err(ChunkError::TooFewSlots(_)) => {
                self.marks.overflow.store(true, Ordering::Relaxed);
                Err(AudioQueueError::Full)
            }
        }
    }
}

/// The side of the worker.
#[derive(Debug)]
pub struct Reader {
    consumer: Consumer<f32>,
    marks: Arc<Marks>,
}

impl Reader {
    /// Returns the end mark of the capture, for the driver.
    #[must_use]
    pub fn end_mark(&self) -> EndMark {
        EndMark(Arc::clone(&self.marks))
    }

    /// Returns the failure of the queue, if any, without a drain.
    #[must_use]
    pub fn failure(&self) -> Option<AudioQueueError> {
        self.state().err()
    }

    /// Moves the queued samples to the end of `samples`.
    ///
    /// One call moves only the samples that are in the queue when it starts.
    /// Samples that arrive during the call stay for the next call.
    ///
    /// Returns `Ended` when the writer is gone after the end mark: no more samples can arrive.
    ///
    /// # Errors
    ///
    /// `Full` after a push that did not fit, and `Closed` when the writer is gone without the end mark.
    pub fn drain(&mut self, samples: &mut Vec<f32>) -> Result<Status, AudioQueueError> {
        let gone = self.state()?;
        let limit = self.consumer.slots();
        samples.reserve(limit);
        if let Ok(chunk) = self.consumer.read_chunk(limit) {
            let (first, second) = chunk.as_slices();
            samples.extend_from_slice(first);
            samples.extend_from_slice(second);
            chunk.commit_all();
        }
        Ok(if gone { Status::Ended } else { Status::Open })
    }

    /// Returns `true` when the writer is gone after the end mark, and `false` while it can push.
    fn state(&self) -> Result<bool, AudioQueueError> {
        // This `Acquire` load synchronizes with the drop of the writer. Thus, when the writer is gone,
        // the marks and all its samples are visible after this load.
        let gone = self.consumer.is_abandoned();
        if self.marks.overflow.load(Ordering::Relaxed) {
            return Err(AudioQueueError::Full);
        }
        if gone && !self.marks.end.load(Ordering::Relaxed) {
            return Err(AudioQueueError::Closed);
        }
        Ok(gone)
    }
}

/// Tells the worker that the capture ends normally. Set it before the audio input stops.
#[derive(Debug)]
pub struct EndMark(Arc<Marks>);

impl EndMark {
    /// Marks the end of the capture.
    pub fn set(&self) {
        self.0.end.store(true, Ordering::Relaxed);
    }
}

/// The state of the queue after a drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// More samples can arrive.
    Open,
    /// The capture ended, and the drain returned its last samples.
    Ended,
}

#[cfg(test)]
mod tests {
    use bloop_core::error::AudioQueueError;
    use bloop_core::ports::platform::AudioSink;

    use super::{Status, new};

    #[test]
    fn the_queue_ends_only_when_the_writer_goes_after_the_end_mark() {
        let (mut writer, mut reader) = new(8);
        let mut samples = Vec::new();

        writer.try_push(&[0.1, 0.2]).unwrap();
        assert_eq!(reader.failure(), None);
        reader.end_mark().set();
        writer.try_push(&[0.3]).unwrap();
        assert_eq!(reader.drain(&mut samples), Ok(Status::Open));
        drop(writer);

        assert_eq!(reader.failure(), None);
        assert_eq!(reader.drain(&mut samples), Ok(Status::Ended));
        assert_eq!(samples, [0.1, 0.2, 0.3]);
    }

    #[test]
    fn a_drain_keeps_the_order_of_samples_that_wrap_around_the_ring() {
        let (mut writer, mut reader) = new(4);
        let mut samples = Vec::new();

        writer.try_push(&[0.1, 0.2, 0.3]).unwrap();
        assert_eq!(reader.drain(&mut samples), Ok(Status::Open));
        writer.try_push(&[0.4, 0.5, 0.6]).unwrap();
        assert_eq!(reader.drain(&mut samples), Ok(Status::Open));

        assert_eq!(samples, [0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
    }

    #[test]
    fn a_push_that_does_not_fit_fails_the_queue_for_good() {
        let (mut writer, mut reader) = new(4);
        let mut samples = Vec::new();

        writer.try_push(&[0.1, 0.2, 0.3]).unwrap();
        assert_eq!(writer.try_push(&[0.4, 0.5]), Err(AudioQueueError::Full));
        assert_eq!(reader.consumer.slots(), 3);
        assert_eq!(reader.failure(), Some(AudioQueueError::Full));
        reader.end_mark().set();
        drop(writer);

        assert_eq!(reader.drain(&mut samples), Err(AudioQueueError::Full));
        assert_eq!(reader.failure(), Some(AudioQueueError::Full));
        assert_eq!(samples, []);
    }

    #[test]
    fn the_queue_closes_when_the_writer_goes_without_the_end_mark() {
        let (mut writer, mut reader) = new(8);
        let mut samples = Vec::new();

        writer.try_push(&[0.1]).unwrap();
        drop(writer);

        assert_eq!(reader.failure(), Some(AudioQueueError::Closed));
        assert_eq!(reader.drain(&mut samples), Err(AudioQueueError::Closed));
    }

    #[test]
    fn a_drain_after_the_end_adds_no_samples() {
        let (mut writer, mut reader) = new(8);
        let mut samples = Vec::new();

        writer.try_push(&[0.1, 0.2]).unwrap();
        reader.end_mark().set();
        drop(writer);

        assert_eq!(reader.drain(&mut samples), Ok(Status::Ended));
        assert_eq!(reader.drain(&mut samples), Ok(Status::Ended));
        assert_eq!(samples, [0.1, 0.2]);
    }
}
