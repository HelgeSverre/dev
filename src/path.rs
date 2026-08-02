use std::env;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::candidate::Availability;

/// Return an absolute logical path without resolving symlinks.
pub fn logical_absolute(base: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    normalize_lexically(&joined)
}

fn normalize_lexically(path: &Path) -> std::io::Result<PathBuf> {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !output.pop() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "path escapes its filesystem root",
                    ));
                }
            }
            other => output.push(other.as_os_str()),
        }
    }
    Ok(output)
}

/// Resolve a program against the candidate's effective `PATH` without running it.
#[must_use]
pub fn resolve_program(
    program: &OsStr,
    cwd: &Path,
    env_delta: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Availability {
    let path = Path::new(program);
    if path.components().count() > 1 || path.is_absolute() {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        };
        return if is_executable(&resolved) {
            Availability::Available {
                resolved_program: resolved,
            }
        } else {
            Availability::MissingProgram {
                program: program.to_os_string(),
            }
        };
    }

    let effective_path = env_delta
        .get(OsStr::new("PATH"))
        .cloned()
        .or_else(|| env::var_os("PATH"));
    let Some(effective_path) = effective_path else {
        return Availability::MissingProgram {
            program: program.to_os_string(),
        };
    };
    env::split_paths(&effective_path)
        .map(|directory| {
            if directory.is_absolute() {
                directory.join(program)
            } else {
                cwd.join(directory).join(program)
            }
        })
        .find(|candidate| is_executable(candidate))
        .map_or_else(
            || Availability::MissingProgram {
                program: program.to_os_string(),
            },
            |resolved_program| Availability::Available { resolved_program },
        )
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    if path.is_file() {
        return true;
    }
    ["exe", "cmd", "bat", "com"]
        .iter()
        .any(|extension| path.with_extension(extension).is_file())
}

#[cfg(not(any(unix, windows)))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn lexical_absolute_preserves_symlink_spelling() -> anyhow::Result<()> {
        let value = logical_absolute(Path::new("/tmp/project"), Path::new("./src/../app"))?;
        assert_eq!(value, Path::new("/tmp/project/app"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn relative_path_entries_resolve_from_candidate_cwd() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        std::fs::create_dir(temp.path().join("bin"))?;
        let program = temp.path().join("bin/probe");
        std::fs::write(&program, "#!/bin/sh\n")?;
        let mut permissions = program.metadata()?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&program, permissions)?;
        let env = BTreeMap::from([(OsString::from("PATH"), OsString::from("bin"))]);
        assert_eq!(
            resolve_program(OsStr::new("probe"), temp.path(), &env),
            Availability::Available {
                resolved_program: program
            }
        );
        Ok(())
    }
}
