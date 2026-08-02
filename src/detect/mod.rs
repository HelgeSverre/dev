mod cargo;
mod node;

use crate::candidate::Candidate;
use crate::diagnostic::Diagnostic;
use crate::intent::Invocation;
use crate::scan::{FileIndex, RootInfo};

pub use cargo::CargoDetector;
pub use node::NodeDetector;

/// Read-only context shared by all detectors.
#[derive(Debug)]
pub struct ScanCtx<'a> {
    pub invocation: &'a Invocation,
    pub roots: &'a RootInfo,
    pub index: &'a FileIndex,
}

#[derive(Debug, Default)]
pub struct Detection {
    pub candidates: Vec<Candidate>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Detection {
    fn append(&mut self, mut other: Self) {
        self.candidates.append(&mut other.candidates);
        self.diagnostics.append(&mut other.diagnostics);
    }
}

/// A deterministic, data-only project command detector.
pub trait Detector: Send + Sync {
    fn name(&self) -> &'static str;
    fn synonyms(&self) -> &'static [&'static str];
    fn detect(&self, context: &ScanCtx<'_>) -> Detection;
}

/// Run the static M1 detector registry.
#[must_use]
pub fn detect_all(context: &ScanCtx<'_>) -> Detection {
    let detectors: [&dyn Detector; 2] = [&NodeDetector, &CargoDetector];
    let mut output = Detection::default();
    for detector in detectors {
        output.append(detector.detect(context));
    }
    output.candidates.sort_by(|left, right| {
        left.action_key
            .cmp(&right.action_key)
            .then_with(|| left.detector.cmp(right.detector))
    });
    output.diagnostics.sort_by(|left, right| {
        left.detector
            .cmp(right.detector)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.message.cmp(&right.message))
    });
    output
}
