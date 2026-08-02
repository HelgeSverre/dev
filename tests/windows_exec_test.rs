#![cfg(windows)]
#![allow(unsafe_code)]

use std::fs;
use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use windows_sys::Win32::System::Console::{
    GenerateConsoleCtrlEvent, GetConsoleWindow, CTRL_BREAK_EVENT,
};
use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;

/// Returns true when a real console window is attached to this process.
/// On headless CI runners (GitHub Actions, Docker, etc.) there is no
/// console, so `GenerateConsoleCtrlEvent` cannot propagate Ctrl-Break
/// to child process groups.
fn has_console() -> bool {
    unsafe { GetConsoleWindow() as usize != 0 }
}

fn project_with_npm(temp: &Path, script: &str) -> anyhow::Result<(PathBuf, PathBuf)> {
    let project = temp.join("project");
    let bin = temp.join("bin");
    fs::create_dir(&project)?;
    fs::create_dir(&bin)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"defined by test shim"}}"#,
    )?;
    fs::write(bin.join("npm.cmd"), script.replace('\n', "\r\n"))?;
    Ok((project, bin))
}

fn dev_command(project: &Path, bin: &Path) -> Command {
    let mut command = Command::new(cargo_bin!("dev"));
    command
        .args(["run", "--quiet", "--at"])
        .arg(project)
        .env("PATH", bin)
        .env("PATHEXT", ".CMD;.EXE");
    command
}

fn wait_for_file(child: &mut Child, path: &Path, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.is_file() {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            anyhow::bail!(
                "dev exited with {status} before child created {}",
                path.display()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    anyhow::bail!("timed out waiting for {}", path.display())
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    anyhow::bail!("timed out waiting for dev to exit")
}

fn delayed_marker_script(ready: &Path, late: &Path, milliseconds: u64) -> String {
    format!(
        "@echo off\necho ready>\"{}\"\n\"%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -NoLogo -NoProfile -NonInteractive -Command \"Start-Sleep -Milliseconds {milliseconds}\"\necho late>\"{}\"\n",
        ready.display(),
        late.display()
    )
}

#[test]
fn pathext_command_returns_the_exact_child_exit_code() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let (project, bin) = project_with_npm(temp.path(), "@echo off\nexit /b 37\n")?;
    let status = dev_command(&project, &bin).status()?;
    assert_eq!(status.code(), Some(37));
    Ok(())
}

#[test]
fn job_object_kills_descendants_when_the_launcher_is_terminated() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let ready = temp.path().join("ready.txt");
    let late = temp.path().join("late.txt");
    let script = delayed_marker_script(&ready, &late, 2_000);
    let (project, bin) = project_with_npm(temp.path(), &script)?;
    let mut dev = dev_command(&project, &bin).spawn()?;
    wait_for_file(&mut dev, &ready, Duration::from_secs(5))?;

    dev.kill()?;
    dev.wait()?;
    thread::sleep(Duration::from_millis(2_500));
    assert!(!late.exists(), "a descendant survived the Job Object");
    Ok(())
}

#[test]
fn ctrl_break_is_forwarded_to_the_child_process_group() -> anyhow::Result<()> {
    if !has_console() {
        eprintln!(
            "skipping: no console window attached (GenerateConsoleCtrlEvent cannot propagate)"
        );
        return Ok(());
    }
    let temp = tempfile::tempdir()?;
    let ready = temp.path().join("ready.txt");
    let late = temp.path().join("late.txt");
    let script = delayed_marker_script(&ready, &late, 10_000);
    let (project, bin) = project_with_npm(temp.path(), &script)?;
    let mut command = dev_command(&project, &bin);
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    let mut dev = command.spawn()?;
    wait_for_file(&mut dev, &ready, Duration::from_secs(5))?;
    thread::sleep(Duration::from_millis(100));

    // SAFETY: `dev.id()` is the process-group identifier created by the
    // CREATE_NEW_PROCESS_GROUP flag above, and this process retains the shared
    // console while the event is generated.
    if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, dev.id()) } == 0 {
        let error = std::io::Error::last_os_error();
        let _ = dev.kill();
        let _ = dev.wait();
        return Err(error.into());
    }
    wait_for_exit(&mut dev, Duration::from_secs(5))?;
    assert!(!late.exists(), "the interrupted child kept running");
    Ok(())
}

#[test]
fn command_quoting_preserves_spaces_empty_arguments_and_quotes() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let capture = temp.path().join("capture.ps1");
    let observed = temp.path().join("observed.json");
    let powershell_path = |path: &Path| path.to_string_lossy().replace('\'', "''");
    fs::write(
        &capture,
        format!(
            "$payload = [ordered]@{{ cwd = (Get-Location).Path; args = @($args) }}\n[IO.File]::WriteAllText('{}', (ConvertTo-Json -Compress -InputObject $payload))\n",
            powershell_path(&observed)
        ),
    )?;
    let shim = format!(
        "@echo off\n\"%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -NoLogo -NoProfile -NonInteractive -File \"{}\" %*\n",
        capture.display()
    );
    let (project, bin) = project_with_npm(temp.path(), &shim)?;

    let output = dev_command(&project, &bin)
        .args(["--", "--flag", "a b", "", "a'b\"c"])
        .output()?;
    anyhow::ensure!(output.status.success(), "dev failed: {output:?}");
    let payload: serde_json::Value = serde_json::from_slice(&fs::read(observed)?)?;
    // Canonicalize both paths to handle Windows 8.3 short-name divergence
    // (e.g. RUNNER~1 vs runneradmin) across different APIs.
    let cwd_from_ps = payload["cwd"].as_str().unwrap();
    let canonical_ps = fs::canonicalize(cwd_from_ps)?;
    let canonical_project = fs::canonicalize(&project)?;
    assert_eq!(canonical_ps, canonical_project);
    assert_eq!(
        payload["args"],
        serde_json::json!(["run", "dev", "--", "--flag", "a b", "", "a'b\"c"])
    );
    Ok(())
}
