mod artisan;
mod cargo;
mod composer;
mod dart;
mod docker;
mod go;
mod make;
mod node;
mod php_file;
mod python_file;
mod script;
mod shell;
mod swift;
mod target;
mod zig;

use crate::candidate::Candidate;
use crate::diagnostic::Diagnostic;
use crate::intent::Invocation;
use crate::scan::{FileIndex, RootInfo};

pub use artisan::ArtisanDetector;
pub use cargo::CargoDetector;
pub use composer::ComposerDetector;
pub use dart::DartDetector;
pub use docker::DockerDetector;
pub use go::GoDetector;
pub use make::MakeDetector;
pub use node::NodeDetector;
pub use php_file::PhpFileDetector;
pub use python_file::PythonFileDetector;
pub use shell::ShellDetector;
pub use swift::SwiftDetector;
pub use target::{TargetBinder, TargetRunner};
pub use zig::ZigDetector;

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

/// Run the static detector registry.
#[must_use]
pub fn detect_all(context: &ScanCtx<'_>) -> Detection {
    let detectors: [&dyn Detector; 13] = [
        &NodeDetector,
        &CargoDetector,
        &ComposerDetector,
        &ArtisanDetector,
        &GoDetector,
        &PhpFileDetector,
        &ZigDetector,
        &SwiftDetector,
        &DartDetector,
        &PythonFileDetector,
        &ShellDetector,
        &MakeDetector,
        &DockerDetector,
    ];
    let mut output = Detection::default();
    for detector in detectors {
        output.append(detector.detect(context));
    }
    output.candidates = target::expand(output.candidates, context);
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
