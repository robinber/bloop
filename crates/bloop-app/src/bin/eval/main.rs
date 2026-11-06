//! Evaluation tool: runs the correction steps of Bloop on the samples of a test set, with the real API.
//!
//! The tool checks the arguments, the test set, and the prompt before it reads the API key or opens a connection.
//! It runs the samples one at a time, in file order, without retries.
//! It writes each sample, its final text, and a summary to stdout, for manual review.
//! It sends no text to the log. The log lines of the chat adapter go to stderr.

#![forbid(unsafe_code)]

mod args;
mod report;
mod run;
mod set;

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use bloop_core::error::SecretError;
use bloop_core::pipeline::Pipeline;
use bloop_core::ports::ai::{ChatModel, StepContext};
use bloop_core::ports::platform::SecretStore as _;
use bloop_core::steps;
use bloop_macos::keychain::Keychain;
use bloop_providers::{ChatConfig, build_chat_model};
use tokio::runtime::Builder;

use crate::args::{Command, Options, USAGE};
use crate::report::Run;
use crate::run::Recording;

fn main() -> ExitCode {
    match args::parse(std::env::args_os().skip(1)) {
        Ok(Command::Help) => match io::stdout().write_all(USAGE.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("eval: write the help: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::Run(options)) => match evaluate(&options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("eval: {error:#}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("eval: {error}\nRun `eval --help` for the options.");
            ExitCode::FAILURE
        }
    }
}

/// A prompt file: the system message of `LlmRefine`, exactly as the file contains it.
struct Prompt {
    /// The file name without its extension.
    name: String,
    text: String,
}

fn evaluate(options: &Options) -> Result<()> {
    let text = fs::read_to_string(&options.set)
        .with_context(|| format!("read the test set {}", options.set.display()))?;
    let set =
        set::parse(&text).with_context(|| format!("invalid test set {}", options.set.display()))?;
    let samples = set.samples(options.split);
    if samples.is_empty() {
        bail!(
            "the test set {} has no {} samples",
            options.set.display(),
            options.split.name()
        );
    }
    let prompt = read_prompt(&options.prompt)?;
    let config = match &options.model {
        Some(model) => ChatConfig::default().with_model(model.clone()),
        None => ChatConfig::default(),
    };
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .context("start the runtime")?;
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .try_init()
        .map_err(|error| anyhow!(error))
        .context("start the log output")?;

    let provider = config.account();
    let model_id = config.model().to_owned();
    let key = Keychain.get(provider).map_err(|error| match error {
        SecretError::NotFound => anyhow!(
            "Keychain has no API key for the account {provider} of the service bloop, see docs/SETUP.md"
        ),
        SecretError::Unavailable => {
            anyhow!("Keychain did not give the API key of the account {provider}")
        }
    })?;
    let model = build_chat_model(config, key, options.limit)
        .map_err(|error| anyhow!("the chat model did not start: {error:?}"))?;
    let chat = Arc::new(Recording::new(model));
    let pipeline = Pipeline::new(steps::correction(
        Arc::clone(&chat) as Arc<dyn ChatModel>,
        prompt.text.clone(),
    ));
    let context = StepContext {
        language: set.language.clone(),
    };

    let mut out = io::stdout().lock();
    let mut results = Vec::with_capacity(samples.len());
    for (index, sample) in samples.iter().enumerate() {
        let outcome = runtime.block_on(run::sample(&pipeline, &chat, &sample.raw, &context));
        report::write_sample(&mut out, index + 1, samples.len(), sample, &outcome)
            .context("write the report")?;
        results.push((*sample, outcome));
    }
    let run = Run {
        set: &options.set,
        split: options.split,
        provider,
        model: &model_id,
        prompt_name: &prompt.name,
        prompt_text: &prompt.text,
        limit: options.limit,
        pairs: set.pairs.len(),
    };
    report::write_summary(&mut out, &run, &results).context("write the report")?;
    out.flush().context("write the report")
}

fn read_prompt(path: &Path) -> Result<Prompt> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read the prompt {}", path.display()))?;
    if text.trim().is_empty() {
        bail!("the prompt {} is empty", path.display());
    }
    let name = path
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
    Ok(Prompt { name, text })
}
