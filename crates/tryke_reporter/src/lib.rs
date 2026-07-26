pub mod clear;
pub mod diagnostic;
pub mod dot;
pub mod duration;
pub mod json;
pub mod junit;
pub mod live;
pub mod llm;
pub mod next;
#[cfg(feature = "terminal")]
pub mod progress;
pub mod reporter;
pub mod sugar;
pub mod summary;
pub mod text;

pub use dot::DotReporter;
pub use json::JSONReporter;
pub use junit::JUnitReporter;
pub use llm::LlmReporter;
pub use next::NextReporter;
#[cfg(feature = "terminal")]
pub use progress::ProgressReporter;
pub use reporter::Reporter;
pub use sugar::SugarReporter;
pub use text::{TextReporter, Verbosity};

/// Reporter implementation selected by the CLI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReporterKind {
    Text,
    Json,
    Dot,
    Junit,
    Llm,
    Next,
    Sugar,
}

/// Build the selected reporter and apply terminal progress behavior.
#[cfg(feature = "terminal")]
#[must_use]
pub fn build_reporter(
    kind: ReporterKind,
    verbosity: Verbosity,
    no_progress: bool,
) -> Box<dyn Reporter> {
    // Next and Sugar render their own progress UI, so native terminal
    // progress is only layered over the non-live text and dot reporters.
    let use_progress = !no_progress
        && progress::supports_progress()
        && matches!(kind, ReporterKind::Text | ReporterKind::Dot);

    match kind {
        ReporterKind::Text if use_progress => Box::new(ProgressReporter::new(
            TextReporter::with_verbosity(verbosity),
        )),
        ReporterKind::Text => Box::new(TextReporter::with_verbosity(verbosity)),
        ReporterKind::Dot if use_progress => Box::new(ProgressReporter::new(DotReporter::new())),
        ReporterKind::Dot => Box::new(DotReporter::new()),
        ReporterKind::Next => Box::new(NextReporter::new()),
        ReporterKind::Sugar => Box::new(SugarReporter::new()),
        ReporterKind::Json => Box::new(JSONReporter::new()),
        ReporterKind::Junit => Box::new(JUnitReporter::new()),
        ReporterKind::Llm => Box::new(LlmReporter::new()),
    }
}

#[cfg(all(test, feature = "terminal"))]
mod tests {
    use super::*;

    #[test]
    fn builds_every_reporter_kind() {
        for kind in [
            ReporterKind::Text,
            ReporterKind::Json,
            ReporterKind::Dot,
            ReporterKind::Junit,
            ReporterKind::Llm,
            ReporterKind::Next,
            ReporterKind::Sugar,
        ] {
            drop(build_reporter(kind, Verbosity::Normal, true));
        }
    }
}
