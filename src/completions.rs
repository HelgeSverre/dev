use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::cli::ShellChoice;
use crate::cli::write_completions;

/// Details about an installed completion script.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Installation {
    pub shell_name: &'static str,
    pub path: PathBuf,
    pub hint: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(
        "could not detect the current shell from $SHELL; pass one explicitly, for example `dev completions zsh --install`"
    )]
    ShellDetection,
    #[error("could not determine the user's home directory")]
    HomeDirectory,
    #[error(
        "automatic PowerShell completion installation is not supported; run `dev completions powershell >> $PROFILE` from PowerShell"
    )]
    PowerShellProfile,
    #[error("creating completion directory `{path}` failed: {source}")]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("writing completion script `{path}` failed: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Install a completion script for an explicit shell or the shell named by `$SHELL`.
pub fn install(shell: Option<ShellChoice>) -> Result<Installation, InstallError> {
    let shell = shell
        .or_else(|| {
            std::env::var_os("SHELL")
                .as_deref()
                .and_then(shell_from_program)
        })
        .ok_or(InstallError::ShellDetection)?;
    let home = directories::BaseDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .ok_or(InstallError::HomeDirectory)?;
    let config =
        absolute_environment_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
    let data =
        absolute_environment_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
    let zsh = absolute_environment_path("ZDOTDIR").unwrap_or_else(|| home.join(".zsh"));

    install_at(shell, &config, &data, &zsh)
}

fn absolute_environment_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn install_at(
    shell: ShellChoice,
    config: &Path,
    data: &Path,
    zsh: &Path,
) -> Result<Installation, InstallError> {
    let (shell_name, path, hint) = match shell {
        ShellChoice::Bash => ("bash", data.join("bash-completion/completions/dev"), None),
        ShellChoice::Zsh => (
            "zsh",
            zsh.join("completions/_dev"),
            Some(format!(
                "Ensure {} is in fpath before compinit: fpath=({} $fpath)",
                zsh.join("completions").display(),
                zsh.join("completions").display()
            )),
        ),
        ShellChoice::Fish => ("fish", config.join("fish/completions/dev.fish"), None),
        ShellChoice::Elvish => (
            "elvish",
            config.join("elvish/lib/dev.elv"),
            Some(format!(
                "Add `use dev` to {} to load the completions",
                config.join("elvish/rc.elv").display()
            )),
        ),
        ShellChoice::PowerShell => return Err(InstallError::PowerShellProfile),
        ShellChoice::Nushell => ("nushell", config.join("nushell/completions/dev.nu"), None),
    };

    let parent = path.parent().ok_or(InstallError::HomeDirectory)?;
    std::fs::create_dir_all(parent).map_err(|source| InstallError::CreateDirectory {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut output = Vec::new();
    write_completions(shell, &mut output);
    std::fs::write(&path, output).map_err(|source| InstallError::Write {
        path: path.clone(),
        source,
    })?;

    Ok(Installation {
        shell_name,
        path,
        hint,
    })
}

fn shell_from_program(program: &OsStr) -> Option<ShellChoice> {
    let name = Path::new(program).file_name()?.to_str()?;
    if name.eq_ignore_ascii_case("bash") {
        Some(ShellChoice::Bash)
    } else if name.eq_ignore_ascii_case("zsh") {
        Some(ShellChoice::Zsh)
    } else if name.eq_ignore_ascii_case("fish") {
        Some(ShellChoice::Fish)
    } else if name.eq_ignore_ascii_case("elvish") {
        Some(ShellChoice::Elvish)
    } else if name.eq_ignore_ascii_case("pwsh")
        || name.eq_ignore_ascii_case("powershell")
        || name.eq_ignore_ascii_case("powershell.exe")
    {
        Some(ShellChoice::PowerShell)
    } else if name.eq_ignore_ascii_case("nu") {
        Some(ShellChoice::Nushell)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_shell_program_names() {
        assert_eq!(
            shell_from_program(OsStr::new("/bin/bash")),
            Some(ShellChoice::Bash)
        );
        assert_eq!(
            shell_from_program(OsStr::new("/opt/homebrew/bin/zsh")),
            Some(ShellChoice::Zsh)
        );
        assert_eq!(shell_from_program(OsStr::new("fish")), Some(ShellChoice::Fish));
        assert_eq!(
            shell_from_program(OsStr::new("pwsh")),
            Some(ShellChoice::PowerShell)
        );
        assert_eq!(shell_from_program(OsStr::new("nu")), Some(ShellChoice::Nushell));
    }

    #[test]
    fn installs_supported_shells_in_user_local_directories() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let config = temporary.path().join("config");
        let data = temporary.path().join("data");
        let zsh = temporary.path().join("zsh");
        let cases = [
            (
                ShellChoice::Bash,
                "bash",
                data.join("bash-completion/completions/dev"),
            ),
            (ShellChoice::Zsh, "zsh", zsh.join("completions/_dev")),
            (
                ShellChoice::Fish,
                "fish",
                config.join("fish/completions/dev.fish"),
            ),
            (ShellChoice::Elvish, "elvish", config.join("elvish/lib/dev.elv")),
            (
                ShellChoice::Nushell,
                "nushell",
                config.join("nushell/completions/dev.nu"),
            ),
        ];

        for (shell, shell_name, expected_path) in cases {
            let installation = install_at(shell, &config, &data, &zsh)?;
            assert_eq!(installation.shell_name, shell_name);
            assert_eq!(installation.path, expected_path);
            let contents = std::fs::read(&installation.path)?;
            assert!(!contents.is_empty());
            assert!(contents.windows(3).any(|window| window == b"dev"));
        }
        Ok(())
    }

    #[test]
    fn powershell_install_requires_its_profile_path() {
        let error = install_at(
            ShellChoice::PowerShell,
            Path::new("config"),
            Path::new("data"),
            Path::new("zsh"),
        )
        .expect_err("PowerShell installation should require a manual profile path");
        assert!(matches!(error, InstallError::PowerShellProfile));
    }
}
