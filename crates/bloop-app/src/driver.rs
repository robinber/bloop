//! Driver: gives each event to the core and executes the commands through the ports.

use std::collections::VecDeque;
use std::rc::Rc;

use bloop_audio::buffer;
use bloop_core::domain::Generation;
use bloop_core::error::AppError;
use bloop_core::ports::platform::{AudioInput, Overlay, Scheduler, TextOutput};
use bloop_core::session::{Command, Core, Event};

use crate::hub::Snapshot;
use crate::worker::{Spawner, Task};

/// The capacity of the audio queue of each capture, in samples: 20 s at 48 kHz.
const CAPACITY: usize = 960_000;

/// Gives the settings snapshot of each new generation. It operates on the main thread.
pub(crate) trait Snapshots {
    /// Returns the current settings snapshot.
    fn snapshot(&mut self) -> Rc<Snapshot>;
}

/// Owns the core, the ports, and the transcription tasks. It operates on the main thread.
pub(crate) struct Driver<S, O, T, A, N> {
    core: Core,
    scheduler: S,
    overlay: O,
    output: T,
    input: A,
    spawner: Spawner,
    snapshots: N,
    /// The settings snapshot of the generation whose hold timer runs.
    pending: Option<(Generation, Rc<Snapshot>)>,
    active: Option<Active>,
    jobs: Vec<(Generation, Task)>,
}

/// The capture that the microphone fills, and its transcription task.
struct Active {
    generation: Generation,
    task: Task,
}

impl<S: Scheduler, O: Overlay, T: TextOutput, A: AudioInput, N: Snapshots> Driver<S, O, T, A, N> {
    /// `snapshots` gives the settings of each generation: the hold time and the speech objects.
    pub(crate) fn new(
        scheduler: S,
        overlay: O,
        output: T,
        input: A,
        spawner: Spawner,
        snapshots: N,
    ) -> Self {
        Self {
            core: Core::new(),
            scheduler,
            overlay,
            output,
            input,
            spawner,
            snapshots,
            pending: None,
            active: None,
            jobs: Vec::new(),
        }
    }

    /// Gives the event to the core, then the events that the commands produce, in order.
    pub(crate) fn handle(&mut self, event: Event) {
        let mut events = VecDeque::from([event]);
        while let Some(event) = events.pop_front() {
            tracing::info!(?event, "event");
            for command in self.core.handle(event) {
                tracing::info!(?command, "command");
                events.extend(self.execute(command));
            }
        }
    }

    pub(crate) fn capture_active(&self) -> bool {
        self.core.capture_active()
    }

    fn execute(&mut self, command: Command) -> Option<Event> {
        match command {
            Command::StartHoldTimer(generation) => {
                let snapshot = self.snapshots.snapshot();
                self.scheduler
                    .after(snapshot.hold, Event::HoldElapsed(generation));
                self.pending = Some((generation, snapshot));
            }
            Command::StartCapture(generation) => return self.start_capture(generation),
            Command::RecordTarget(generation) => {
                self.output.record_target(generation);
            }
            Command::StopCaptureAndProcess(generation) => return self.stop_capture(generation),
            Command::CancelCapture => {
                self.active = None;
                self.input.cancel();
            }
            Command::Insert(generation, text) => {
                self.forget(generation);
                self.output.insert(generation, text);
            }
            Command::CopyToClipboard(text) => self.output.copy(&text),
            Command::ShowOrb(view) => self.overlay.show(view),
            Command::CloseOrb => self.overlay.close(),
            Command::CancelWork(generation) => {
                self.forget(generation);
                self.output.cancel(generation);
            }
        }
        None
    }

    /// Starts the microphone and the transcription task with the settings snapshot of `generation`.
    fn start_capture(&mut self, generation: Generation) -> Option<Event> {
        let Some((_, snapshot)) = self
            .pending
            .take()
            .filter(|(pending, _)| *pending == generation)
        else {
            tracing::error!(?generation, "the generation has no settings snapshot");
            return Some(Event::CaptureFailed(generation, AppError::Internal));
        };
        let (writer, reader) = buffer::new(CAPACITY);
        let task = self
            .input
            .start(generation, Box::new(writer))
            .and_then(|rate| {
                self.spawner
                    .spawn(generation, rate, reader, &snapshot.speech)
            });
        match task {
            Ok(task) => {
                self.active = Some(Active { generation, task });
                None
            }
            Err(error) => Some(Event::CaptureFailed(generation, error)),
        }
    }

    fn stop_capture(&mut self, generation: Generation) -> Option<Event> {
        let task = self
            .active
            .take()
            .filter(|active| active.generation == generation)
            .map(|active| active.task);
        if let Some(task) = &task {
            task.end_capture();
        }
        let summary = match self.input.stop(generation) {
            Ok(summary) => summary,
            Err(error) => return Some(Event::CaptureFailed(generation, error)),
        };
        tracing::info!(duration = ?summary.duration, peak = summary.peak, "the capture ended");
        let Some(task) = task else {
            return Some(Event::CaptureFailed(generation, AppError::Internal));
        };
        self.jobs.push((generation, task));
        None
    }

    /// Drops the transcription task of the job, which stops it if it still runs.
    fn forget(&mut self, generation: Generation) {
        self.jobs.retain(|(job, _)| *job != generation);
    }
}

#[cfg(test)]
mod tests;
