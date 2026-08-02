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

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use crate::candidate::{Availability, Candidate, SelectionPolicy};
    use crate::intent::Intent;
    use crate::registry::{SHELL, SHELL_SOURCE};

    use super::{execute, ExecutionOptions};

    #[test]
    fn exec_environment_helper() {
        if std::env::var_os("DEV_EXEC_TEST_HELPER").is_none() {
            return;
        }
        let program = PathBuf::from(
            std::env::var_os("DEV_EXEC_TEST_PROGRAM").expect("helper program path must be set"),
        );
        let cwd =
            PathBuf::from(std::env::var_os("DEV_EXEC_TEST_CWD").expect("helper cwd must be set"));
        let output =
            std::env::var_os("DEV_EXEC_TEST_OUTPUT").expect("helper output path must be set");
        let mut candidate = Candidate::new(
            "test:exec-environment",
            SHELL,
            SHELL_SOURCE,
            Intent::Run,
            "exec-environment",
            program.as_os_str(),
            Vec::new(),
            cwd,
            95,
            SelectionPolicy::Automatic,
        );
        candidate.env = BTreeMap::from([
            (
                OsString::from("DEV_DECLARED_DELTA"),
                OsString::from("exact value"),
            ),
            (OsString::from("DEV_EXEC_OUTPUT"), output),
        ]);
        candidate.availability = Availability::Available {
            resolved_program: program,
        };
        let result = execute(
            &candidate,
            &[],
            ExecutionOptions {
                quiet: true,
                colors: false,
                decisive_match: None,
            },
        );
        panic!("exec returned instead of replacing the helper: {result:?}");
    }

    #[test]
    fn exec_applies_declared_environment_deltas_and_working_directory() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let cwd = temp.path().join("working directory");
        let program = temp.path().join("probe");
        let output = temp.path().join("observed");
        std::fs::create_dir(&cwd)?;
        std::fs::write(
            &program,
            "#!/bin/sh\nprintf 'cwd=<%s>\\ndelta=<%s>\\n' \"$PWD\" \"$DEV_DECLARED_DELTA\" > \"$DEV_EXEC_OUTPUT\"\n",
        )?;
        let mut permissions = program.metadata()?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&program, permissions)?;

        let status = std::process::Command::new(std::env::current_exe()?)
            .args(["--exact", "exec::tests::exec_environment_helper"])
            .env("DEV_EXEC_TEST_HELPER", "1")
            .env("DEV_EXEC_TEST_PROGRAM", &program)
            .env("DEV_EXEC_TEST_CWD", &cwd)
            .env("DEV_EXEC_TEST_OUTPUT", &output)
            .status()?;

        anyhow::ensure!(status.success(), "exec helper failed with {status}");
        assert_eq!(
            std::fs::read_to_string(output)?,
            format!("cwd=<{}>\ndelta=<exact value>\n", cwd.display())
        );
        Ok(())
    }
}
