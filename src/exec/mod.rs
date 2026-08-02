use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use crate::candidate::{Availability, Candidate};
use crate::query::TermMatch;
use crate::ui::command_display;

#[cfg(windows)]
mod windows;

#[derive(Copy, Clone, Debug)]
pub struct ExecutionOptions<'a> {
    pub quiet: bool,
    pub colors: bool,
    pub decisive_match: Option<&'a TermMatch>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("selected candidate is unavailable: {0}")]
    Unavailable(String),
    #[error("refusing to recursively execute dev through `{0}`")]
    Recursive(String),
    #[error("failed to execute `{program}`: {source}")]
    Start {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

impl ExecutionError {
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Unavailable(_) => 6,
            Self::Recursive(_) | Self::Start { .. } => 1,
        }
    }
}

/// Execute the candidate with inherited stdio and exact argv semantics.
pub fn execute(
    candidate: &Candidate,
    passthrough: &[OsString],
    options: ExecutionOptions<'_>,
) -> Result<i32, ExecutionError> {
    let resolved_program = match &candidate.availability {
        Availability::Available { resolved_program } => resolved_program,
        Availability::MissingProgram { program } => {
            return Err(ExecutionError::Unavailable(format!(
                "program `{}` was not found",
                program.to_string_lossy()
            )));
        }
        Availability::UnsupportedHost { reason } => {
            return Err(ExecutionError::Unavailable(reason.clone()));
        }
    };
    check_recursion(resolved_program)?;

    if !options.quiet {
        let command = command_display::diagnostic(candidate, passthrough);
        if options.colors {
            eprintln!(
                "\x1b[36m›\x1b[0m {command}  ({}, {})",
                candidate.detector,
                candidate.cwd.display()
            );
        } else {
            eprintln!(
                "› {command}  ({}, {})",
                candidate.detector,
                candidate.cwd.display()
            );
        }
        if let Some(matched) = options.decisive_match {
            let detail = format!(
                "  matched: {:?} -> {:?}",
                matched.hint, matched.candidate_value
            );
            if options.colors {
                eprintln!("\x1b[2m{detail}\x1b[0m");
            } else {
                eprintln!("{detail}");
            }
        }
        let _ = std::io::stderr().flush();
    }
    #[cfg(windows)]
    let mut command = Command::new(resolved_program);
    #[cfg(not(windows))]
    let mut command = Command::new(&candidate.program);
    command
        .args(candidate.command_with_passthrough(passthrough))
        .current_dir(&candidate.cwd)
        .envs(&candidate.env);
    #[cfg(unix)]
    command.env("PWD", &candidate.cwd);
    execute_command(command, &candidate.program.to_string_lossy())
}

fn check_recursion(candidate_program: &Path) -> Result<(), ExecutionError> {
    let Ok(current) = std::env::current_exe() else {
        return Ok(());
    };
    if same_file(&current, candidate_program) {
        return Err(ExecutionError::Recursive(
            candidate_program.display().to_string(),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (left.metadata(), right.metadata()) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => match (left.canonicalize(), right.canonicalize()) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        },
    }
}

#[cfg(not(unix))]
fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(unix)]
fn execute_command(mut command: Command, program: &str) -> Result<i32, ExecutionError> {
    use std::os::unix::process::CommandExt;
    let source = command.exec();
    Err(ExecutionError::Start {
        program: program.to_owned(),
        source,
    })
}

#[cfg(windows)]
fn execute_command(command: Command, program: &str) -> Result<i32, ExecutionError> {
    windows::execute_command(command).map_err(|source| ExecutionError::Start {
        program: program.to_owned(),
        source,
    })
}

#[cfg(not(any(unix, windows)))]
fn execute_command(mut command: Command, program: &str) -> Result<i32, ExecutionError> {
    let status = command.status().map_err(|source| ExecutionError::Start {
        program: program.to_owned(),
        source,
    })?;
    Ok(status.code().unwrap_or(1))
}
