use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::candidate::Availability;
use crate::registry::{CommandOutput, DoctorProbe, LocalMetadataProbe, ToolId, ToolRegistration};

const MAX_CAPTURE_BYTES: usize = 16 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolReport {
    pub tool: ToolId,
    pub program: &'static str,
    pub resolved_program: Option<PathBuf>,
    pub outcome: ProbeOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeOutcome {
    Missing,
    Version(String),
    PresentUnknown(String),
    Failed(String),
    TimedOut { timeout: Duration },
}

#[must_use]
pub fn inspect(cwd: &Path) -> Vec<ToolReport> {
    crate::registry::tools()
        .iter()
        .map(|tool| inspect_tool(*tool, cwd))
        .collect()
}

fn inspect_tool(tool: ToolRegistration, cwd: &Path) -> ToolReport {
    let Availability::Available { resolved_program } =
        crate::path::resolve_program(OsStr::new(tool.program), cwd, &BTreeMap::new())
    else {
        return ToolReport {
            tool: tool.id,
            program: tool.program,
            resolved_program: None,
            outcome: ProbeOutcome::Missing,
        };
    };
    let outcome = match tool.doctor {
        DoctorProbe::Command {
            args,
            timeout,
            output,
        } => probe_command(&resolved_program, args, cwd, timeout, output),
        DoctorProbe::LocalMetadata(probe) => probe_local_metadata(probe, &resolved_program),
        DoctorProbe::PresenceOnly { reason } => ProbeOutcome::PresentUnknown(reason.to_owned()),
    };
    ToolReport {
        tool: tool.id,
        program: tool.program,
        resolved_program: Some(resolved_program),
        outcome,
    }
}

fn probe_command(
    program: &Path,
    args: &[&str],
    cwd: &Path,
    timeout: Duration,
    output: CommandOutput,
) -> ProbeOutcome {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_process_scope(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return ProbeOutcome::Failed(error.to_string()),
    };
    let stdout = child.stdout.take().map(capture_bounded);
    let stderr = child.stderr.take().map(capture_bounded);
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                terminate_process_scope(&mut child);
                let _ = join_capture(stdout);
                let _ = join_capture(stderr);
                return ProbeOutcome::TimedOut { timeout };
            }
            Err(error) => {
                terminate_process_scope(&mut child);
                let _ = join_capture(stdout);
                let _ = join_capture(stderr);
                return ProbeOutcome::Failed(error.to_string());
            }
        }
    };
    let stdout = join_capture(stdout);
    let stderr = join_capture(stderr);
    let summary = selected_line(&stdout, output)
        .or_else(|| selected_line(&stderr, output))
        .or_else(|| first_line(&stdout))
        .or_else(|| first_line(&stderr))
        .unwrap_or_else(|| {
            format!(
                "exit {}",
                status.and_then(|status| status.code()).unwrap_or(1)
            )
        });
    if status.is_some_and(|status| status.success()) {
        ProbeOutcome::Version(summary)
    } else {
        ProbeOutcome::Failed(summary)
    }
}

fn capture_bounded<R>(mut reader: R) -> JoinHandle<Vec<u8>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let remaining = MAX_CAPTURE_BYTES.saturating_sub(captured.len());
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        captured
    })
}

fn join_capture(handle: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    handle
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default()
}

#[cfg(unix)]
fn configure_process_scope(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_scope(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_process_scope(child: &mut Child) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    if let Ok(pid) = i32::try_from(child.id()) {
        let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
fn terminate_process_scope(child: &mut Child) {
    let _ = Command::new("taskkill")
        .args(["/PID", &child.id().to_string(), "/T", "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_scope(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn probe_local_metadata(probe: LocalMetadataProbe, program: &Path) -> ProbeOutcome {
    match probe {
        LocalMetadataProbe::FlutterSdk => probe_flutter_metadata(program),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FlutterVersion {
    framework_version: Option<String>,
    flutter_version: Option<String>,
    channel: Option<String>,
}

fn probe_flutter_metadata(program: &Path) -> ProbeOutcome {
    let resolved = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    let Some(sdk_root) = resolved.parent().and_then(Path::parent) else {
        return ProbeOutcome::PresentUnknown(
            "present; SDK metadata location is unknown".to_owned(),
        );
    };
    let metadata_path = sdk_root.join("bin/cache/flutter.version.json");
    let mut file = match std::fs::File::open(&metadata_path) {
        Ok(file) => file,
        Err(_) => {
            return ProbeOutcome::PresentUnknown(
                "present; SDK-local version metadata is unavailable".to_owned(),
            );
        }
    };
    let mut bytes = Vec::new();
    if file
        .by_ref()
        .take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 > MAX_METADATA_BYTES
    {
        return ProbeOutcome::PresentUnknown(
            "present; SDK-local version metadata is unreadable".to_owned(),
        );
    }
    let Ok(metadata) = serde_json::from_slice::<FlutterVersion>(&bytes) else {
        return ProbeOutcome::PresentUnknown(
            "present; SDK-local version metadata is invalid".to_owned(),
        );
    };
    let Some(version) = metadata.framework_version.or(metadata.flutter_version) else {
        return ProbeOutcome::PresentUnknown(
            "present; SDK-local version metadata has no version".to_owned(),
        );
    };
    let summary = metadata
        .channel
        .filter(|channel| !channel.is_empty())
        .map_or_else(
            || format!("Flutter {version}"),
            |channel| format!("Flutter {version} ({channel})"),
        );
    ProbeOutcome::Version(summary)
}

fn first_line(bytes: &[u8]) -> Option<String> {
    let line = bytes
        .split(|byte| *byte == b'\n' || *byte == b'\r')
        .find(|line| !line.is_empty())?;
    Some(summarize_line(line))
}

fn selected_line(bytes: &[u8], output: CommandOutput) -> Option<String> {
    match output {
        CommandOutput::FirstNonEmptyLine => first_line(bytes),
        CommandOutput::LinePrefix(prefix) => bytes
            .split(|byte| *byte == b'\n' || *byte == b'\r')
            .find(|line| {
                String::from_utf8_lossy(line)
                    .trim_start()
                    .starts_with(prefix)
            })
            .map(summarize_line),
    }
}

fn summarize_line(line: &[u8]) -> String {
    let mut output = String::from_utf8_lossy(line)
        .chars()
        .take(160)
        .collect::<String>();
    output = crate::ui::terminal_text(&output);
    if line.len() > 160 {
        output.push('…');
    }
    output
}

#[must_use]
pub fn render(reports: &[ToolReport]) -> String {
    let mut output = String::from("dev doctor\n\ntoolchains:\n");
    for report in reports {
        let (marker, detail) = match &report.outcome {
            ProbeOutcome::Missing => ("-", "not found on PATH".to_owned()),
            ProbeOutcome::Version(version) => ("ok", version.clone()),
            ProbeOutcome::PresentUnknown(reason) => ("ok", reason.clone()),
            ProbeOutcome::Failed(error) => ("!", format!("version probe failed: {error}")),
            ProbeOutcome::TimedOut { timeout } => (
                "!",
                format!("version probe timed out after {}ms", timeout.as_millis()),
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
    fn command_probe_uses_exact_arguments_and_enforces_timeout() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let version = temp.path().join("version");
        executable(&version, "#!/bin/sh\nprintf '%s %s\\n' \"$1\" \"$2\"\n")?;
        assert_eq!(
            probe_command(
                &version,
                &["version", "--short"],
                temp.path(),
                Duration::from_secs(2),
                CommandOutput::FirstNonEmptyLine,
            ),
            ProbeOutcome::Version("version --short".to_owned())
        );

        let slow = temp.path().join("slow");
        executable(&slow, "#!/bin/sh\nwhile :; do :; done\n")?;
        let timeout = Duration::from_millis(20);
        assert_eq!(
            probe_command(
                &slow,
                &[],
                temp.path(),
                timeout,
                CommandOutput::FirstNonEmptyLine,
            ),
            ProbeOutcome::TimedOut { timeout }
        );
        Ok(())
    }

    #[test]
    fn command_probe_can_select_a_registry_declared_version_line() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let version = temp.path().join("gradle");
        executable(
            &version,
            "#!/bin/sh\nprintf '\\n------------------------------------------------------------\\nGradle 9.4.1\\n------------------------------------------------------------\\n'\n",
        )?;

        assert_eq!(
            probe_command(
                &version,
                &["--version"],
                temp.path(),
                Duration::from_secs(2),
                CommandOutput::LinePrefix("Gradle "),
            ),
            ProbeOutcome::Version("Gradle 9.4.1".to_owned())
        );
        Ok(())
    }

    #[test]
    fn flutter_probe_reads_sdk_metadata_without_running_launcher() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(bin.join("cache"))?;
        let flutter = bin.join("flutter");
        executable(&flutter, "#!/bin/sh\nexit 99\n")?;
        std::fs::write(
            bin.join("cache/flutter.version.json"),
            r#"{"frameworkVersion":"3.41.4","channel":"stable"}"#,
        )?;

        assert_eq!(
            probe_flutter_metadata(&flutter),
            ProbeOutcome::Version("Flutter 3.41.4 (stable)".to_owned())
        );
        Ok(())
    }
}
