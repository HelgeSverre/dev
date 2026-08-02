#![cfg(unix)]

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use dev_launcher::candidate::{Availability, EvidenceKind};
use dev_launcher::detect::{detect_all, ScanCtx};
use dev_launcher::intent::{Intent, Invocation, Target};
use dev_launcher::resolve::Resolution;
use dev_launcher::scan::{resolve_roots, FileIndex, RootInfo, ScanOptions};

const GOLDEN: &str = include_str!("snapshots/corpus-structural.snap");

#[test]
fn structural_corpus_matches_golden_and_is_repeatable() -> anyhow::Result<()> {
    std::env::remove_var("VIRTUAL_ENV");
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus");
    let source_fixtures = fixture_directories(&corpus)?;
    assert!(
        source_fixtures.len() >= 50,
        "the specification requires at least 50 fixture repositories"
    );
    let materialized = tempfile::tempdir()?;
    let fixtures = materialize_fixtures(&corpus, &source_fixtures, materialized.path())?;

    let actual = render_corpus(&fixtures)?;
    let repeated = render_corpus(&fixtures)?;
    assert_eq!(actual.as_bytes(), repeated.as_bytes());
    if std::env::var_os("DEV_UPDATE_GOLDENS").as_deref() == Some(OsStr::new("1")) {
        fs::write(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/snapshots/corpus-structural.snap"),
            actual,
        )?;
        return Ok(());
    }
    assert_eq!(actual, GOLDEN);
    Ok(())
}

fn fixture_directories(corpus: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut fixtures = fs::read_dir(corpus)?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_type().ok()?.is_dir().then_some(entry.path()))
        .collect::<Vec<_>>();
    fixtures.sort();
    Ok(fixtures)
}

fn materialize_fixtures(
    corpus: &Path,
    sources: &[PathBuf],
    destination: &Path,
) -> anyhow::Result<Vec<(String, PathBuf)>> {
    sources
        .iter()
        .map(|source| {
            let name = source.strip_prefix(corpus)?.to_string_lossy().into_owned();
            let fixture = destination.join(&name);
            copy_directory(source, &fixture)?;
            fs::create_dir(fixture.join(".git"))?;
            Ok((name, fixture))
        })
        .collect()
}

fn copy_directory(source: &Path, destination: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target)?;
            fs::set_permissions(&target, entry.metadata()?.permissions())?;
        }
    }
    Ok(())
}

fn render_corpus(fixtures: &[(String, PathBuf)]) -> anyhow::Result<String> {
    let mut output = format!("schema=2 repositories={}\n", fixtures.len());
    for (name, fixture) in fixtures {
        for intent in [Intent::Run, Intent::Build, Intent::Test] {
            render_case(&mut output, name, fixture, intent)?;
        }
    }
    Ok(output)
}

fn render_case(
    output: &mut String,
    name: &str,
    fixture: &Path,
    intent: Intent,
) -> anyhow::Result<()> {
    let invocation = Invocation {
        intent,
        target: Target::Directory(fixture.to_path_buf()),
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
    let detection = detect_all(&context);
    let mut candidates =
        dev_launcher::dedupe::deduplicate(detection.candidates, &invocation.target);
    for candidate in &mut candidates {
        anyhow::ensure!(
            !candidate.action_key.is_empty()
                && !candidate.action_name.is_empty()
                && !candidate.label.is_empty()
                && !candidate.description.is_empty(),
            "{name}/{intent}: candidate metadata is incomplete for {}",
            candidate.action_key
        );
        anyhow::ensure!(
            !candidate.evidence.is_empty()
                && !candidate.search.identities.is_empty()
                && !candidate.search.target_paths.is_empty()
                && !candidate.search.scopes.is_empty()
                && !candidate.search.tags.is_empty(),
            "{name}/{intent}: candidate evidence/search document is incomplete for {}",
            candidate.action_key
        );
        candidate.availability = Availability::Available {
            resolved_program: PathBuf::from("<fixture-tool>"),
        };
        candidate
            .evidence
            .retain(|evidence| evidence.kind != EvidenceKind::Availability);
        dev_launcher::score::recompute(candidate);
    }
    let resolution = dev_launcher::resolve::resolve(candidates, &[], 0, false);

    writeln!(output, "\n## {name}/{intent}")?;
    writeln!(
        output,
        "roots package={} workspace={} scan={}",
        optional_path(roots.package_root.as_deref(), fixture),
        optional_path(roots.workspace_root.as_deref(), fixture),
        relative_path(&roots.scan_root, fixture)
    )?;
    writeln!(
        output,
        "scan structural={} targets={} truncated={}",
        index.structural.len(),
        index.targets.len(),
        !index.truncated.is_empty()
    )?;
    render_resolution(output, &resolution, &roots, fixture)?;
    for diagnostic in &detection.diagnostics {
        writeln!(
            output,
            "diagnostic detector={} severity={:?} source={} message={}",
            diagnostic.detector,
            diagnostic.severity,
            optional_path(diagnostic.source.as_deref(), fixture),
            normalize_text(&diagnostic.message, fixture)
        )?;
    }
    Ok(())
}

fn render_resolution(
    output: &mut String,
    resolution: &Resolution,
    _roots: &RootInfo,
    fixture: &Path,
) -> anyhow::Result<()> {
    let selected = resolution
        .selected_candidate()
        .map_or("-", |candidate| candidate.action_key.as_str());
    writeln!(
        output,
        "resolution status={:?} reason={:?} selected={selected}",
        resolution.status, resolution.reason
    )?;
    for ranked in &resolution.candidates {
        let candidate = &ranked.candidate;
        let args = candidate
            .args
            .iter()
            .map(|argument| format!("{:?}", argument.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(",");
        writeln!(
            output,
            "candidate action={} detector={} source={} layer={:?} origin={:?} policy={:?} base={} total={} distance={} cwd={} scope={} program={:?} args=[{}] availability=available",
            candidate.action_key,
            candidate.detector,
            candidate.source,
            candidate.layer,
            candidate.origin,
            candidate.selection,
            candidate.base_points,
            candidate.structural_points,
            candidate.anchor_distance,
            relative_path(&candidate.cwd, fixture),
            relative_path(&candidate.scope_root, fixture),
            normalize_text(&candidate.program.to_string_lossy(), fixture),
            args
        )?;
        for evidence in &candidate.evidence {
            writeln!(
                output,
                "  evidence kind={:?} points={} source={} reason={}",
                evidence.kind,
                evidence.points,
                optional_path(evidence.source.as_deref(), fixture),
                normalize_text(&evidence.reason, fixture)
            )?;
        }
    }
    Ok(())
}

fn optional_path(path: Option<&Path>, fixture: &Path) -> String {
    path.map_or_else(|| "-".to_owned(), |path| relative_path(path, fixture))
}

fn relative_path(path: &Path, fixture: &Path) -> String {
    let value = path.strip_prefix(fixture).unwrap_or(path);
    if value.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        value.to_string_lossy().replace('\\', "/")
    }
}

fn normalize_text(value: &str, fixture: &Path) -> String {
    value.replace(&fixture.to_string_lossy().into_owned(), "<fixture>")
}
