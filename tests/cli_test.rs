#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;

fn fake_program(directory: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let bin = directory.join("bin");
    fs::create_dir_all(&bin)?;
    let program = bin.join(name);
    fs::write(
        &program,
        r#"#!/bin/sh
printf 'cwd=<%s>\n' "$PWD"
index=0
for argument in "$@"; do
  printf 'arg%s=<%s>\n' "$index" "$argument"
  index=$((index + 1))
done
"#,
    )?;
    let mut permissions = program.metadata()?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&program, permissions)?;
    Ok(bin)
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
