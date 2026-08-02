use std::process::ExitCode;

use dev_launcher::cli::Request;
use dev_launcher::detect::{detect_all, ScanCtx};
use dev_launcher::resolve::ResolutionStatus;
use dev_launcher::scan::{resolve_roots, FileIndex, ScanOptions};

fn main() -> ExitCode {
    match run() {
        Ok(code) => exit_code(code),
        Err(error) => {
            eprintln!("dev: {error:#}");
            ExitCode::from(1)
        }
    }
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
    let Request::Resolve(request) = request;

    let roots = resolve_roots(&request.invocation.target);
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
    let resolution = dev_launcher::resolve::resolve(
        candidates,
        &request.invocation.hints,
        request.invocation.chaos,
        request.pick,
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
    let Some(candidate) = resolution.selected_candidate() else {
        eprint!("{}", dev_launcher::ui::error::candidate_table(&resolution));
        return Ok(resolution_exit_code(resolution.status));
    };
    if request.dry_run {
        #[cfg(unix)]
        println!(
            "{}",
            dev_launcher::ui::command_display::posix(candidate, &request.invocation.passthrough,)?
        );
        #[cfg(windows)]
        println!(
            "{}",
            dev_launcher::ui::command_display::powershell(
                candidate,
                &request.invocation.passthrough,
            )?
        );
        return Ok(0);
    }
    match dev_launcher::exec::execute(candidate, &request.invocation.passthrough, request.quiet) {
        Ok(code) => Ok(code),
        Err(error) => {
            eprintln!("dev: {error}");
            Ok(error.exit_code())
        }
    }
}

fn resolution_exit_code(status: ResolutionStatus) -> i32 {
    match status {
        ResolutionStatus::Resolved | ResolutionStatus::Remembered => 0,
        ResolutionStatus::NoCandidates => 4,
        ResolutionStatus::Ambiguous | ResolutionStatus::HintNoMatch => 5,
    }
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
