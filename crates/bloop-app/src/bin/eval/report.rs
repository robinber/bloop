//! The output of the evaluation: one block for each sample, then a summary.

use std::io::{self, Write};
use std::ops::AddAssign;
use std::path::Path;
use std::time::Duration;

use crate::run::{Outcome, Request};
use crate::set::{Sample, Split};

/// The punctuation marks that the report counts, in report order.
const MARKS: [char; 7] = [',', '.', '?', '!', ':', ';', '…'];

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The number of each punctuation mark in a text, in the order of [`MARKS`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Marks([usize; MARKS.len()]);

impl Marks {
    /// Counts each character of `text` that is one of [`MARKS`]. Thus `...` counts as three periods.
    pub(crate) fn count(text: &str) -> Self {
        let mut marks = Self::default();
        for character in text.chars() {
            if let Some(count) = MARKS
                .iter()
                .position(|mark| *mark == character)
                .and_then(|index| marks.0.get_mut(index))
            {
                *count += 1;
            }
        }
        marks
    }

    pub(crate) fn total(&self) -> usize {
        self.0.iter().sum()
    }
}

impl AddAssign for Marks {
    fn add_assign(&mut self, other: Self) {
        for (count, added) in self.0.iter_mut().zip(other.0) {
            *count += added;
        }
    }
}

/// Returns the FNV-1a hash, 64 bits, of the bytes of `text`.
pub(crate) fn fnv1a64(text: &str) -> u64 {
    text.bytes().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

/// Returns the median of `durations`: the middle value, or the mean of the two middle values.
pub(crate) fn median(durations: &[Duration]) -> Option<Duration> {
    let mut sorted = durations.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    let upper = *sorted.get(middle)?;
    if sorted.len() % 2 == 1 {
        return Some(upper);
    }
    let lower = *sorted.get(middle.checked_sub(1)?)?;
    Some(lower + upper.saturating_sub(lower) / 2)
}

/// The settings of a run, for the summary.
pub(crate) struct Run<'a> {
    pub(crate) set: &'a Path,
    pub(crate) split: Split,
    pub(crate) provider: &'a str,
    pub(crate) model: &'a str,
    pub(crate) prompt_name: &'a str,
    pub(crate) prompt_text: &'a str,
    pub(crate) limit: Duration,
    pub(crate) pairs: usize,
}

/// Writes the block of the sample at `position`, from 1, of `count` samples.
pub(crate) fn write_sample(
    out: &mut impl Write,
    position: usize,
    count: usize,
    sample: &Sample,
    outcome: &Outcome,
) -> io::Result<()> {
    writeln!(
        out,
        "[{position}/{count}] {} | {} | {}",
        sample.id,
        sample.category,
        sample.split().name()
    )?;
    writeln!(out, "  raw:    {}", one_line(&sample.raw))?;
    let fallback = outcome.fallback();
    match &outcome.text {
        Some(text) => {
            let label = match &fallback {
                Some(_) => "[raw fallback] ",
                None if text.as_str() == sample.raw => "[unchanged] ",
                None => "",
            };
            writeln!(out, "  final:  {label}{}", one_line(text.as_str()))?;
        }
        None => writeln!(out, "  final:  [nothing heard]")?,
    }
    let before = Marks::count(&sample.raw);
    let after = outcome
        .text
        .as_ref()
        .map_or_else(Marks::default, |text| Marks::count(text.as_str()));
    write!(out, "  marks:  ")?;
    write_marks(out, before, after)?;
    writeln!(out)?;
    write!(out, "  chat:   ")?;
    if outcome.requests.is_empty() {
        write!(out, "no request")?;
    }
    for (index, request) in outcome.requests.iter().enumerate() {
        if index > 0 {
            write!(out, "; ")?;
        }
        write_request(out, request)?;
    }
    writeln!(out)?;
    write!(out, "  steps:  ")?;
    for (index, report) in outcome.report.iter().enumerate() {
        if index > 0 {
            write!(out, ", ")?;
        }
        write!(out, "{} {:?}", report.step, report.result)?;
    }
    writeln!(out)?;
    if let Some(note) = &sample.note {
        writeln!(out, "  note:   {}", one_line(note))?;
    }
    writeln!(out)
}

/// Writes the summary of the run.
pub(crate) fn write_summary(
    out: &mut impl Write,
    run: &Run<'_>,
    results: &[(&Sample, Outcome)],
) -> io::Result<()> {
    let durations: Vec<Duration> = results
        .iter()
        .flat_map(|(_, outcome)| outcome.requests.iter().map(|request| request.elapsed))
        .collect();
    let fallbacks: Vec<(&str, String)> = results
        .iter()
        .filter_map(|(sample, outcome)| Some((sample.id.as_str(), outcome.fallback()?)))
        .collect();
    let mut before = Marks::default();
    let mut after = Marks::default();
    let mut corrected = 0;
    for (sample, outcome) in results {
        if let (Some(text), None) = (&outcome.text, outcome.fallback()) {
            before += Marks::count(&sample.raw);
            after += Marks::count(text.as_str());
            corrected += 1;
        }
    }
    writeln!(out, "Summary")?;
    writeln!(
        out,
        "  set:          {}, {} {} samples",
        run.set.display(),
        results.len(),
        run.split.name()
    )?;
    writeln!(out, "  provider:     {}", run.provider)?;
    writeln!(out, "  model:        {}", run.model)?;
    writeln!(
        out,
        "  prompt:       {}, FNV-1a 64 {:016x}",
        run.prompt_name,
        fnv1a64(run.prompt_text)
    )?;
    writeln!(out, "  chat limit:   {} ms", run.limit.as_millis())?;
    match (median(&durations), durations.iter().max()) {
        (Some(median), Some(max)) => writeln!(
            out,
            "  chat time:    median {} ms, max {} ms, {} requests",
            median.as_millis(),
            max.as_millis(),
            durations.len()
        )?,
        _ => writeln!(out, "  chat time:    no request")?,
    }
    writeln!(out, "  fallbacks:    {}", fallbacks.len())?;
    for (id, reason) in &fallbacks {
        writeln!(out, "                {id}: {reason}")?;
    }
    write!(out, "  punctuation:  ")?;
    write_marks(out, before, after)?;
    writeln!(out, ", over {corrected} samples without fallback")?;
    writeln!(
        out,
        "  dictionary:   {} pairs in the set, the correction does not apply them",
        run.pairs
    )
}

/// Returns `text` with each control character as its Rust escape, so that a reply cannot
/// send a terminal escape sequence or break the line of its field.
fn one_line(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_control() {
            line.extend(character.escape_default());
        } else {
            line.push(character);
        }
    }
    line
}

fn write_marks(out: &mut impl Write, before: Marks, after: Marks) -> io::Result<()> {
    for ((mark, before), after) in MARKS.iter().zip(before.0).zip(after.0) {
        write!(out, "{mark} {before}→{after}  ")?;
    }
    write!(out, "total {}→{}", before.total(), after.total())
}

fn write_request(out: &mut impl Write, request: &Request) -> io::Result<()> {
    let millis = request.elapsed.as_millis();
    match request.result {
        Ok(finish) => write!(out, "{millis} ms, finish {finish:?}"),
        Err(error) => write!(out, "{millis} ms, error {error:?}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use bloop_core::domain::FinalText;
    use bloop_core::error::{ProviderError, Rejection, StepError};
    use bloop_core::pipeline::{StepReport, StepResult};
    use bloop_core::ports::ai::Finish;

    use super::{Marks, Run, fnv1a64, median, write_sample, write_summary};
    use crate::run::{Outcome, Request};
    use crate::set::{Sample, Split};

    fn millis(values: &[u64]) -> Vec<Duration> {
        values.iter().copied().map(Duration::from_millis).collect()
    }

    fn sample(id: &str, raw: &str, validation: bool) -> Sample {
        Sample {
            id: id.to_owned(),
            category: "question",
            raw: raw.to_owned(),
            note: Some("Keep the question.".to_owned()),
            validation,
        }
    }

    fn outcome(text: &str, millis: u64, result: Result<Finish, ProviderError>) -> Outcome {
        let refine = match result {
            Ok(Finish::Complete) => StepResult::Applied,
            Ok(_) => StepResult::Failed(StepError::Rejected(Rejection::Incomplete)),
            Err(error) => StepResult::Failed(StepError::Provider(error)),
        };
        Outcome {
            text: Some(FinalText::new(text.to_owned())),
            report: vec![
                StepReport {
                    step: "llm_refine",
                    result: refine,
                },
                StepReport {
                    step: "trim",
                    result: StepResult::Applied,
                },
            ],
            requests: vec![Request {
                elapsed: Duration::from_millis(millis),
                result,
            }],
        }
    }

    fn text(write: impl FnOnce(&mut Vec<u8>) -> std::io::Result<()>) -> String {
        let mut out = Vec::new();
        write(&mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn marks_count_each_punctuation_character() {
        let marks = Marks::count("Oui, non. Quoi ? Ah ! Note : un ; deux… trois... 2,5 %");

        assert_eq!(marks, Marks([2, 4, 1, 1, 1, 1, 1]));
        assert_eq!(marks.total(), 11);
        assert_eq!(Marks::count("sans ponctuation").total(), 0);
    }

    #[test]
    fn marks_add_each_count() {
        let mut marks = Marks::count("a, b.");

        marks += Marks::count("c, d? e…");

        assert_eq!(marks, Marks([2, 1, 1, 0, 0, 0, 1]));
    }

    #[test]
    fn fnv1a64_gives_the_reference_values() {
        assert_eq!(format!("{:016x}", fnv1a64("")), "cbf29ce484222325");
        assert_eq!(format!("{:016x}", fnv1a64("a")), "af63dc4c8601ec8c");
        assert_eq!(format!("{:016x}", fnv1a64("foobar")), "85944171f73967e8");
        assert_eq!(format!("{:016x}", fnv1a64("é")), "0ac21707b7181e01");
    }

    #[test]
    fn the_median_is_the_middle_value_or_the_mean_of_the_two_middle_values() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&millis(&[700])), Some(Duration::from_millis(700)));
        assert_eq!(
            median(&millis(&[900, 300, 600])),
            Some(Duration::from_millis(600))
        );
        assert_eq!(
            median(&millis(&[1_000, 200, 400, 30_000])),
            Some(Duration::from_millis(700))
        );
        assert_eq!(median(&[Duration::MAX, Duration::MAX]), Some(Duration::MAX));
    }

    #[test]
    fn a_sample_block_shows_the_texts_the_marks_the_request_the_steps_and_the_note() {
        let sample = sample("question-01", "tu viens ce soir", false);
        let outcome = outcome("Tu viens ce soir ?", 640, Ok(Finish::Complete));

        let block = text(|out| write_sample(out, 1, 25, &sample, &outcome));

        assert_eq!(
            block,
            "[1/25] question-01 | question | dev\n\
             \x20 raw:    tu viens ce soir\n\
             \x20 final:  Tu viens ce soir ?\n\
             \x20 marks:  , 0→0  . 0→0  ? 0→1  ! 0→0  : 0→0  ; 0→0  … 0→0  total 0→1\n\
             \x20 chat:   640 ms, finish Complete\n\
             \x20 steps:  llm_refine Applied, trim Applied\n\
             \x20 note:   Keep the question.\n\n"
        );
    }

    #[test]
    fn a_sample_block_shows_the_control_characters_of_its_texts_as_escapes() {
        let mut sample = sample("question-03", "tu viens\nce soir", false);
        sample.note = Some("Check\x1b[2J this.".to_owned());
        let outcome = outcome("Tu viens\x1b[31m ?\r\n", 640, Ok(Finish::Complete));

        let block = text(|out| write_sample(out, 3, 25, &sample, &outcome));

        assert_eq!(
            block,
            "[3/25] question-03 | question | dev\n\
             \x20 raw:    tu viens\\nce soir\n\
             \x20 final:  Tu viens\\u{1b}[31m ?\\r\\n\n\
             \x20 marks:  , 0→0  . 0→0  ? 0→1  ! 0→0  : 0→0  ; 0→0  … 0→0  total 0→1\n\
             \x20 chat:   640 ms, finish Complete\n\
             \x20 steps:  llm_refine Applied, trim Applied\n\
             \x20 note:   Check\\u{1b}[2J this.\n\n"
        );
    }

    #[test]
    fn a_sample_block_marks_a_raw_fallback_and_an_unchanged_text() {
        let sample = sample("question-02", "Tu viens ?", true);

        let fallback = text(|out| {
            write_sample(
                out,
                2,
                9,
                &sample,
                &outcome("Tu viens ?", 30_000, Err(ProviderError::Timeout)),
            )
        });
        let unchanged = text(|out| {
            write_sample(
                out,
                2,
                9,
                &sample,
                &outcome("Tu viens ?", 500, Ok(Finish::Complete)),
            )
        });

        assert!(fallback.starts_with("[2/9] question-02 | question | validation\n"));
        assert!(fallback.contains("  final:  [raw fallback] Tu viens ?\n"));
        assert!(fallback.contains("  chat:   30000 ms, error Timeout\n"));
        assert!(
            fallback.contains("  steps:  llm_refine Failed(Provider(Timeout)), trim Applied\n")
        );
        assert!(unchanged.contains("  final:  [unchanged] Tu viens ?\n"));
    }

    #[test]
    fn the_summary_gives_the_settings_the_times_the_fallbacks_and_the_marks() {
        let first = sample("question-01", "tu viens", false);
        let second = sample("question-02", "il pleut.", false);
        let third = sample("question-03", "on y va", false);
        let results = [
            (&first, outcome("Tu viens ?", 600, Ok(Finish::Complete))),
            (&second, outcome("il pleut.", 2_000, Ok(Finish::Truncated))),
            (&third, outcome("On y va.", 900, Ok(Finish::Complete))),
        ];
        let run = Run {
            set: Path::new("prompts/test-set-fr.toml"),
            split: Split::Dev,
            provider: "mistral",
            model: "mistral-small-latest",
            prompt_name: "natural-light",
            prompt_text: "a",
            limit: Duration::from_secs(30),
            pairs: 3,
        };

        let summary = text(|out| write_summary(out, &run, &results));

        assert_eq!(
            summary,
            "Summary\n\
             \x20 set:          prompts/test-set-fr.toml, 3 dev samples\n\
             \x20 provider:     mistral\n\
             \x20 model:        mistral-small-latest\n\
             \x20 prompt:       natural-light, FNV-1a 64 af63dc4c8601ec8c\n\
             \x20 chat limit:   30000 ms\n\
             \x20 chat time:    median 900 ms, max 2000 ms, 3 requests\n\
             \x20 fallbacks:    1\n\
             \x20               question-02: llm_refine Failed(Rejected(Incomplete))\n\
             \x20 punctuation:  , 0→0  . 0→1  ? 0→1  ! 0→0  : 0→0  ; 0→0  … 0→0  total 0→2, over 2 samples without fallback\n\
             \x20 dictionary:   3 pairs in the set, the correction does not apply them\n"
        );
    }
}
