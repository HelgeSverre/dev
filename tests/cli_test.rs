#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use assert_cmd::cargo::cargo_bin_cmd;

fn write_executable(path: &Path, contents: &str) -> anyhow::Result<()> {
    fs::write(path, contents)?;
    let mut permissions = path.metadata()?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn fake_program(directory: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let bin = directory.join("bin");
    fs::create_dir_all(&bin)?;
    let program = bin.join(name);
    write_executable(
        &program,
        r#"#!/bin/sh
printf 'cwd=<%s>\n' "$PWD"
index=0
for argument in "$@"; do
  printf 'arg%s=<%s>\n' "$index" "$argument"
  index=$((index + 1))
done
if [ -n "${DEV_FAKE_READ_STDIN:-}" ]; then
  IFS= read -r line
  printf 'stdin=<%s>\n' "$line"
fi
if [ -n "${DEV_FAKE_STDERR:-}" ]; then
  printf 'child-stderr=<%s>\n' "$DEV_FAKE_STDERR" >&2
fi
exit "${DEV_FAKE_EXIT:-0}"
"#,
    )?;
    Ok(bin)
}

fn remember_with_picker(
    project: &Path,
    state: &Path,
    bin: &Path,
    hints: &[&str],
) -> anyhow::Result<()> {
    use std::process::Command;
    use std::time::Duration;

    use expectrl::{Eof, Expect, Session};

    let mut command = Command::new(env!("CARGO_BIN_EXE_dev"));
    command
        .args(["run", "--quiet", "--pick"])
        .args(hints)
        .arg("--at")
        .arg(project)
        .env("PATH", bin)
        .env("XDG_STATE_HOME", state)
        .env("TERM", "xterm-256color");
    let mut session = Session::spawn(command).context("spawning dev in a PTY")?;
    session.set_expect_timeout(Some(Duration::from_secs(5)));
    session
        .expect("\u{1b}[?1049h")
        .map_err(anyhow::Error::from)
        .context("waiting for picker alternate screen")?;
    session
        .send("\u{12}")
        .map_err(anyhow::Error::from)
        .context("sending Ctrl-R")?;
    session
        .expect(Eof)
        .map_err(anyhow::Error::from)
        .context("waiting for remembered command to exit")?;
    Ok(())
}

#[test]
fn node_script_executes_real_child_with_exact_passthrough_and_cwd() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("project with spaces");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"ignored by the detector"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--at"])
        .arg(&project)
        .args(["--", "--port", "a b", ""])
        .env("PATH", &bin);
    command.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<dev>\narg2=<-->\narg3=<--port>\narg4=<a b>\narg5=<>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn unmatched_hints_do_not_execute_the_structural_default() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("project");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"must not run"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--at"])
        .arg(&project)
        .args(["purple", "monkey", "lasagna"])
        .env("PATH", &bin);
    command
        .assert()
        .code(5)
        .stdout("")
        .stderr(predicates::str::contains("HintNoMatch"));
    Ok(())
}

#[test]
fn cargo_json_is_deterministic_and_describes_the_known_command() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("rust-project");
    fs::create_dir_all(project.join("src"))?;
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(project.join("src/main.rs"), "fn main() {}\n")?;
    let bin = fake_program(temp.path(), "cargo")?;

    let run = || -> anyhow::Result<Vec<u8>> {
        let mut command = cargo_bin_cmd!("dev");
        let output = command
            .args(["run", "--json", "--at"])
            .arg(&project)
            .env("PATH", &bin)
            .output()?;
        anyhow::ensure!(output.status.success(), "dev failed: {output:?}");
        Ok(output.stdout)
    };
    let first = run()?;
    let second = run()?;
    assert_eq!(first, second);

    let json: serde_json::Value = serde_json::from_slice(&first)?;
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["resolution"]["status"], "resolved");
    assert_eq!(json["candidates"][0]["program"]["display"], "cargo");
    assert_eq!(json["candidates"][0]["args"][0]["display"], "run");
    assert_eq!(
        json["candidates"][0]["cwd"],
        project.to_string_lossy().as_ref()
    );
    Ok(())
}

#[test]
fn typo_can_select_an_explicit_obscure_node_script() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("node-project");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"refresh-search-index":"ignored"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--at"])
        .arg(&project)
        .arg("refresh-search-indxe")
        .env("PATH", &bin);
    command.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<refresh-search-index>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn deep_cargo_workspace_members_are_indexed_and_use_workspace_commands() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("cargo-workspace");
    let member = project.join("deep/services/probe");
    fs::create_dir_all(member.join("src"))?;
    fs::write(
        project.join("Cargo.toml"),
        "[workspace]\nmembers = [\"deep/services/probe\"]\nresolver = \"2\"\n",
    )?;
    fs::write(
        member.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(member.join("src/main.rs"), "fn main() {}\n")?;
    let bin = fake_program(temp.path(), "cargo")?;

    let mut workspace_build = cargo_bin_cmd!("dev");
    let output = workspace_build
        .args(["build", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .output()?;
    anyhow::ensure!(output.status.success(), "dev failed: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["candidates"].as_array().map(Vec::len), Some(1));
    assert_eq!(json["candidates"][0]["args"][0]["display"], "build");
    assert_eq!(json["candidates"][0]["args"][1]["display"], "--workspace");

    let mut member_run = cargo_bin_cmd!("dev");
    let output = member_run
        .args(["run", "--json", "--at"])
        .arg(&member)
        .env("PATH", &bin)
        .output()?;
    anyhow::ensure!(output.status.success(), "dev failed: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let args = json["candidates"][0]["args"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("candidate args must be an array"))?
        .iter()
        .map(|argument| argument["display"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(args, ["run", "-p", "probe"]);
    assert_eq!(
        json["candidates"][0]["cwd"],
        project.to_string_lossy().as_ref()
    );
    Ok(())
}

#[test]
fn malformed_cargo_manifest_is_reported_in_json() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("malformed-cargo");
    fs::create_dir(&project)?;
    fs::write(project.join("Cargo.toml"), "[package\nname = nope\n")?;
    let bin = fake_program(temp.path(), "cargo")?;

    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .output()?;
    assert_eq!(output.status.code(), Some(4));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["resolution"]["status"], "no_candidates");
    assert_eq!(json["diagnostics"][0]["detector"], "cargo");
    assert!(json["diagnostics"][0]["message"]
        .as_str()
        .is_some_and(|message| message.contains("invalid Cargo.toml")));
    Ok(())
}

#[test]
fn node_workspace_member_uses_the_workspace_package_manager() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("node-workspace");
    let member = project.join("deep/apps/web");
    fs::create_dir_all(&member)?;
    fs::write(
        project.join("package.json"),
        r#"{"workspaces":["deep/apps/web"]}"#,
    )?;
    fs::write(project.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n")?;
    fs::write(
        member.join("package.json"),
        r#"{"name":"web","scripts":{"dev":"vite"}}"#,
    )?;
    let bin = fake_program(temp.path(), "pnpm")?;

    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--json", "--at"])
        .arg(&member)
        .env("PATH", &bin)
        .output()?;
    anyhow::ensure!(output.status.success(), "dev failed: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["resolution"]["status"], "resolved");
    assert_eq!(json["candidates"][0]["program"]["display"], "pnpm");
    assert!(json["candidates"][0]["structural_evidence"]
        .as_array()
        .is_some_and(|evidence| evidence.iter().any(|item| item["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("pnpm-lock.yaml")))));
    Ok(())
}

#[test]
fn next_start_requires_an_explicit_identity_hint() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("next-project");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"next dev","start":"next start"},"dependencies":{"next":"16"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;

    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .output()?;
    anyhow::ensure!(output.status.success(), "dev failed: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["resolution"]["status"], "resolved");
    let candidates = json["candidates"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("candidates must be an array"))?;
    let start = candidates
        .iter()
        .find(|candidate| {
            candidate["action_key"]
                .as_str()
                .is_some_and(|key| key.ends_with(":start"))
        })
        .ok_or_else(|| anyhow::anyhow!("Next start candidate must exist"))?;
    assert_eq!(start["policy"], "explicit_hint");
    assert!(start["description"]
        .as_str()
        .is_some_and(|description| description.contains("prior Next build")));
    Ok(())
}

#[test]
fn child_stdio_and_exit_status_are_preserved() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("stdio-project");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"stdio"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("DEV_FAKE_READ_STDIN", "1")
        .env("DEV_FAKE_STDERR", "visible")
        .env("DEV_FAKE_EXIT", "37")
        .write_stdin("from-parent\n");
    command
        .assert()
        .code(37)
        .stdout(predicates::str::contains("stdin=<from-parent>"))
        .stderr("child-stderr=<visible>\n");
    Ok(())
}

#[test]
fn child_observes_the_inherited_terminal() -> anyhow::Result<()> {
    use std::process::Command;
    use std::time::Duration;

    use expectrl::{Eof, Expect, Session};

    let temp = tempfile::tempdir()?;
    let project = temp.path().join("tty-project");
    let bin = temp.path().join("bin");
    fs::create_dir(&project)?;
    fs::create_dir(&bin)?;
    fs::write(project.join("package.json"), r#"{"scripts":{"dev":"tty"}}"#)?;
    write_executable(
        &bin.join("npm"),
        r#"#!/bin/sh
stdin=no
stdout=no
stderr=no
test -t 0 && stdin=yes
test -t 1 && stdout=yes
test -t 2 && stderr=yes
printf 'tty stdin=%s stdout=%s stderr=%s\n' "$stdin" "$stdout" "$stderr"
"#,
    )?;

    let mut command = Command::new(env!("CARGO_BIN_EXE_dev"));
    command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("TERM", "xterm-256color");
    let mut session = Session::spawn(command)?;
    session.set_expect_timeout(Some(Duration::from_secs(5)));
    session
        .expect("tty stdin=yes stdout=yes stderr=yes")
        .map_err(anyhow::Error::from)?;
    session.expect(Eof).map_err(anyhow::Error::from)?;
    Ok(())
}

#[test]
fn signal_reaches_the_exec_replaced_child() -> anyhow::Result<()> {
    use std::os::unix::process::ExitStatusExt as _;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir()?;
    let project = temp.path().join("signal-project");
    let bin = temp.path().join("bin");
    let ready = temp.path().join("child-ready");
    fs::create_dir(&project)?;
    fs::create_dir(&bin)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"signal"}}"#,
    )?;
    write_executable(
        &bin.join("npm"),
        r#"#!/bin/sh
: > "$DEV_SIGNAL_READY"
exec /bin/sleep 30
"#,
    )?;

    let mut child = Command::new(env!("CARGO_BIN_EXE_dev"))
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("DEV_SIGNAL_READY", &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("dev exited with {status} before the child became ready");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("timed out waiting for the exec-replaced child");
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let delivered = Command::new("/bin/kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()?;
    anyhow::ensure!(delivered.success(), "failed to deliver SIGTERM");
    let status = child.wait()?;
    assert_eq!(status.signal(), Some(15));
    Ok(())
}

#[test]
fn non_utf8_passthrough_bytes_reach_the_child_unchanged() -> anyhow::Result<()> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = tempfile::tempdir()?;
    let project = temp.path().join("bytes-project");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"bytes"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;
    let opaque = OsString::from_vec(vec![b'f', 0x80, b'o']);

    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .arg("--")
        .arg(opaque)
        .env("PATH", &bin)
        .output()?;
    anyhow::ensure!(output.status.success(), "dev failed: {output:?}");
    let mut expected = format!(
        "cwd=<{}>\narg0=<run>\narg1=<dev>\narg2=<-->\narg3=<f",
        project.display()
    )
    .into_bytes();
    expected.extend([0x80, b'o', b'>', b'\n']);
    assert_eq!(output.stdout, expected);
    Ok(())
}

#[test]
fn recursion_is_rejected_before_exec() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir()?;
    let project = temp.path().join("recursive-project");
    let bin = temp.path().join("bin");
    fs::create_dir(&project)?;
    fs::create_dir(&bin)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"recursive"}}"#,
    )?;
    symlink(env!("CARGO_BIN_EXE_dev"), bin.join("npm"))?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    command
        .assert()
        .code(1)
        .stdout("")
        .stderr(predicates::str::contains("refusing to recursively execute"));
    Ok(())
}

#[test]
fn package_managers_receive_their_declared_passthrough_shape() -> anyhow::Result<()> {
    let cases = [
        ("npm", "11.16.0", true),
        ("pnpm", "11.15.1", true),
        ("yarn", "4.9.2", false),
        ("bun", "1.3.14", false),
    ];
    for (manager, version, inserts_double_dash) in cases {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join(format!("{manager}-project"));
        fs::create_dir(&project)?;
        fs::write(
            project.join("package.json"),
            format!(r#"{{"packageManager":"{manager}@{version}","scripts":{{"dev":"probe"}}}}"#),
        )?;
        let bin = fake_program(temp.path(), manager)?;

        let mut command = cargo_bin_cmd!("dev");
        let output = command
            .args(["run", "--quiet", "--at"])
            .arg(&project)
            .args(["--", "--flag", "value"])
            .env("PATH", &bin)
            .output()?;
        anyhow::ensure!(output.status.success(), "{manager} case failed: {output:?}");
        let stdout = String::from_utf8(output.stdout)?;
        assert!(stdout.contains("arg0=<run>"), "{manager}: {stdout}");
        assert!(stdout.contains("arg1=<dev>"), "{manager}: {stdout}");
        if inserts_double_dash {
            assert!(stdout.contains("arg2=<-->"), "{manager}: {stdout}");
            assert!(stdout.contains("arg3=<--flag>"), "{manager}: {stdout}");
            assert!(stdout.contains("arg4=<value>"), "{manager}: {stdout}");
        } else {
            assert!(stdout.contains("arg2=<--flag>"), "{manager}: {stdout}");
            assert!(stdout.contains("arg3=<value>"), "{manager}: {stdout}");
            assert!(!stdout.contains("arg4="), "{manager}: {stdout}");
        }
    }
    Ok(())
}

#[test]
fn picker_can_teach_and_cache_maintenance_can_clear_a_choice() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("teachable-project");
    let state = temp.path().join("state");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;
    remember_with_picker(&project, &state, &bin, &[])?;

    let mut remembered = cargo_bin_cmd!("dev");
    remembered
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state);
    remembered
        .assert()
        .success()
        .stdout(predicates::str::contains("arg1=<dev>"));

    let mut list = cargo_bin_cmd!("dev");
    list.args(["cache", "list"]).env("XDG_STATE_HOME", &state);
    list.assert().success().stdout(predicates::str::contains(
        "node:teachable-project:script:dev",
    ));

    let mut clear = cargo_bin_cmd!("dev");
    clear
        .args(["cache", "clear", "--yes"])
        .env("XDG_STATE_HOME", &state);
    clear
        .assert()
        .success()
        .stderr(predicates::str::contains("cleared 1 remembered choice"));

    let mut after_clear = cargo_bin_cmd!("dev");
    after_clear
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state);
    after_clear.assert().code(5);
    Ok(())
}

#[test]
fn picker_cancel_restores_the_terminal_without_running_or_caching() -> anyhow::Result<()> {
    use std::process::Command;
    use std::time::Duration;

    use expectrl::{Eof, Expect, Session};

    let temp = tempfile::tempdir()?;
    let project = temp.path().join("cancelled-picker-project");
    let state = temp.path().join("state");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_dev"));
    command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state)
        .env("TERM", "xterm-256color");
    let mut session = Session::spawn(command)?;
    session.set_expect_timeout(Some(Duration::from_secs(5)));
    session
        .expect("\u{1b}[?1049h")
        .map_err(anyhow::Error::from)?;
    session.send("\u{1b}").map_err(anyhow::Error::from)?;
    session
        .expect("\u{1b}[?1049l")
        .map_err(anyhow::Error::from)
        .context("waiting for terminal restoration")?;
    session.expect(Eof).map_err(anyhow::Error::from)?;
    assert!(!state.join("dev/choices.json").exists());
    Ok(())
}

#[test]
fn corrupt_cache_is_quarantined_and_never_blocks_execution() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("corrupt-cache-project");
    let state = temp.path().join("state");
    let cache_directory = state.join("dev");
    fs::create_dir(&project)?;
    fs::create_dir_all(&cache_directory)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"works"}}"#,
    )?;
    fs::write(cache_directory.join("choices.json"), "{not json")?;
    let bin = fake_program(temp.path(), "npm")?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state);
    command
        .assert()
        .success()
        .stdout(predicates::str::contains("arg1=<dev>"))
        .stderr(predicates::str::contains("ignored corrupt cache"));
    assert!(fs::read_dir(cache_directory)?
        .filter_map(Result::ok)
        .any(|entry| { entry.file_name().to_string_lossy().contains("corrupt-") }));
    Ok(())
}

#[test]
fn corrupt_cache_is_recovered_during_a_locked_write() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let state = temp.path().join("state");
    let cache_directory = state.join("dev");
    fs::create_dir_all(&cache_directory)?;
    fs::write(cache_directory.join("choices.json"), "{not json")?;

    let mut clear = cargo_bin_cmd!("dev");
    clear
        .args(["cache", "clear", "--yes"])
        .env("XDG_STATE_HOME", &state);
    clear
        .assert()
        .success()
        .stderr(predicates::str::contains("ignored corrupt cache"));

    let recovered: serde_json::Value =
        serde_json::from_slice(&fs::read(cache_directory.join("choices.json"))?)?;
    assert_eq!(recovered["schema_version"], 1);
    assert_eq!(recovered["entries"].as_array().map(Vec::len), Some(0));
    assert!(fs::read_dir(cache_directory)?
        .filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().contains("corrupt-")));
    Ok(())
}

#[test]
fn stale_remembered_choices_revalidate_exact_action_semantics() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("stale-project");
    let state = temp.path().join("state");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;
    remember_with_picker(&project, &state, &bin, &[])?;

    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second","lint":"third"}}"#,
    )?;
    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state);
    command
        .assert()
        .success()
        .stdout(predicates::str::contains("arg1=<dev>"))
        .stderr(predicates::str::contains(
            "project changed; remembered action still exists",
        ));

    let mut fast_again = cargo_bin_cmd!("dev");
    fast_again
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state);
    fast_again
        .assert()
        .success()
        .stdout(predicates::str::contains("arg1=<dev>"))
        .stderr("");
    Ok(())
}

#[test]
fn stale_choice_never_accepts_changed_argv_for_the_same_action() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("changed-command-project");
    let state = temp.path().join("state");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;
    remember_with_picker(&project, &state, &bin, &[])?;

    fs::write(project.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n")?;
    fake_program(temp.path(), "pnpm")?;
    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state)
        .output()?;
    assert_eq!(output.status.code(), Some(5));
    assert!(output.stderr.is_empty(), "unexpected stderr: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["resolution"]["status"], "ambiguous");
    assert_eq!(json["resolution"]["reason"], "remembered_command_changed");
    assert_eq!(json["candidates"][0]["program"]["display"], "pnpm");
    Ok(())
}

#[test]
fn disappeared_remembered_action_is_retained_as_unavailable_context() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("disappeared-action-project");
    let state = temp.path().join("state");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;
    remember_with_picker(&project, &state, &bin, &[])?;

    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"start":"second"}}"#,
    )?;
    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state)
        .output()?;
    assert_eq!(output.status.code(), Some(5));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["resolution"]["reason"], "remembered_action_missing");
    let remembered = json["candidates"]
        .as_array()
        .and_then(|candidates| {
            candidates.iter().find(|candidate| {
                candidate["action_key"]
                    .as_str()
                    .is_some_and(|key| key.ends_with(":script:dev"))
            })
        })
        .ok_or_else(|| anyhow::anyhow!("missing remembered context candidate"))?;
    assert_eq!(remembered["availability"]["status"], "unsupported_host");
    Ok(())
}

#[test]
fn concurrent_picker_writers_preserve_every_remembered_query() -> anyhow::Result<()> {
    use std::process::Command;
    use std::time::Duration;

    use expectrl::{Eof, Expect, Session};

    let temp = tempfile::tempdir()?;
    let project = temp.path().join("concurrent-project");
    let state = temp.path().join("state");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;

    let mut sessions = Vec::new();
    for query in ["dev", "start", "npm", "concurrent-project"] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dev"));
        command
            .args(["run", "--quiet", "--pick", query, "--at"])
            .arg(&project)
            .env("PATH", &bin)
            .env("XDG_STATE_HOME", &state)
            .env("TERM", "xterm-256color");
        let mut session = Session::spawn(command)?;
        session.set_expect_timeout(Some(Duration::from_secs(5)));
        session
            .expect("\u{1b}[?1049h")
            .map_err(anyhow::Error::from)
            .context("waiting for concurrent picker")?;
        sessions.push(session);
    }
    for session in &mut sessions {
        session
            .send("\u{12}")
            .map_err(anyhow::Error::from)
            .context("sending concurrent Ctrl-R")?;
    }
    for session in &mut sessions {
        session
            .expect(Eof)
            .map_err(anyhow::Error::from)
            .context("waiting for concurrent remembered command")?;
    }

    let store: serde_json::Value =
        serde_json::from_slice(&fs::read(state.join("dev/choices.json"))?)?;
    assert_eq!(store["entries"].as_array().map(Vec::len), Some(4));
    Ok(())
}

#[test]
fn busy_cache_lock_skips_remembering_but_still_executes() -> anyhow::Result<()> {
    use std::fs::OpenOptions;
    use std::process::Command;
    use std::time::Duration;

    use expectrl::{Eof, Expect, Session};
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("busy-cache-project");
    let state = temp.path().join("state");
    let cache_directory = state.join("dev");
    fs::create_dir(&project)?;
    fs::create_dir_all(&cache_directory)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache_directory.join("choices.lock"))?;
    fs4::FileExt::lock(&lock)?;

    let mut command = Command::new(env!("CARGO_BIN_EXE_dev"));
    command
        .args(["run", "--quiet", "--pick", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state)
        .env("TERM", "xterm-256color");
    let mut session = Session::spawn(command)?;
    session.set_expect_timeout(Some(Duration::from_secs(5)));
    session
        .expect("\u{1b}[?1049h")
        .map_err(anyhow::Error::from)?;
    session.send("\u{12}").map_err(anyhow::Error::from)?;
    session
        .expect("cache lock remained busy")
        .map_err(anyhow::Error::from)?;
    session.expect(Eof).map_err(anyhow::Error::from)?;

    fs4::FileExt::unlock(&lock)?;
    assert!(!cache_directory.join("choices.json").exists());
    Ok(())
}

#[test]
#[cfg(target_os = "linux")]
fn remembered_choice_round_trips_a_non_utf8_project_path() -> anyhow::Result<()> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt as _;

    let temp = tempfile::tempdir()?;
    let project = temp.path().join(OsString::from_vec(vec![
        b'o', b'p', b'a', 0x80, b'q', b'u', b'e',
    ]));
    let state = temp.path().join("state");
    fs::create_dir(&project).context("creating non-UTF-8 project directory")?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"first","start":"second"}}"#,
    )
    .context("writing manifest in non-UTF-8 project directory")?;
    let bin = fake_program(temp.path(), "npm").context("creating fake npm")?;
    remember_with_picker(&project, &state, &bin, &[])
        .context("remembering choice for non-UTF-8 project")?;

    let stored = fs::read_to_string(state.join("dev/choices.json"))?;
    assert!(
        stored.contains("unix-bytes"),
        "cache lost opaque path: {stored}"
    );

    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .env("XDG_STATE_HOME", &state)
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "cached execution failed: {output:?}"
    );
    assert!(output
        .stdout
        .windows(b"arg1=<dev>\n".len())
        .any(|window| window == b"arg1=<dev>\n"));
    Ok(())
}

#[test]
fn composer_script_executes_with_documented_argument_forwarding() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("composer-project");
    fs::create_dir(&project)?;
    fs::write(
        project.join("composer.json"),
        r#"{
            "name":"acme/tool",
            "scripts":{
                "dev":["php server.php","php worker.php"],
                "deploy":"php deploy.php"
            }
        }"#,
    )?;
    let bin = fake_program(temp.path(), "composer")?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .args(["--", "--host", "127.0.0.1"])
        .env("PATH", &bin);
    command.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run-script>\narg1=<dev>\narg2=<-->\narg3=<--host>\narg4=<127.0.0.1>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn malformed_composer_manifest_is_diagnostic_not_a_command() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("malformed-composer");
    fs::create_dir(&project)?;
    fs::write(project.join("composer.json"), "{\"scripts\":")?;
    let bin = fake_program(temp.path(), "composer")?;

    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .output()?;
    assert_eq!(output.status.code(), Some(4));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["resolution"]["status"], "no_candidates");
    assert!(json["diagnostics"]
        .as_array()
        .is_some_and(|diagnostics| diagnostics.iter().any(|diagnostic| {
            diagnostic["detector"] == "composer"
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("invalid composer.json"))
        })));
    Ok(())
}

#[test]
fn laravel_prefers_composer_dev_and_binds_artisan_to_a_test_file() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("laravel-project");
    let test_file = project.join("tests/Feature/AuthTest.php");
    fs::create_dir_all(test_file.parent().unwrap_or(&project))?;
    fs::write(
        project.join("composer.json"),
        r#"{
            "name":"acme/app",
            "require":{"laravel/framework":"^13.0"},
            "scripts":{"dev":["php artisan serve","npm run dev"]}
        }"#,
    )?;
    fs::write(project.join("artisan"), "<?php\n")?;
    fs::write(&test_file, "<?php\n")?;
    let bin = fake_program(temp.path(), "composer")?;
    fake_program(temp.path(), "php")?;

    let mut run = cargo_bin_cmd!("dev");
    let output = run
        .args(["run", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .output()?;
    anyhow::ensure!(output.status.success(), "Laravel run failed: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["resolution"]["status"], "resolved");
    let selected = json["resolution"]["selected_candidate_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing selected candidate"))?;
    let selected_candidate = json["candidates"]
        .as_array()
        .and_then(|candidates| {
            candidates
                .iter()
                .find(|candidate| candidate["id"].as_str() == Some(selected))
        })
        .ok_or_else(|| anyhow::anyhow!("missing selected Composer candidate"))?;
    assert_eq!(selected_candidate["detector"], "composer");
    assert_eq!(selected_candidate["lifecycle"], "multi_process");
    assert!(json["candidates"]
        .as_array()
        .is_some_and(|candidates| candidates
            .iter()
            .any(|candidate| candidate["detector"] == "artisan")));

    let mut test = cargo_bin_cmd!("dev");
    test.args(["test", "--quiet", "--at"])
        .arg(&test_file)
        .env("PATH", &bin);
    test.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<artisan>\narg1=<test>\narg2=<tests/Feature/AuthTest.php>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn composer_uses_an_explicit_project_local_test_runner() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("composer-tests");
    let runner = project.join("vendor/bin/pest");
    let target = project.join("tests/Feature/PaymentTest.php");
    fs::create_dir_all(runner.parent().unwrap_or(&project))?;
    fs::create_dir_all(target.parent().unwrap_or(&project))?;
    fs::write(project.join("composer.json"), r#"{"name":"acme/tests"}"#)?;
    fs::write(&target, "<?php\n")?;
    fs::write(
        &runner,
        "#!/bin/sh\nprintf 'runner=<pest>\\ncwd=<%s>\\narg0=<%s>\\n' \"$PWD\" \"$1\"\n",
    )?;
    let mut permissions = runner.metadata()?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&runner, permissions)?;

    let mut command = cargo_bin_cmd!("dev");
    command.args(["test", "--quiet", "--at"]).arg(&target);
    command.assert().success().stdout(format!(
        "runner=<pest>\ncwd=<{}>\narg0=<tests/Feature/PaymentTest.php>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn go_detector_targets_main_packages_without_invoking_go_list() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("go-project");
    fs::create_dir_all(project.join("cmd/worker"))?;
    fs::write(
        project.join("go.mod"),
        "module example.com/acme/tool\n\ngo 1.26\n",
    )?;
    fs::write(project.join("main.go"), "package main\nfunc main() {}\n")?;
    fs::write(
        project.join("cmd/worker/main.go"),
        "//go:build unix\n\npackage main\nfunc main() {}\n",
    )?;
    let bin = fake_program(temp.path(), "go")?;

    let mut root = cargo_bin_cmd!("dev");
    root.args(["run", "--quiet", "--at"])
        .arg(&project)
        .arg("tool")
        .env("PATH", &bin);
    root.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<.>\n",
        project.display()
    ));

    let mut worker = cargo_bin_cmd!("dev");
    worker
        .args(["run", "--quiet", "worker", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    worker.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<./cmd/worker>\n",
        project.display()
    ));

    let mut build = cargo_bin_cmd!("dev");
    build
        .args(["build", "--quiet", "worker", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    build.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<build>\narg1=<./cmd/worker>\n",
        project.display()
    ));

    let mut test = cargo_bin_cmd!("dev");
    test.args(["test", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    test.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<test>\narg1=<./...>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn go_test_binds_an_explicit_file_to_its_package() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("go-tests");
    let package = project.join("internal/auth");
    let target = package.join("auth_test.go");
    fs::create_dir_all(&package)?;
    fs::write(project.join("go.mod"), "module example.com/acme/tests\n")?;
    fs::write(package.join("auth.go"), "package auth\n")?;
    fs::write(&target, "package auth\n")?;
    let bin = fake_program(temp.path(), "go")?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["test", "--quiet", "--at"])
        .arg(&target)
        .env("PATH", &bin);
    command.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<test>\narg1=<./internal/auth>\n",
        project.display()
    ));

    let mut hinted = cargo_bin_cmd!("dev");
    hinted
        .args(["test", "--quiet", "auth", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    hinted.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<test>\narg1=<./internal/auth>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn go_work_discovers_a_deep_static_module_member() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let workspace = temp.path().join("go-workspace");
    let member = workspace.join("deep/services/runtime/worker");
    fs::create_dir_all(&member)?;
    fs::write(
        workspace.join("go.work"),
        "go 1.26\n\nuse ./deep/services/runtime/worker\n",
    )?;
    fs::write(member.join("go.mod"), "module example.com/acme/worker\n")?;
    fs::write(member.join("main.go"), "package main\nfunc main() {}\n")?;
    let bin = fake_program(temp.path(), "go")?;

    let mut command = cargo_bin_cmd!("dev");
    command
        .args(["run", "--quiet", "worker", "--at"])
        .arg(&workspace)
        .env("PATH", &bin);
    command.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<.>\n",
        member.display()
    ));
    Ok(())
}

#[test]
fn standalone_php_targets_use_the_interpreter_and_hint_widening() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("php-targets");
    let target = project.join("tools/legacy_importer.php");
    fs::create_dir_all(target.parent().unwrap_or(&project))?;
    fs::write(&target, "<?php echo 'ok';\n")?;
    let bin = fake_program(temp.path(), "php")?;

    let mut explicit = cargo_bin_cmd!("dev");
    explicit
        .args(["run", "--quiet", "--at"])
        .arg(&target)
        .env("PATH", &bin);
    explicit.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<legacy_importer.php>\n",
        target.parent().unwrap_or(&project).display()
    ));

    let mut hinted = cargo_bin_cmd!("dev");
    hinted
        .args(["run", "--quiet", "legacy", "importer", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    hinted.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<legacy_importer.php>\n",
        target.parent().unwrap_or(&project).display()
    ));
    Ok(())
}

#[test]
fn zig_build_steps_and_standalone_files_preserve_passthrough_placement() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("zig-project");
    fs::create_dir(&project)?;
    fs::write(
        project.join("build.zig"),
        "const std = @import(\"std\");\npub fn build(b: *std.Build) void { _ = b; }\n",
    )?;
    let standalone = project.join("tools/probe.zig");
    fs::create_dir_all(standalone.parent().unwrap_or(&project))?;
    fs::write(&standalone, "pub fn main() void {}\n")?;
    let bin = fake_program(temp.path(), "zig")?;

    let mut run = cargo_bin_cmd!("dev");
    run.args(["run", "--quiet", "--at"])
        .arg(&project)
        .args(["--", "alpha"])
        .env("PATH", &bin);
    run.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<build>\narg1=<run>\narg2=<-->\narg3=<alpha>\n",
        project.display()
    ));

    let mut build = cargo_bin_cmd!("dev");
    build
        .args(["build", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    build
        .assert()
        .success()
        .stdout(format!("cwd=<{}>\narg0=<build>\n", project.display()));

    let mut test = cargo_bin_cmd!("dev");
    test.args(["test", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    test.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<build>\narg1=<test>\n",
        project.display()
    ));

    let mut file = cargo_bin_cmd!("dev");
    file.args(["run", "--quiet", "--at"])
        .arg(&standalone)
        .env("PATH", &bin);
    file.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<probe.zig>\n",
        standalone.parent().unwrap_or(&project).display()
    ));
    Ok(())
}

#[test]
fn swiftpm_infers_only_conventional_executable_layouts() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("swift-project");
    fs::create_dir_all(project.join("Sources/Dealer"))?;
    fs::create_dir_all(project.join("Tests/DealerTests"))?;
    fs::write(
        project.join("Package.swift"),
        "// swift-tools-version: 6.1\n",
    )?;
    fs::write(
        project.join("Sources/Dealer/main.swift"),
        "print(\"cards\")\n",
    )?;
    let bin = fake_program(temp.path(), "swift")?;

    let mut run = cargo_bin_cmd!("dev");
    run.args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    run.assert()
        .success()
        .stdout(format!("cwd=<{}>\narg0=<run>\n", project.display()));

    let mut build = cargo_bin_cmd!("dev");
    build
        .args(["build", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    build
        .assert()
        .success()
        .stdout(format!("cwd=<{}>\narg0=<build>\n", project.display()));

    let mut test = cargo_bin_cmd!("dev");
    test.args(["test", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    test.assert()
        .success()
        .stdout(format!("cwd=<{}>\narg0=<test>\n", project.display()));

    fs::create_dir_all(project.join("Sources/Worker"))?;
    fs::write(
        project.join("Sources/Worker/main.swift"),
        "print(\"working\")\n",
    )?;
    let mut named_run = cargo_bin_cmd!("dev");
    named_run
        .args(["run", "worker", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    named_run.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<Worker>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn bare_xcode_project_explains_why_no_command_was_inferred() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("xcode-only");
    fs::create_dir_all(project.join("App.xcodeproj"))?;
    let bin = fake_program(temp.path(), "swift")?;

    let mut command = cargo_bin_cmd!("dev");
    let output = command
        .args(["run", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .output()?;
    assert_eq!(output.status.code(), Some(4));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert!(json["diagnostics"]
        .as_array()
        .is_some_and(|diagnostics| diagnostics.iter().any(|diagnostic| {
            diagnostic["detector"] == "swift"
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("scheme and destination"))
        })));
    Ok(())
}

#[test]
fn flutter_commands_include_device_warning_test_binding_and_host_availability() -> anyhow::Result<()>
{
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("flutter-project");
    let test_file = project.join("test/widget_test.dart");
    for directory in ["ios", "web", "windows"] {
        fs::create_dir_all(project.join(directory))?;
    }
    fs::create_dir_all(test_file.parent().unwrap_or(&project))?;
    fs::write(
        project.join("pubspec.yaml"),
        "name: flutter_probe\ndependencies:\n  flutter:\n    sdk: flutter\n",
    )?;
    fs::write(&test_file, "void main() {}\n")?;
    let bin = fake_program(temp.path(), "flutter")?;

    let mut run = cargo_bin_cmd!("dev");
    run.args(["run", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    let output = run.output()?;
    anyhow::ensure!(output.status.success(), "Flutter run failed: {output:?}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(json["candidates"][0]["lifecycle"], "long_running");
    assert!(json["candidates"][0]["description"]
        .as_str()
        .is_some_and(|description| description.contains("prompt")));

    let mut test = cargo_bin_cmd!("dev");
    test.args(["test", "--quiet", "--at"])
        .arg(&test_file)
        .env("PATH", &bin);
    test.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<test>\narg1=<test/widget_test.dart>\n",
        project.display()
    ));

    let mut builds = cargo_bin_cmd!("dev");
    let output = builds
        .args(["build", "--json", "--at"])
        .arg(&project)
        .env("PATH", &bin)
        .output()?;
    assert_eq!(output.status.code(), Some(5));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let windows = json["candidates"]
        .as_array()
        .and_then(|candidates| {
            candidates
                .iter()
                .find(|candidate| candidate["action_key"] == "flutter:flutter_probe:windows")
        })
        .ok_or_else(|| anyhow::anyhow!("missing Windows Flutter build"))?;
    if cfg!(target_os = "windows") {
        assert_eq!(windows["availability"]["status"], "available");
    } else {
        assert_eq!(windows["availability"]["status"], "unsupported_host");
    }
    Ok(())
}

#[test]
fn dart_package_and_explicit_test_file_use_current_cli_forms() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("dart-project");
    let test_file = project.join("test/parser_test.dart");
    fs::create_dir_all(project.join("bin"))?;
    fs::create_dir_all(test_file.parent().unwrap_or(&project))?;
    fs::write(project.join("pubspec.yaml"), "name: parser_tool\n")?;
    fs::write(project.join("bin/parser_tool.dart"), "void main() {}\n")?;
    fs::write(&test_file, "void main() {}\n")?;
    let bin = fake_program(temp.path(), "dart")?;

    let mut run = cargo_bin_cmd!("dev");
    run.args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    run.assert()
        .success()
        .stdout(format!("cwd=<{}>\narg0=<run>\n", project.display()));

    let mut test = cargo_bin_cmd!("dev");
    test.args(["test", "--quiet", "--at"])
        .arg(&test_file)
        .env("PATH", &bin);
    test.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<test>\narg1=<test/parser_test.dart>\n",
        project.display()
    ));

    let mut hinted_test = cargo_bin_cmd!("dev");
    hinted_test
        .args(["test", "parser", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    hinted_test.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<test>\narg1=<test/parser_test.dart>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn python_files_prefer_active_virtual_environment_then_python3() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("python-files");
    let script = project.join("tools/report.py");
    fs::create_dir_all(script.parent().unwrap_or(&project))?;
    fs::write(&script, "print('report')\n")?;

    let virtual_environment = temp.path().join("active-venv");
    let virtual_bin = fake_program(&virtual_environment, "python")?;
    let mut explicit = cargo_bin_cmd!("dev");
    explicit
        .args(["run", "--quiet", "--at"])
        .arg(&script)
        .env("VIRTUAL_ENV", &virtual_environment)
        .env("PATH", &virtual_bin);
    explicit.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<report.py>\n",
        script.parent().unwrap_or(&project).display()
    ));

    let python3_bin = fake_program(temp.path(), "python3")?;
    let mut hinted = cargo_bin_cmd!("dev");
    hinted
        .args(["run", "report", "--quiet", "--at"])
        .arg(&project)
        .env_remove("VIRTUAL_ENV")
        .env("PATH", &python3_bin);
    hinted.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<report.py>\n",
        script.parent().unwrap_or(&project).display()
    ));
    Ok(())
}

#[test]
fn make_scanner_maps_conventional_and_explicit_literal_targets() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("make-project");
    fs::create_dir_all(project.join("src"))?;
    fs::write(
        project.join("Makefile"),
        "## Local server\ndev: ## Serve locally\n\t@echo ignored\nbuild:\ntest:\ndeploy:\n%.o: %.c\n.PHONY: dev build test deploy\n",
    )?;
    let bin = fake_program(temp.path(), "make")?;

    for (intent, target) in [("run", "dev"), ("build", "build"), ("test", "test")] {
        let mut command = cargo_bin_cmd!("dev");
        command
            .args([intent, "--quiet", "--at"])
            .arg(project.join("src"))
            .env("PATH", &bin);
        command
            .assert()
            .success()
            .stdout(format!("cwd=<{}>\narg0=<{target}>\n", project.display()));
    }

    let mut deploy = cargo_bin_cmd!("dev");
    deploy
        .args(["run", "deploy", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    deploy
        .assert()
        .success()
        .stdout(format!("cwd=<{}>\narg0=<deploy>\n", project.display()));
    Ok(())
}

#[test]
fn compose_is_hint_only_but_service_identity_can_beat_native_default() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("compose-and-node");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"dev":"vite"}}"#,
    )?;
    fs::write(
        project.join("compose.yaml"),
        "services:\n  api:\n    image: example/api\n  database:\n    image: postgres\n",
    )?;
    fs::write(project.join("Dockerfile"), "FROM scratch\n")?;
    let bin = fake_program(temp.path(), "npm")?;
    fake_program(temp.path(), "docker")?;

    let mut unhinted = cargo_bin_cmd!("dev");
    unhinted
        .args(["run", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    unhinted.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<dev>\n",
        project.display()
    ));

    let mut service = cargo_bin_cmd!("dev");
    service
        .args(["run", "api", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    service.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<compose>\narg1=<up>\narg2=<api>\n",
        project.display()
    ));

    let mut build = cargo_bin_cmd!("dev");
    build
        .args(["build", "dockerfile", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    build.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<build>\narg1=<.>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn every_standard_compose_filename_is_recognized() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let bin = fake_program(temp.path(), "docker")?;
    for filename in [
        "compose.yml",
        "compose.yaml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ] {
        let project = temp.path().join(filename.replace('.', "-"));
        fs::create_dir(&project)?;
        fs::write(
            project.join(filename),
            "services:\n  web:\n    image: example/web\n",
        )?;
        let mut command = cargo_bin_cmd!("dev");
        let output = command
            .args(["run", "compose", "--json", "--at"])
            .arg(&project)
            .env("PATH", &bin)
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "{filename} was not recognized: {output:?}"
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert!(json["candidates"].as_array().is_some_and(|candidates| {
            candidates.iter().any(|candidate| {
                candidate["structural_evidence"]
                    .as_array()
                    .is_some_and(|evidence| {
                        evidence.iter().any(|item| {
                            item["source"]
                                .as_str()
                                .is_some_and(|source| source == filename)
                        })
                    })
            })
        }));
    }
    Ok(())
}

#[test]
fn shell_candidates_respect_shebang_permissions_and_conventional_names() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("shell-project");
    let scripts = project.join("scripts");
    fs::create_dir_all(&scripts)?;
    let interpreted = scripts.join("probe");
    fs::write(
        &interpreted,
        "#!/usr/bin/env bash -e\nprintf 'ignored\\n'\n",
    )?;
    let conventional = project.join("test.sh");
    fs::write(&conventional, "printf 'ignored\\n'\n")?;
    let executable = scripts.join("release");
    fs::write(&executable, "#!/bin/sh\nprintf 'release-ran\\n'\n")?;
    let mut permissions = executable.metadata()?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions)?;
    let bin = fake_program(temp.path(), "bash")?;
    fake_program(temp.path(), "sh")?;

    let mut shebang = cargo_bin_cmd!("dev");
    shebang
        .args(["run", "--quiet", "--at"])
        .arg(&interpreted)
        .env("PATH", &bin);
    shebang.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<-e>\narg1=<probe>\n",
        scripts.display()
    ));

    let mut test = cargo_bin_cmd!("dev");
    test.args(["test", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    test.assert()
        .success()
        .stdout(format!("cwd=<{}>\narg0=<test.sh>\n", project.display()));

    let mut release = cargo_bin_cmd!("dev");
    release
        .args(["run", "release", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    release.assert().success().stdout("release-ran\n");
    Ok(())
}

#[test]
fn doctor_reports_local_toolchains_without_treating_missing_tools_as_failure() -> anyhow::Result<()>
{
    let temp = tempfile::tempdir()?;
    let bin = fake_program(temp.path(), "npm")?;
    let mut command = cargo_bin_cmd!("dev");
    command.env("PATH", &bin).arg("doctor");
    command
        .assert()
        .success()
        .stdout(predicates::str::contains("dev doctor"))
        .stdout(predicates::str::contains("ok npm"))
        .stdout(predicates::str::contains("cargo     not found on PATH"));
    Ok(())
}

#[test]
fn node_test_provider_binds_explicit_and_hinted_test_files_once() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("node-tests");
    let test_file = project.join("tests/deep/participant-sync.test.js");
    fs::create_dir_all(test_file.parent().unwrap_or(&project))?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"test":"vitest"}}"#,
    )?;
    fs::write(&test_file, "export {};\n")?;
    let bin = fake_program(temp.path(), "npm")?;

    let mut explicit = cargo_bin_cmd!("dev");
    explicit
        .args(["test", "--quiet", "--at"])
        .arg(&test_file)
        .args(["--", "--watch"])
        .env("PATH", &bin);
    explicit.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<test>\narg2=<-->\narg3=<tests/deep/participant-sync.test.js>\narg4=<--watch>\n",
        project.display()
    ));

    let mut hinted = cargo_bin_cmd!("dev");
    hinted
        .args(["test", "participant-sync", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    hinted.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<test>\narg2=<-->\narg3=<tests/deep/participant-sync.test.js>\n",
        project.display()
    ));

    let mut generic = cargo_bin_cmd!("dev");
    generic
        .args(["test", "test", "--quiet", "--at"])
        .arg(&project)
        .env("PATH", &bin);
    generic.assert().success().stdout(format!(
        "cwd=<{}>\narg0=<run>\narg1=<test>\n",
        project.display()
    ));
    Ok(())
}

#[test]
fn empty_project_error_includes_scan_context_and_actionable_alternatives() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("empty-project");
    fs::create_dir(&project)?;
    let mut command = cargo_bin_cmd!("dev");
    command.args(["run", "--at"]).arg(&project);
    command
        .assert()
        .code(4)
        .stdout("")
        .stderr(predicates::str::contains("nothing runnable found for Run"))
        .stderr(predicates::str::contains("scanned:"))
        .stderr(predicates::str::contains("Try:"))
        .stderr(predicates::str::contains("dev test --at"));
    Ok(())
}

#[test]
fn hinted_preamble_names_decisive_match_and_honors_color_controls() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let project = temp.path().join("preamble-project");
    fs::create_dir(&project)?;
    fs::write(
        project.join("package.json"),
        r#"{"scripts":{"refresh-search-index":"ignored"}}"#,
    )?;
    let bin = fake_program(temp.path(), "npm")?;

    let run = |color: &str, no_color: bool| -> anyhow::Result<std::process::Output> {
        let mut command = cargo_bin_cmd!("dev");
        command
            .args(["run", "refresh-search-index", "--color", color, "--at"])
            .arg(&project)
            .env("PATH", &bin);
        if no_color {
            command.env("NO_COLOR", "1");
        } else {
            command.env_remove("NO_COLOR");
        }
        Ok(command.output()?)
    };

    let plain = run("never", false)?;
    anyhow::ensure!(plain.status.success(), "plain execution failed: {plain:?}");
    let plain_stderr = String::from_utf8(plain.stderr)?;
    assert!(plain_stderr.contains("matched: \"refresh-search-index\""));
    assert!(!plain_stderr.contains('\u{1b}'));

    let colored = run("always", false)?;
    anyhow::ensure!(
        colored.status.success(),
        "colored execution failed: {colored:?}"
    );
    assert!(colored
        .stderr
        .windows(5)
        .any(|window| window == b"\x1b[36m"));

    let suppressed = run("always", true)?;
    anyhow::ensure!(
        suppressed.status.success(),
        "NO_COLOR execution failed: {suppressed:?}"
    );
    assert!(!suppressed.stderr.contains(&0x1b));
    Ok(())
}
