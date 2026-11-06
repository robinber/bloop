//! The command-line arguments of the evaluation tool.

use std::ffi::OsString;
use std::fmt;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::Duration;

use crate::set::Split;

pub(crate) const USAGE: &str = "\
Usage: eval [options]

Runs the correction steps of Bloop on a test set with the real API.
Prints each sample, its final text, and a summary.

Options:
  --set <path>      Test set file (default: prompts/test-set-fr.toml)
  --prompt <path>   Prompt file (default: prompts/natural-light.md)
  --model <id>      Chat model identifier (default: the model of the default chat settings)
  --validation      Run only the reserved validation samples (default: only the other samples)
  --limit-ms <n>    Time limit of each chat request, from 1 to 300000 ms (default: 30000)
  --help            Show this help
";

const DEFAULT_SET: &str = "prompts/test-set-fr.toml";
const DEFAULT_PROMPT: &str = "prompts/natural-light.md";
const DEFAULT_LIMIT: Duration = Duration::from_secs(30);
const LIMIT_MS: RangeInclusive<u64> = 1..=300_000;

/// What the arguments ask for.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Help,
    Run(Options),
}

/// The options of an evaluation run.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) set: PathBuf,
    pub(crate) prompt: PathBuf,
    /// The model identifier, or `None` for the model of the default chat settings.
    pub(crate) model: Option<String>,
    pub(crate) split: Split,
    /// The time limit of each chat request.
    pub(crate) limit: Duration,
}

/// Why the arguments are invalid.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ArgsError {
    Unknown(String),
    MissingValue(&'static str),
    Repeated(&'static str),
    NotUtf8(&'static str),
    EmptyModel,
    Limit(String),
}

impl fmt::Display for ArgsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(argument) => write!(f, "unknown argument {argument:?}"),
            Self::MissingValue(option) => write!(f, "{option} needs a value"),
            Self::Repeated(option) => write!(f, "{option} occurs more than one time"),
            Self::NotUtf8(option) => write!(f, "the value of {option} is not UTF-8 text"),
            Self::EmptyModel => write!(f, "the value of --model is empty"),
            Self::Limit(value) => write!(
                f,
                "the value of --limit-ms must be a whole number from {} to {}, not {value:?}",
                LIMIT_MS.start(),
                LIMIT_MS.end()
            ),
        }
    }
}

/// Parses the arguments after the program name. `--help` stops the parse.
pub(crate) fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, ArgsError> {
    let mut set = None;
    let mut prompt = None;
    let mut model = None;
    let mut validation = None;
    let mut limit = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--help" | "-h") => return Ok(Command::Help),
            Some("--set") => store(&mut set, "--set", value(&mut arguments, "--set")?.into())?,
            Some("--prompt") => store(
                &mut prompt,
                "--prompt",
                value(&mut arguments, "--prompt")?.into(),
            )?,
            Some("--model") => {
                let id = value(&mut arguments, "--model")?
                    .into_string()
                    .map_err(|_| ArgsError::NotUtf8("--model"))?;
                if id.trim().is_empty() {
                    return Err(ArgsError::EmptyModel);
                }
                store(&mut model, "--model", id)?;
            }
            Some("--validation") => store(&mut validation, "--validation", Split::Validation)?,
            Some("--limit-ms") => {
                let text = value(&mut arguments, "--limit-ms")?;
                let millis = text
                    .to_str()
                    .and_then(|text| text.parse().ok())
                    .filter(|millis| LIMIT_MS.contains(millis))
                    .ok_or_else(|| ArgsError::Limit(text.to_string_lossy().into_owned()))?;
                store(&mut limit, "--limit-ms", Duration::from_millis(millis))?;
            }
            _ => return Err(ArgsError::Unknown(argument.to_string_lossy().into_owned())),
        }
    }
    Ok(Command::Run(Options {
        set: set.unwrap_or_else(|| DEFAULT_SET.into()),
        prompt: prompt.unwrap_or_else(|| DEFAULT_PROMPT.into()),
        model,
        split: validation.unwrap_or(Split::Dev),
        limit: limit.unwrap_or(DEFAULT_LIMIT),
    }))
}

/// Returns the value after `option`. A missing value or another option is an error.
fn value(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &'static str,
) -> Result<OsString, ArgsError> {
    arguments
        .next()
        .filter(|value| !value.as_encoded_bytes().starts_with(b"--"))
        .ok_or(ArgsError::MissingValue(option))
}

fn store<T>(slot: &mut Option<T>, option: &'static str, value: T) -> Result<(), ArgsError> {
    if slot.replace(value).is_some() {
        return Err(ArgsError::Repeated(option));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{ArgsError, Command, Options, parse};
    use crate::set::Split;

    fn arguments(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn options(list: &[&str]) -> Options {
        match parse(arguments(list)) {
            Ok(Command::Run(options)) => options,
            other => panic!("{other:?}"),
        }
    }

    fn error(list: &[&str]) -> ArgsError {
        parse(arguments(list)).unwrap_err()
    }

    #[test]
    fn without_arguments_the_run_uses_the_defaults() {
        assert_eq!(
            options(&[]),
            Options {
                set: PathBuf::from("prompts/test-set-fr.toml"),
                prompt: PathBuf::from("prompts/natural-light.md"),
                model: None,
                split: Split::Dev,
                limit: Duration::from_secs(30),
            }
        );
    }

    #[test]
    fn each_option_sets_its_value() {
        assert_eq!(
            options(&[
                "--set",
                "set.toml",
                "--prompt",
                "prompts/natural-full.md",
                "--model",
                "ministral-8b-2512",
                "--validation",
                "--limit-ms",
                "8000",
            ]),
            Options {
                set: PathBuf::from("set.toml"),
                prompt: PathBuf::from("prompts/natural-full.md"),
                model: Some("ministral-8b-2512".to_owned()),
                split: Split::Validation,
                limit: Duration::from_millis(8_000),
            }
        );
    }

    #[test]
    fn help_stops_the_parse() {
        for list in [
            &["--help"][..],
            &["-h"],
            &["--validation", "--help", "--bad"],
        ] {
            assert_eq!(parse(arguments(list)), Ok(Command::Help), "{list:?}");
        }
    }

    #[test]
    fn an_unknown_argument_is_an_error() {
        assert_eq!(
            error(&["--validate"]),
            ArgsError::Unknown("--validate".to_owned())
        );
        assert_eq!(
            error(&["set.toml"]),
            ArgsError::Unknown("set.toml".to_owned())
        );
    }

    #[test]
    fn an_option_without_its_value_is_an_error() {
        assert_eq!(error(&["--set"]), ArgsError::MissingValue("--set"));
        assert_eq!(
            error(&["--model", "--validation"]),
            ArgsError::MissingValue("--model")
        );
    }

    #[test]
    fn a_repeated_option_is_an_error() {
        assert_eq!(
            error(&["--prompt", "a.md", "--prompt", "b.md"]),
            ArgsError::Repeated("--prompt")
        );
        assert_eq!(
            error(&["--validation", "--validation"]),
            ArgsError::Repeated("--validation")
        );
    }

    #[test]
    fn the_model_must_be_utf8_text_that_is_not_empty() {
        assert_eq!(error(&["--model", " "]), ArgsError::EmptyModel);
        let list = vec![OsString::from("--model"), OsString::from_vec(vec![0xff])];
        assert_eq!(parse(list), Err(ArgsError::NotUtf8("--model")));
    }

    #[test]
    fn the_limit_must_be_a_whole_number_of_milliseconds_in_its_range() {
        assert_eq!(
            options(&["--limit-ms", "1"]).limit,
            Duration::from_millis(1)
        );
        assert_eq!(
            options(&["--limit-ms", "300000"]).limit,
            Duration::from_secs(300)
        );
        for value in ["0", "300001", "-5", "8s", "1.5", ""] {
            assert_eq!(
                error(&["--limit-ms", value]),
                ArgsError::Limit(value.to_owned()),
                "{value:?}"
            );
        }
    }

    #[test]
    fn a_path_can_be_text_that_is_not_utf8() {
        let list = vec![OsString::from("--set"), OsString::from_vec(vec![0xff])];

        let Ok(Command::Run(options)) = parse(list) else {
            panic!("the parse failed");
        };

        assert_eq!(options.set.as_os_str().as_encoded_bytes(), [0xff]);
    }
}
