//! Error categories that events carry to the state machine.

/// A macOS permission that Bloop needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// Microphone access for the capture.
    Microphone,
    /// Accessibility access for the active event tap and the paste command.
    Accessibility,
    /// Input Monitoring access for key events.
    InputMonitoring,
}

/// A failure of an AI provider request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderError {
    /// The request did not reach the provider.
    Network,
    /// The provider did not answer within the time limit.
    Timeout,
    /// The provider answered with an HTTP error status.
    Http(u16),
    /// The provider rejected the API key.
    Unauthorized,
    /// The provider answered with data that Bloop cannot use.
    InvalidResponse,
}

/// A failure of the bounded audio queue between the microphone and the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioQueueError {
    /// The queue had no space for the samples. Bloop never drops samples silently.
    Full,
    /// The microphone stopped before the end of the capture.
    Closed,
}

/// A failure that stops a capture or a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppError {
    /// A required permission is missing.
    PermissionMissing(Permission),
    /// The microphone is not available or stopped during the capture.
    Microphone,
    /// The audio queue failed.
    AudioQueue(AudioQueueError),
    /// An AI provider request failed.
    Provider(ProviderError),
}
