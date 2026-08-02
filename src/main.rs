use std::io::{self, IsTerminal, Write};

use dev_launcher::cache::{CacheLookup, QueryCacheKey};
use dev_launcher::candidate::{Availability, Candidate};
use dev_launcher::cli::{CacheRequest, ColorMode, Request, ResolveRequest};
use dev_launcher::detect::{detect_all, ScanCtx};
use dev_launcher::query::{MatchClass, TermMatch};
use dev_launcher::resolve::{RankedCandidate, Resolution, ResolutionReason, ResolutionStatus};
use dev_launcher::scan::{resolve_roots, FileIndex, ScanOptions};
use dev_launcher::ui::picker::PickerOutcome;

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dev: {error:#}");
            1
        }
    };
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
    std::process::exit(code);
}

fn run() -> anyhow::Result<i32> {
    let current_directory = std::env::current_dir()?;
    let request = match dev_launcher::cli::parse_from(std::env::args_os(), &current_directory) {
        Ok(request) => request,
        Err(dev_launcher::cli::CliError::Clap(error)) => {
            let code = error.exit_code();
            error.print()?;
            return Ok(code);
        }
        Err(error) => {
            eprintln!("dev: {error}");
            return Ok(2);
        }
    };
    match request {
        Request::Resolve(request) => run_resolution(request),
        Request::Cache(request) => run_cache(request),
        Request::Doctor => run_doctor(),
    }
}

fn run_doctor() -> anyhow::Result<i32> {
    let current_directory = std::env::current_dir()?;
    let reports = dev_launcher::doctor::inspect(&current_directory);
    print!("{}", dev_launcher::doctor::render(&reports));
    Ok(0)
}

fn run_resolution(request: ResolveRequest) -> anyhow::Result<i32> {
    let roots = resolve_roots(&request.invocation.target);
    if request.forget {
        match dev_launcher::cache::forget(&request.invocation, &roots) {
            Ok(true) if request.verbose => eprintln!("dev: forgot the active remembered choice"),
            Ok(_) => {}
            Err(error) => eprintln!("dev: warning: could not forget choice: {error}"),
        }
    }
    let cache_lookup = if request.no_cache || request.forget || request.pick {
        CacheLookup::Missing
    } else {
        dev_launcher::cache::lookup(&request.invocation, &roots)
    };
    if fast_cache_allowed(&request) {
        if let CacheLookup::Valid(entry) = &cache_lookup {
            if let Some(candidate) = entry.candidate(&request.invocation.target) {
                if entry.needs_touch() {
                    if let Err(error) = dev_launcher::cache::touch(&entry.key) {
                        eprintln!("dev: warning: could not refresh remembered choice: {error}");
                    }
                }
                return execute_candidate(&candidate, &request, None);
            }
        }
    }

    let mut index = FileIndex::build(
        &roots,
        ScanOptions {
            structural_depth: request.depth,
            hard_cap: 20_000,
        },
    );
    if !request.invocation.hints.is_empty() || request.pick {
        index.build_targets(&roots, request.invocation.chaos, 20_000);
    }
    let context = ScanCtx {
        invocation: &request.invocation,
        roots: &roots,
        index: &index,
    };
    let detection = detect_all(&context);
    let candidates =
        dev_launcher::dedupe::deduplicate(detection.candidates, &request.invocation.target);
    let mut resolution = dev_launcher::resolve::resolve(
        candidates,
        &request.invocation.hints,
        request.invocation.chaos,
        request.pick,
    );
    let remembered_after_project_change = apply_remembered_choice(
        &mut resolution,
        &cache_lookup,
        request.pick,
        &request.invocation.target,
    );

    if request.json {
        println!(
            "{}",
            dev_launcher::ui::json::render(
                &request.invocation,
                &roots,
                &index,
                &resolution,
                &detection.diagnostics,
            )?
        );
        return Ok(resolution_exit_code(resolution.status));
    }
    if remembered_after_project_change {
        eprintln!("dev: project changed; remembered action still exists");
    }
    if request.why {
        print!(
            "{}",
            dev_launcher::ui::why::render(&resolution, &roots, &index, &detection.diagnostics,)
        );
        return Ok(0);
    }
    if request.list {
        let output = dev_launcher::ui::why::list(&resolution);
        if !output.is_empty() {
            println!("{output}");
        }
        return Ok(0);
    }
    if request.verbose {
        eprint!(
            "{}",
            dev_launcher::ui::why::render(&resolution, &roots, &index, &detection.diagnostics,)
        );
    }

    let choice = choose_candidate(&resolution, &request, !index.truncated.is_empty())?;
    let Some(choice) = choice else {
        eprint!(
            "{}",
            dev_launcher::ui::error::candidate_table(
                &resolution,
                &request.invocation,
                &roots,
                &index,
                &detection.diagnostics,
            )
        );
        return Ok(resolution_exit_code(resolution.status));
    };
    let (index_in_resolution, remember, print_only) = match choice {
        PickerOutcome::Run { index, remember } => (index, remember, request.dry_run),
        PickerOutcome::Print { index } => (index, false, true),
        PickerOutcome::Cancel => return Ok(130),
    };
    let candidate = &resolution.candidates[index_in_resolution].candidate;
    if !candidate.availability.is_available() {
        eprintln!("dev: {}", availability_message(&candidate.availability));
        return Ok(6);
    }
    if remembered_after_project_change {
        if let Err(error) =
            dev_launcher::cache::refresh(&request.invocation, &roots, &index, candidate)
        {
            eprintln!("dev: warning: could not refresh remembered choice: {error}");
        }
    }
    if remember {
        if request.no_cache {
            eprintln!("dev: warning: --no-cache prevents remembering this choice");
        } else if let Err(error) =
            dev_launcher::cache::remember(&request.invocation, &roots, &index, candidate)
        {
            eprintln!("dev: warning: could not remember choice: {error}");
        }
    }
    if print_only {
        print_shell_command(candidate, &request.invocation.passthrough)?;
        return Ok(0);
    }
    let decisive = (!request.invocation.hints.is_empty())
        .then(|| decisive_match(&resolution.candidates[index_in_resolution]))
        .flatten();
    execute_candidate(candidate, &request, decisive)
}

fn fast_cache_allowed(request: &ResolveRequest) -> bool {
    !request.no_cache
        && !request.forget
        && !request.pick
        && !request.json
        && !request.why
        && !request.list
        && !request.dry_run
        && !request.verbose
}

fn apply_remembered_choice(
    resolution: &mut Resolution,
    lookup: &CacheLookup,
    forced_picker: bool,
    target: &dev_launcher::intent::Target,
) -> bool {
    if forced_picker {
        return false;
    }
    let entry = match lookup {
        CacheLookup::Valid(entry) | CacheLookup::Stale(entry) => entry,
        CacheLookup::Missing => return false,
    };
    if let Some(index) = resolution
        .candidates
        .iter()
        .position(|ranked| ranked.candidate.id == entry.candidate_id)
    {
        resolution.status = ResolutionStatus::Remembered;
        resolution.reason = ResolutionReason::RememberedChoice;
        resolution.selected = Some(index);
        matches!(lookup, CacheLookup::Stale(_))
    } else if resolution
        .candidates
        .iter()
        .any(|ranked| ranked.candidate.action_key == entry.action_key)
    {
        resolution.status = ResolutionStatus::Ambiguous;
        resolution.reason = ResolutionReason::RememberedCommandChanged;
        resolution.selected = None;
        false
    } else {
        resolution.status = ResolutionStatus::Ambiguous;
        resolution.reason = ResolutionReason::RememberedActionMissing;
        resolution.selected = None;
        if let Some(mut previous) = entry.candidate(target) {
            previous.availability = Availability::UnsupportedHost {
                reason: "remembered action is no longer declared by the project".to_owned(),
            };
            previous.selection = dev_launcher::candidate::SelectionPolicy::Confirm;
            resolution
                .candidates
                .push(dev_launcher::resolve::RankedCandidate {
                    candidate: previous,
                    query: dev_launcher::query::QueryMatch::default(),
                    finalist: false,
                });
        }
        false
    }
}

fn choose_candidate(
    resolution: &Resolution,
    request: &ResolveRequest,
    scan_truncated: bool,
) -> anyhow::Result<Option<PickerOutcome>> {
    if let Some(index) = resolution.selected {
        return Ok(Some(PickerOutcome::Run {
            index,
            remember: false,
        }));
    }
    match dev_launcher::ui::picker::pick(
        resolution,
        &request.invocation.hints,
        request.invocation.chaos,
        scan_truncated,
        colors_enabled(request.color),
    ) {
        Ok(outcome) => Ok(Some(outcome)),
        Err(dev_launcher::ui::picker::PickerError::NotInteractive) => Ok(None),
        Err(error) => {
            eprintln!("dev: picker unavailable: {error}");
            Ok(None)
        }
    }
}

fn execute_candidate(
    candidate: &Candidate,
    request: &ResolveRequest,
    decisive_match: Option<&TermMatch>,
) -> anyhow::Result<i32> {
    let options = dev_launcher::exec::ExecutionOptions {
        quiet: request.quiet,
        colors: colors_enabled(request.color),
        decisive_match,
    };
    match dev_launcher::exec::execute(candidate, &request.invocation.passthrough, options) {
        Ok(code) => Ok(code),
        Err(error) => {
            eprintln!("dev: {error}");
            Ok(error.exit_code())
        }
    }
}

fn decisive_match(ranked: &RankedCandidate) -> Option<&TermMatch> {
    ranked.query.terms.iter().max_by(|left, right| {
        (left.class == MatchClass::Identity)
            .cmp(&(right.class == MatchClass::Identity))
            .then_with(|| left.class.cmp(&right.class))
            .then_with(|| left.quality_millis.cmp(&right.quality_millis))
            .then_with(|| left.points.cmp(&right.points))
    })
}

fn colors_enabled(mode: ColorMode) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => io::stderr().is_terminal(),
    }
}

fn print_shell_command(
    candidate: &Candidate,
    passthrough: &[std::ffi::OsString],
) -> anyhow::Result<()> {
    #[cfg(unix)]
    println!(
        "{}",
        dev_launcher::ui::command_display::posix(candidate, passthrough)?
    );
    #[cfg(windows)]
    println!(
        "{}",
        dev_launcher::ui::command_display::powershell(candidate, passthrough)?
    );
    Ok(())
}

fn availability_message(availability: &Availability) -> String {
    match availability {
        Availability::Available { .. } => "candidate is available".to_owned(),
        Availability::MissingProgram { program } => format!(
            "selected program `{}` is not available",
            program.to_string_lossy()
        ),
        Availability::UnsupportedHost { reason } => reason.clone(),
    }
}

fn run_cache(request: CacheRequest) -> anyhow::Result<i32> {
    match request {
        CacheRequest::List => {
            for entry in dev_launcher::cache::list()? {
                let query = match &entry.key.query {
                    QueryCacheKey::Unhinted => "—",
                    QueryCacheKey::Hinted(_) => "hinted",
                };
                println!(
                    "{}  {:<5}  {:<6}  {:<8}  {:<8}  {}",
                    entry.key.physical_anchor.display(),
                    entry.key.intent,
                    query,
                    format_age(entry.age()),
                    if entry.is_shape_valid() {
                        "current"
                    } else {
                        "stale"
                    },
                    entry.action_key
                );
            }
            Ok(0)
        }
        CacheRequest::Clear { yes } => {
            if !yes {
                if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
                    eprintln!("dev: cache clear requires a TTY or --yes");
                    return Ok(2);
                }
                eprint!("Clear all remembered dev choices? [y/N] ");
                io::stderr().flush()?;
                let mut answer = String::new();
                io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
                    return Ok(0);
                }
            }
            let count = dev_launcher::cache::clear()?;
            eprintln!("dev: cleared {count} remembered choice(s)");
            Ok(0)
        }
    }
}

fn format_age(age: std::time::Duration) -> String {
    let seconds = age.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 60 * 60 {
        format!("{}m", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{}h", seconds / (60 * 60))
    } else {
        format!("{}d", seconds / (24 * 60 * 60))
    }
}

fn resolution_exit_code(status: ResolutionStatus) -> i32 {
    match status {
        ResolutionStatus::Resolved | ResolutionStatus::Remembered => 0,
        ResolutionStatus::NoCandidates => 4,
        ResolutionStatus::Ambiguous | ResolutionStatus::HintNoMatch => 5,
    }
}
