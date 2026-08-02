use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::candidate::Availability;

const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
const TOOLS: &[&str] = &[
    "node", "npm", "pnpm", "yarn", "bun", "cargo", "rustc", "composer", "php", "go", "zig",
    "swift", "flutter", "dart", "python3", "python", "make", "docker",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolReport {
    pub program: &'static str,
    pub resolved_program: Option<PathBuf>,
    pub outcome: ProbeOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeOutcome {
    Missing,
    Version(String),
    Failed(String),
    TimedOut,
}

#[must_use]
pub fn inspect(cwd: &Path) -> Vec<ToolReport> {
    TOOLS
        .iter()
        .map(|&program| inspect_tool(program, cwd))
        .collect()
}

fn inspect_tool(program: &'static str, cwd: &Path) -> ToolReport {
    let Availability::Available { resolved_program } =
        crate::path::resolve_program(OsStr::new(program), cwd, &BTreeMap::new())
    else {
        return ToolReport {
            program,
            resolved_program: None,
            outcome: ProbeOutcome::Missing,
        };
    };
    let outcome = probe_version(&resolved_program, cwd, VERSION_TIMEOUT);
    ToolReport {
        program,
        resolved_program: Some(resolved_program),
        outcome,
    }
}

fn probe_version(program: &Path, cwd: &Path, timeout: Duration) -> ProbeOutcome {
    let mut child = match Command::new(program)
        .arg("--version")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return ProbeOutcome::Failed(error.to_string()),
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return ProbeOutcome::TimedOut;
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return ProbeOutcome::Failed(error.to_string());
            }
        }
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => return ProbeOutcome::Failed(error.to_string()),
    };
    let summary = first_line(&output.stdout)
        .or_else(|| first_line(&output.stderr))
        .unwrap_or_else(|| format!("exit {}", output.status.code().unwrap_or(1)));
    if output.status.success() {
        ProbeOutcome::Version(summary)
    } else {
        ProbeOutcome::Failed(summary)
    }
}

fn first_line(bytes: &[u8]) -> Option<String> {
    let line = bytes
        .split(|byte| *byte == b'\n' || *byte == b'\r')
        .find(|line| !line.is_empty())?;
    let mut output = String::from_utf8_lossy(line)
        .chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .take(160)
        .collect::<String>();
    if line.len() > 160 {
        output.push('…');
    }
    Some(output)
}

#[must_use]
pub fn render(reports: &[ToolReport]) -> String {
    let mut output = String::from("dev doctor\n\ntoolchains:\n");
    for report in reports {
        let (marker, detail) = match &report.outcome {
            ProbeOutcome::Missing => ("-", "not found on PATH".to_owned()),
            ProbeOutcome::Version(version) => ("ok", version.clone()),
            ProbeOutcome::Failed(error) => ("!", format!("version probe failed: {error}")),
            ProbeOutcome::TimedOut => (
                "!",
                format!(
                    "version probe timed out after {}ms",
                    VERSION_TIMEOUT.as_millis()
                ),
            ),
        };
        output.push_str(&format!("  {marker:>2} {:<9} {detail}\n", report.program));
    }
    output
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn executable(path: &Path, contents: &str) -> anyhow::Result<()> {
        std::fs::write(path, contents)?;
        let mut permissions = path.metadata()?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions)?;
        Ok(())
    }

    #[test]
    fn version_probe_captures_output_and_enforces_timeout() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let version = temp.path().join("version");
        executable(&version, "#!/bin/sh\nprintf 'probe 1.2.3\\n'\n")?;
        assert_eq!(
            probe_version(&version, temp.path(), Duration::from_millis(200)),
            ProbeOutcome::Version("probe 1.2.3".to_owned())
        );

        let slow = temp.path().join("slow");
        executable(&slow, "#!/bin/sh\nwhile :; do :; done\n")?;
        assert_eq!(
            probe_version(&slow, temp.path(), Duration::from_millis(20)),
            ProbeOutcome::TimedOut
        );
        Ok(())
    }
}
