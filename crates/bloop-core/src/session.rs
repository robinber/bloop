//! Session state machine: the driver sends events, and the core returns the commands to execute.

use std::collections::VecDeque;

use crate::domain::{FinalText, Generation};
use crate::error::AppError;

const MAX_JOBS: usize = 3;

/// An input to the state machine.
#[derive(Debug)]
pub enum Event {
    /// The user pressed the dictation key alone.
    DictationKeyDown,
    /// The user released the dictation key.
    DictationKeyUp,
    /// The user pressed another key or clicked during the hold.
    OtherInputDuringHold,
    /// The user pressed Escape.
    Escape,
    /// The hold timer of a generation expired.
    HoldElapsed(Generation),
    /// The capture of a generation reached its time limit.
    CaptureLimitReached(Generation),
    /// The capture of a generation stopped with an error. The driver logs the error.
    CaptureFailed(Generation, AppError),
    /// The job of a generation produced its final text.
    TextReady(Generation, FinalText),
    /// The job of a generation found no speech.
    NothingHeard(Generation),
    /// The job of a generation failed. The driver logs the error.
    WorkFailed(Generation, AppError),
    /// The paste adapter completed the insertion of a generation.
    InsertDone(Generation, InsertOutcome),
    /// The user selected "Copy last result" in the menu.
    CopyLastResult,
    /// The system is about to sleep.
    SystemSleep,
}

/// An action that the driver executes, in the order that the core returns it.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Send `HoldElapsed` for this generation after the hold threshold.
    StartHoldTimer(Generation),
    /// Start the microphone capture.
    StartCapture(Generation),
    /// Record the frontmost application and the input counters for the target check.
    RecordTarget(Generation),
    /// Stop the capture and start the job that processes the audio.
    StopCaptureAndProcess(Generation),
    /// Stop the capture and delete its audio.
    CancelCapture,
    /// Stop the job of this generation.
    CancelWork(Generation),
    /// Paste the final text if the target check passes.
    Insert(Generation, FinalText),
    /// Put the text in the clipboard.
    CopyToClipboard(FinalText),
    /// Open the orb with this view, or change its current view.
    ShowOrb(OrbView),
    /// Close the orb with its closing animation.
    CloseOrb,
}

/// The content that the orb shows.
#[derive(Debug, PartialEq, Eq)]
pub enum OrbView {
    /// The bars follow the voice level.
    Listening,
    /// Red bars for a short time, then the orb closes.
    Error,
    /// The final text, with an instruction to paste it manually.
    Card(FinalText),
}

/// The result of an insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    /// The target application read the text after the paste command.
    Landed,
    /// Bloop did not paste the text, or the target did not read it.
    Fallback(FallbackReason),
}

/// Why an insertion used the fallback card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// The frontmost application or the user input changed after the key release.
    TargetChanged,
    /// Secure input was active.
    SecureInput,
    /// The target did not read the clipboard within the time limit.
    NotRead,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Capture {
    #[default]
    Idle,
    Pending(Generation),
    Listening(Generation),
}

#[derive(Debug)]
enum JobStatus {
    Processing,
    Ready(FinalText),
    Inserting(FinalText),
}

#[derive(Debug)]
struct Job {
    generation: Generation,
    status: JobStatus,
}

/// The session state machine of Bloop.
///
/// It owns one capture at a time and a queue of jobs that paste in session order.
/// A fallback card that arrives during a capture waits until the capture ends.
#[derive(Debug, Default)]
pub struct Core {
    capture: Capture,
    jobs: VecDeque<Job>,
    next_generation: u64,
    last_result: Option<FinalText>,
    pending_card: Option<FinalText>,
}

impl Core {
    /// Creates a state machine with no capture and no job.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` while a capture is pending or listening.
    ///
    /// The event tap blocks Escape only in this condition.
    #[must_use]
    pub fn capture_active(&self) -> bool {
        self.capture != Capture::Idle
    }

    /// Applies one event and returns the commands that the driver must execute, in order.
    #[must_use = "the driver must execute the returned commands"]
    pub fn handle(&mut self, event: Event) -> Vec<Command> {
        match event {
            Event::DictationKeyDown => self.key_down(),
            Event::DictationKeyUp => self.key_up(),
            Event::OtherInputDuringHold | Event::Escape => self.cancel_capture(),
            Event::HoldElapsed(generation) => self.hold_elapsed(generation),
            Event::CaptureLimitReached(generation) => self.capture_limit_reached(generation),
            Event::CaptureFailed(generation, _) => self.capture_failed(generation),
            Event::TextReady(generation, text) => self.text_ready(generation, text),
            Event::NothingHeard(generation) => self.nothing_heard(generation),
            Event::WorkFailed(generation, _) => self.work_failed(generation),
            Event::InsertDone(generation, outcome) => self.insert_done(generation, outcome),
            Event::CopyLastResult => self.copy_last_result(),
            Event::SystemSleep => self.system_sleep(),
        }
    }

    fn key_down(&mut self) -> Vec<Command> {
        if self.capture != Capture::Idle {
            return Vec::new();
        }
        let generation = self.new_generation();
        self.capture = Capture::Pending(generation);
        vec![Command::StartHoldTimer(generation)]
    }

    fn hold_elapsed(&mut self, generation: Generation) -> Vec<Command> {
        if self.capture != Capture::Pending(generation) {
            return Vec::new();
        }
        if self.jobs.len() >= MAX_JOBS {
            let mut commands = vec![Command::ShowOrb(OrbView::Error)];
            commands.extend(self.end_capture());
            return commands;
        }
        self.capture = Capture::Listening(generation);
        vec![
            Command::StartCapture(generation),
            Command::ShowOrb(OrbView::Listening),
        ]
    }

    fn key_up(&mut self) -> Vec<Command> {
        match self.capture {
            Capture::Idle => Vec::new(),
            Capture::Pending(_) => self.end_capture().into_iter().collect(),
            Capture::Listening(generation) => self.finish_capture(generation),
        }
    }

    fn capture_limit_reached(&mut self, generation: Generation) -> Vec<Command> {
        if self.capture == Capture::Listening(generation) {
            self.finish_capture(generation)
        } else {
            Vec::new()
        }
    }

    fn finish_capture(&mut self, generation: Generation) -> Vec<Command> {
        self.jobs.push_back(Job {
            generation,
            status: JobStatus::Processing,
        });
        let mut commands = vec![
            Command::RecordTarget(generation),
            Command::StopCaptureAndProcess(generation),
            Command::CloseOrb,
        ];
        commands.extend(self.end_capture());
        commands
    }

    fn cancel_capture(&mut self) -> Vec<Command> {
        let mut commands = match self.capture {
            Capture::Idle => return Vec::new(),
            Capture::Pending(_) => Vec::new(),
            Capture::Listening(_) => vec![Command::CancelCapture, Command::CloseOrb],
        };
        commands.extend(self.end_capture());
        commands
    }

    fn capture_failed(&mut self, generation: Generation) -> Vec<Command> {
        if self.capture != Capture::Listening(generation) {
            return self.work_failed(generation);
        }
        let mut commands = vec![Command::CancelCapture, Command::ShowOrb(OrbView::Error)];
        commands.extend(self.end_capture());
        commands
    }

    fn end_capture(&mut self) -> Option<Command> {
        self.capture = Capture::Idle;
        self.pending_card
            .take()
            .map(|text| Command::ShowOrb(OrbView::Card(text)))
    }

    fn text_ready(&mut self, generation: Generation, text: FinalText) -> Vec<Command> {
        let Some(job) = self.processing_job_mut(generation) else {
            return Vec::new();
        };
        job.status = JobStatus::Ready(text);
        self.insert_next()
    }

    fn nothing_heard(&mut self, generation: Generation) -> Vec<Command> {
        if self.remove_processing_job(generation) {
            self.insert_next()
        } else {
            Vec::new()
        }
    }

    fn work_failed(&mut self, generation: Generation) -> Vec<Command> {
        if !self.remove_processing_job(generation) {
            return Vec::new();
        }
        let mut commands = Vec::new();
        if self.capture == Capture::Idle {
            commands.push(Command::ShowOrb(OrbView::Error));
        }
        commands.extend(self.insert_next());
        commands
    }

    fn insert_done(&mut self, generation: Generation, outcome: InsertOutcome) -> Vec<Command> {
        let head_is_inserting = self.jobs.front().is_some_and(|job| {
            job.generation == generation && matches!(job.status, JobStatus::Inserting(_))
        });
        if !head_is_inserting {
            return Vec::new();
        }
        let Some(Job {
            status: JobStatus::Inserting(text),
            ..
        }) = self.jobs.pop_front()
        else {
            return Vec::new();
        };
        let mut commands = Vec::new();
        if let InsertOutcome::Fallback(_) = outcome {
            if self.capture == Capture::Idle {
                commands.push(Command::ShowOrb(OrbView::Card(text)));
            } else {
                self.pending_card = Some(text);
            }
        }
        commands.extend(self.insert_next());
        commands
    }

    fn copy_last_result(&self) -> Vec<Command> {
        self.last_result
            .iter()
            .map(|text| Command::CopyToClipboard(text.clone()))
            .collect()
    }

    fn system_sleep(&mut self) -> Vec<Command> {
        let mut commands = self.cancel_capture();
        commands.extend(
            self.jobs
                .drain(..)
                .map(|job| Command::CancelWork(job.generation)),
        );
        commands
    }

    fn insert_next(&mut self) -> Vec<Command> {
        let Some(head) = self.jobs.front_mut() else {
            return Vec::new();
        };
        let JobStatus::Ready(text) = &head.status else {
            return Vec::new();
        };
        let text = text.clone();
        head.status = JobStatus::Inserting(text.clone());
        self.last_result = Some(text.clone());
        vec![Command::Insert(head.generation, text)]
    }

    fn processing_job_mut(&mut self, generation: Generation) -> Option<&mut Job> {
        self.jobs
            .iter_mut()
            .find(|job| job.generation == generation && matches!(job.status, JobStatus::Processing))
    }

    fn remove_processing_job(&mut self, generation: Generation) -> bool {
        let position = self.jobs.iter().position(|job| {
            job.generation == generation && matches!(job.status, JobStatus::Processing)
        });
        position.is_some_and(|index| self.jobs.remove(index).is_some())
    }

    fn new_generation(&mut self) -> Generation {
        let generation = Generation(self.next_generation);
        self.next_generation = self.next_generation.wrapping_add(1);
        generation
    }
}

#[cfg(test)]
mod tests;
