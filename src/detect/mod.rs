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
pub(crate) use node::NodeTestBinder;
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
    fn detect(&self, context: &ScanCtx<'_>) -> Detection;
}

/// Run the static detector registry.
#[must_use]
pub fn detect_all(context: &ScanCtx<'_>) -> Detection {
    let detectors = crate::registry::registrations()
        .iter()
        .map(|registration| registration.detector)
        .collect::<Vec<_>>();
    detect_with_registry(context, &detectors)
}

fn detect_with_registry(context: &ScanCtx<'_>, detectors: &[&dyn Detector]) -> Detection {
    let mut output = Detection::default();
    for detector in detectors {
        output.append(detector.detect(context));
    }
    output.candidates = target::expand(output.candidates, context);
    output.candidates.sort_by(|left, right| {
        left.action_key
            .cmp(&right.action_key)
            .then_with(|| left.detector.cmp(&right.detector))
            .then_with(|| left.source.cmp(&right.source))
    });
    output.diagnostics.sort_by(|left, right| {
        left.detector
            .cmp(&right.detector)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.message.cmp(&right.message))
    });
    output
}

#[cfg(test)]
mod tests {
    use crate::dedupe::deduplicate;
    use crate::intent::{Intent, Invocation, Target};
    use crate::scan::{resolve_roots, FileIndex, ScanOptions};

    use super::*;

    #[test]
    fn detector_registration_order_does_not_change_results() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"name":"web","scripts":{"dev":"vite"},"dependencies":{"vite":"1"}}"#,
        )?;
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"web\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::create_dir(temp.path().join("src"))?;
        std::fs::write(temp.path().join("src/main.rs"), "fn main() {}\n")?;
        let invocation = Invocation {
            intent: Intent::Run,
            target: Target::Directory(temp.path().to_path_buf()),
            hints: Vec::new(),
            passthrough: Vec::new(),
            chaos: 0,
        };
        let roots = resolve_roots(&invocation.target);
        let index = FileIndex::build(&roots, ScanOptions::default());
        let context = ScanCtx {
            invocation: &invocation,
            roots: &roots,
            index: &index,
        };
        let forward = crate::registry::registrations()
            .iter()
            .map(|registration| registration.detector)
            .collect::<Vec<_>>();
        let mut reverse = forward.clone();
        reverse.reverse();
        let summarize = |detection: Detection| {
            deduplicate(detection.candidates, &invocation.target)
                .into_iter()
                .map(|candidate| {
                    (
                        candidate.id,
                        candidate.action_key,
                        candidate.detector,
                        candidate.structural_points,
                        candidate.evidence,
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            summarize(detect_with_registry(&context, &forward)),
            summarize(detect_with_registry(&context, &reverse))
        );
        Ok(())
    }
}
