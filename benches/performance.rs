use std::ffi::OsString;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use dev_launcher::cache::CacheLookup;
use dev_launcher::candidate::{Candidate, SearchDocument, SelectionPolicy};
use dev_launcher::intent::{Intent, Invocation, Target};
use dev_launcher::query::{match_candidate, normalize_query};
use dev_launcher::registry::{NODE, NODE_SOURCE};
use dev_launcher::scan::{resolve_roots, FileIndex, ScanOptions};

const FILE_COUNT: usize = 10_000;
const FILE_GROUPS: usize = 100;

struct Measurement {
    name: String,
    samples: usize,
    p50: Duration,
    p95: Duration,
    p95_budget: Duration,
}

fn main() -> anyhow::Result<()> {
    let generated = tempfile::tempdir()?;
    let repository = generated.path().join("repository");
    generate_repository(&repository)?;

    let mut measurements = vec![
        benchmark_remembered_hit(generated.path())?,
        measure(
            "scan/structural-10k",
            21,
            Duration::from_millis(500),
            || {
                let roots = roots(&repository);
                black_box(FileIndex::build(&roots, ScanOptions::default()));
            },
        ),
        measure("scan/chaos-1", 21, Duration::from_millis(750), || {
            let roots = roots(&repository);
            let mut index = FileIndex::build(&roots, ScanOptions::default());
            index.build_targets(&roots, 1, 20_000);
            black_box(index.targets.len());
        }),
        measure(
            "scan/chaos-2-capped",
            21,
            Duration::from_millis(1_500),
            || {
                let roots = roots(&repository);
                let mut index = FileIndex::build(&roots, ScanOptions::default());
                index.build_targets(&roots, 2, 5_000);
                black_box(index.truncated.len());
            },
        ),
        benchmark_hinted_cli(&repository, 1, Duration::from_millis(500))?,
        benchmark_hinted_cli(&repository, 2, Duration::from_millis(750))?,
    ];
    measurements.extend(benchmark_query_scaling());
    measurements.push(benchmark_startup()?);

    println!("benchmark                         samples      p50       p95    p95 ceiling");
    println!("-------------------------------- ------- --------- --------- --------------");
    let mut failed = false;
    for measurement in &measurements {
        println!(
            "{:<32} {:>7} {:>8.3}ms {:>8.3}ms {:>11.3}ms",
            measurement.name,
            measurement.samples,
            milliseconds(measurement.p50),
            milliseconds(measurement.p95),
            milliseconds(measurement.p95_budget),
        );
        if measurement.p95 > measurement.p95_budget {
            failed = true;
            eprintln!(
                "performance regression: {} p95 {:.3}ms exceeds {:.3}ms",
                measurement.name,
                milliseconds(measurement.p95),
                milliseconds(measurement.p95_budget)
            );
        }
    }
    anyhow::ensure!(!failed, "one or more performance ceilings were exceeded");
    Ok(())
}

fn benchmark_remembered_hit(root: &Path) -> anyhow::Result<Measurement> {
    let state = root.join("state");
    std::env::set_var("XDG_STATE_HOME", &state);
    let project = root.join("remembered");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"vite"}}"#,
    )?;
    let invocation = Invocation {
        intent: Intent::Run,
        target: Target::Directory(project.clone()),
        hints: Vec::new(),
        passthrough: Vec::new(),
        chaos: 0,
    };
    let roots = resolve_roots(&invocation.target);
    let index = FileIndex::build(&roots, ScanOptions::default());
    let mut candidate = Candidate::new(
        "bench:remembered",
        NODE,
        NODE_SOURCE,
        Intent::Run,
        "dev",
        "sh",
        Vec::new(),
        project,
        95,
        SelectionPolicy::Automatic,
    );
    candidate.label = "remembered benchmark".to_owned();
    candidate.refresh_id();
    dev_launcher::score::finalize(&mut candidate, &invocation.target);
    dev_launcher::cache::remember(&invocation, &roots, &index, &candidate)?;

    let measurement = measure(
        "cache/exact-remembered-hit",
        101,
        Duration::from_millis(100),
        || match dev_launcher::cache::lookup(&invocation, &roots) {
            CacheLookup::Valid(entry) => {
                black_box(entry.candidate(&invocation.target));
            }
            other => panic!("expected valid cache entry, got {other:?}"),
        },
    );
    Ok(measurement)
}

fn benchmark_query_scaling() -> Vec<Measurement> {
    [10, 100, 1_000, 10_000]
        .into_iter()
        .map(|count| {
            let candidates = (0..count)
                .map(|index| query_candidate(index, count))
                .collect::<Vec<_>>();
            let query = normalize_query(&[format!("participant-{}", count - 1)]);
            let samples = if count < 1_000 { 101 } else { 31 };
            let budget = match count {
                10 => Duration::from_millis(5),
                100 => Duration::from_millis(20),
                1_000 => Duration::from_millis(150),
                _ => Duration::from_millis(1_500),
            };
            measure(
                &format!("query/{count}-candidates"),
                samples,
                budget,
                || {
                    let total = candidates
                        .iter()
                        .map(|candidate| match_candidate(candidate, &query, 1).total_points)
                        .sum::<i32>();
                    black_box(total);
                },
            )
        })
        .collect()
}

fn benchmark_startup() -> anyhow::Result<Measurement> {
    let executable = release_binary()?;
    let measurement = measure("startup/minimal", 31, Duration::from_millis(250), || {
        let status = Command::new(&executable)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("release binary should start");
        assert!(status.success());
    });
    Ok(measurement)
}

fn benchmark_hinted_cli(
    root: &Path,
    chaos: u8,
    p95_budget: Duration,
) -> anyhow::Result<Measurement> {
    let executable = release_binary()?;
    let chaos = chaos.to_string();
    Ok(measure(
        &format!("cli/hinted-chaos-{chaos}-10k"),
        11,
        p95_budget,
        || {
            let status = Command::new(&executable)
                .args(["run", "--list", "--no-cache", "--chaos"])
                .arg(&chaos)
                .arg("-C")
                .arg(root)
                .arg("file-099")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("release binary should complete hinted discovery");
            assert!(status.success());
        },
    ))
}

fn release_binary() -> anyhow::Result<PathBuf> {
    let current = std::env::current_exe()?;
    let release = current
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("benchmark executable has no release directory"))?;
    let executable = release.join(format!("dev{}", std::env::consts::EXE_SUFFIX));
    anyhow::ensure!(
        executable.is_file(),
        "build the release dev binary before running the benchmark"
    );
    Ok(executable)
}

fn query_candidate(index: usize, count: usize) -> Candidate {
    let identity = format!("participant-{index}");
    let mut candidate = Candidate::new(
        format!("bench:query:{index}"),
        NODE,
        NODE_SOURCE,
        Intent::Run,
        &identity,
        "tool",
        vec![OsString::from(format!("scope-{}", index % 100))],
        PathBuf::from(format!("/fixture/member-{}", index % count.max(1))),
        15,
        SelectionPolicy::ExplicitHint,
    );
    candidate.search = SearchDocument {
        identities: vec![identity],
        scopes: vec![format!("member-{}", index % 100)],
        tags: vec!["node".to_owned(), "test".to_owned()],
        text: vec!["generated query benchmark candidate".to_owned()],
        ..SearchDocument::default()
    };
    candidate
}

fn roots(repository: &Path) -> dev_launcher::scan::RootInfo {
    resolve_roots(&Target::Directory(repository.to_path_buf()))
}

fn generate_repository(root: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(root.join(".git"))?;
    fs::write(
        root.join("package.json"),
        r#"{"name":"benchmark","scripts":{"dev":"vite","test":"vitest"}}"#,
    )?;
    for directory in 0..FILE_GROUPS {
        let path = root.join("data").join(format!("group-{directory:03}"));
        fs::create_dir_all(&path)?;
        for file in 0..(FILE_COUNT / FILE_GROUPS) {
            fs::write(path.join(format!("file-{file:03}.txt")), b"fixture\n")?;
        }
    }
    let tests = root.join("tests/integration");
    fs::create_dir_all(&tests)?;
    for file in 0..250 {
        fs::write(tests.join(format!("case-{file:03}.test.js")), b"test\n")?;
    }
    Ok(())
}

fn measure(
    name: &str,
    samples: usize,
    p95_budget: Duration,
    mut operation: impl FnMut(),
) -> Measurement {
    operation();
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        operation();
        durations.push(started.elapsed());
    }
    durations.sort_unstable();
    Measurement {
        name: name.to_owned(),
        samples,
        p50: percentile(&durations, 50),
        p95: percentile(&durations, 95),
        p95_budget,
    }
}

fn percentile(values: &[Duration], percentile: usize) -> Duration {
    let index = (values.len() - 1) * percentile / 100;
    values[index]
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
