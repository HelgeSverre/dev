use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

use crate::intent::{Intent, Invocation, Target};
use crate::path::logical_absolute;

/// Parsed top-level command line.
#[derive(Clone, Debug)]
pub enum Request {
    Resolve(ResolveRequest),
    Cache(CacheRequest),
    Doctor,
    Completions { shell: Option<Shell>, install: bool },
}

#[derive(Clone, Debug)]
pub struct ResolveRequest {
    pub invocation: Invocation,
    pub why: bool,
    pub list: bool,
    pub dry_run: bool,
    pub pick: bool,
    pub forget: bool,
    pub no_cache: bool,
    pub depth: usize,
    pub json: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub color: ColorMode,
}

#[derive(Clone, Debug)]
pub enum CacheRequest {
    List,
    Clear { yes: bool },
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Clap(#[from] clap::Error),
    #[error("{0}")]
    Usage(String),
    #[error("cannot inspect target `{path}`: {source}")]
    Target {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Parser)]
#[command(
    name = "dev",
    version,
    about = "Discover and run commands a project already defines",
    arg_required_else_help = true,
    disable_help_subcommand = true
)]
struct RawCli {
    #[command(subcommand)]
    command: RawCommand,
}

#[derive(Debug, Subcommand)]
enum RawCommand {
    /// Discover a command that runs the project or a target.
    Run(ActionArgs),
    /// Discover a command that builds the project or a target.
    Build(ActionArgs),
    /// Discover a command that tests the project or a target.
    Test(ActionArgs),
    /// Inspect or clear remembered choices.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Check locally available toolchains with bounded version probes.
    Doctor,
    /// Generate shell completion scripts.
    #[command(after_help = "Examples:
  dev completions --install
  dev completions zsh --install
  dev completions bash > ~/.local/share/bash-completion/completions/dev
  dev completions powershell >> $PROFILE")]
    Completions {
        /// Shell to generate completions for.
        #[arg(value_enum, required_unless_present = "install")]
        shell: Option<Shell>,
        /// Install the script, detecting the shell when it is omitted.
        #[arg(long)]
        install: bool,
    },
}

#[derive(Clone, Debug, Args)]
struct ActionArgs {
    /// Explicit filesystem target.
    #[arg(short = 'C', long = "at", value_name = "PATH")]
    at: Option<PathBuf>,

    /// Render ranking evidence without running.
    #[arg(short = 'w', long, conflicts_with_all = ["list", "dry_run", "json"])]
    why: bool,

    /// Print a terse candidate list without running.
    #[arg(short = 'l', long, conflicts_with_all = ["dry_run", "json"])]
    list: bool,

    /// Print the selected shell command without running.
    #[arg(short = 'n', long, conflicts_with = "json")]
    dry_run: bool,

    /// Force interactive selection.
    #[arg(short = 'p', long)]
    pick: bool,

    /// Forget the remembered choice for this exact invocation.
    #[arg(short = 'f', long)]
    forget: bool,

    /// Disable remembered-choice reads and writes.
    #[arg(long)]
    no_cache: bool,

    /// Fuzzy discovery breadth.
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=2))]
    chaos: Option<u8>,

    /// Structural scan depth.
    #[arg(long, default_value_t = 3, value_parser = parse_positive_depth)]
    depth: usize,

    /// Emit the versioned machine-readable result.
    #[arg(long)]
    json: bool,

    /// Suppress the execution preamble.
    #[arg(short = 'q', long)]
    quiet: bool,

    /// Emit scan and detector diagnostics.
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Color output mode.
    #[arg(long, value_enum, default_value_t = ColorMode::Auto)]
    color: ColorMode,

    /// Optional target followed by retrieval hints.
    #[arg(value_name = "TARGET_OR_HINT", allow_hyphen_values = false)]
    positionals: Vec<OsString>,
}

#[derive(Debug, Subcommand)]
enum CacheCommand {
    /// List remembered choices.
    List,
    /// Clear every remembered choice.
    Clear {
        /// Skip interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
}

/// Parse a platform-native argument sequence while preserving arguments after `--`.
pub fn parse_from<I, T>(arguments: I, current_directory: &Path) -> Result<Request, CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut before_delimiter = Vec::new();
    let mut passthrough = Vec::new();
    let mut found_delimiter = false;
    for argument in arguments.into_iter().map(Into::into) {
        if !found_delimiter && argument == OsStr::new("--") {
            found_delimiter = true;
        } else if found_delimiter {
            passthrough.push(argument);
        } else {
            before_delimiter.push(argument);
        }
    }

    let raw = RawCli::try_parse_from(before_delimiter)?;
    match raw.command {
        RawCommand::Run(args) => resolve_request(Intent::Run, args, passthrough, current_directory),
        RawCommand::Build(args) => {
            resolve_request(Intent::Build, args, passthrough, current_directory)
        }
        RawCommand::Test(args) => {
            resolve_request(Intent::Test, args, passthrough, current_directory)
        }
        RawCommand::Cache { command } => {
            if !passthrough.is_empty() {
                return Err(CliError::Usage(
                    "cache commands do not accept passthrough arguments".to_owned(),
                ));
            }
            match command {
                CacheCommand::List => Ok(Request::Cache(CacheRequest::List)),
                CacheCommand::Clear { yes } => Ok(Request::Cache(CacheRequest::Clear { yes })),
            }
        }
        RawCommand::Doctor => {
            if !passthrough.is_empty() {
                return Err(CliError::Usage(
                    "doctor does not accept passthrough arguments".to_owned(),
                ));
            }
            Ok(Request::Doctor)
        }
        RawCommand::Completions { shell, install } => {
            if !passthrough.is_empty() {
                return Err(CliError::Usage(
                    "completions does not accept passthrough arguments".to_owned(),
                ));
            }
            Ok(Request::Completions { shell, install })
        }
    }
}

/// Write a completion script for `dev` to the supplied writer.
pub fn write_completions(shell: Shell, writer: &mut impl Write) {
    clap_complete::generate(shell, &mut RawCli::command(), "dev", writer);
}

fn resolve_request(
    intent: Intent,
    args: ActionArgs,
    passthrough: Vec<OsString>,
    current_directory: &Path,
) -> Result<Request, CliError> {
    let mut positional_target = None;
    let mut hints = Vec::new();
    for positional in args.positionals {
        if is_path_like(&positional) {
            if positional_target.is_some() || args.at.is_some() {
                return Err(CliError::Usage(
                    "only one target is accepted; use non-path words as hints".to_owned(),
                ));
            }
            positional_target = Some(PathBuf::from(positional));
        } else {
            hints.push(positional.into_string().map_err(|value| {
                CliError::Usage(format!(
                    "hint is not valid Unicode: {}",
                    value.to_string_lossy()
                ))
            })?);
        }
    }

    let supplied_target = args
        .at
        .or(positional_target)
        .unwrap_or_else(|| PathBuf::from("."));
    let logical_target =
        logical_absolute(current_directory, &supplied_target).map_err(|source| {
            CliError::Target {
                path: supplied_target.clone(),
                source,
            }
        })?;
    let metadata = logical_target
        .metadata()
        .map_err(|source| CliError::Target {
            path: supplied_target,
            source,
        })?;
    let target = if metadata.is_dir() {
        Target::Directory(logical_target)
    } else if metadata.is_file() {
        Target::File(logical_target)
    } else {
        return Err(CliError::Usage(
            "target must be a regular file or directory".to_owned(),
        ));
    };

    let chaos = match (hints.is_empty(), args.pick, args.chaos) {
        (true, false, _) => 0,
        (true, true, Some(value)) | (false, _, Some(value)) => value,
        (true, true, None) => 0,
        (false, _, None) => 1,
    };

    Ok(Request::Resolve(ResolveRequest {
        invocation: Invocation {
            intent,
            target,
            hints,
            passthrough,
            chaos,
        },
        why: args.why,
        list: args.list,
        dry_run: args.dry_run,
        pick: args.pick,
        forget: args.forget,
        no_cache: args.no_cache,
        depth: args.depth,
        json: args.json,
        quiet: args.quiet,
        verbose: args.verbose,
        color: args.color,
    }))
}

fn parse_positive_depth(value: &str) -> Result<usize, String> {
    let depth = value
        .parse::<usize>()
        .map_err(|error| format!("invalid depth: {error}"))?;
    if depth == 0 {
        Err("depth must be at least 1".to_owned())
    } else {
        Ok(depth)
    }
}

#[must_use]
pub fn is_path_like(value: &OsStr) -> bool {
    let path = Path::new(value);
    if path.is_absolute() || value == OsStr::new(".") || value == OsStr::new("..") {
        return true;
    }
    let display = value.to_string_lossy();
    display.starts_with("./")
        || display.starts_with("../")
        || display.starts_with(".\\")
        || display.starts_with("..\\")
        || display.contains('/')
        || display.contains('\\')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_existing_name_remains_a_hint() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        std::fs::create_dir(directory.path().join("test"))?;
        let request = parse_from(["dev", "run", "test"], directory.path())?;
        let Request::Resolve(request) = request else {
            anyhow::bail!("expected resolve request");
        };
        assert_eq!(request.invocation.hints, ["test"]);
        assert_eq!(request.invocation.target.path(), directory.path());
        Ok(())
    }

    #[test]
    fn bare_name_parses_identically_when_no_matching_path_exists() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let without_path = parse_from(["dev", "run", "test"], directory.path())?;
        std::fs::create_dir(directory.path().join("test"))?;
        let with_path = parse_from(["dev", "run", "test"], directory.path())?;
        let (Request::Resolve(without_path), Request::Resolve(with_path)) =
            (without_path, with_path)
        else {
            anyhow::bail!("expected resolve requests");
        };
        assert_eq!(without_path.invocation, with_path.invocation);
        Ok(())
    }

    #[test]
    fn delimiter_preserves_opaque_arguments() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let request = parse_from(
            ["dev", "build", "rust", "--", "--release", ""],
            directory.path(),
        )?;
        let Request::Resolve(request) = request else {
            anyhow::bail!("expected resolve request");
        };
        assert_eq!(request.invocation.hints, ["rust"]);
        assert_eq!(request.invocation.passthrough, ["--release", ""]);
        Ok(())
    }

    #[test]
    fn missing_path_like_target_is_an_error() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let result = parse_from(["dev", "run", "./missing"], directory.path());
        assert!(matches!(result, Err(CliError::Target { .. })));
        Ok(())
    }

    #[test]
    fn human_and_machine_output_modes_conflict() {
        for arguments in [
            ["dev", "run", "--why", "--list"],
            ["dev", "run", "--why", "--json"],
            ["dev", "run", "--list", "--dry-run"],
            ["dev", "run", "--dry-run", "--json"],
        ] {
            let result = parse_from(arguments, Path::new("."));
            assert!(matches!(
                result,
                Err(CliError::Clap(error))
                    if error.kind() == clap::error::ErrorKind::ArgumentConflict
            ));
        }
    }

    #[test]
    fn completion_shell_is_parsed() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let request = parse_from(["dev", "completions", "fish"], directory.path())?;
        assert!(matches!(
            request,
            Request::Completions {
                shell: Some(Shell::Fish),
                install: false
            }
        ));
        Ok(())
    }

    #[test]
    fn completion_install_can_detect_the_shell() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let request = parse_from(["dev", "completions", "--install"], directory.path())?;
        assert!(matches!(
            request,
            Request::Completions {
                shell: None,
                install: true
            }
        ));
        Ok(())
    }

    #[test]
    fn every_supported_completion_script_is_generated() {
        for shell in [
            Shell::Bash,
            Shell::Elvish,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Zsh,
        ] {
            let mut output = Vec::new();
            write_completions(shell, &mut output);
            assert!(!output.is_empty(), "{shell:?} completion was empty");
            assert!(
                output.windows(3).any(|window| window == b"dev"),
                "{shell:?} completion did not reference dev"
            );
        }
    }
}
