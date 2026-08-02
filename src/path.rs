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
        return if let Some(resolved_program) = resolve_executable(&resolved, env_delta) {
            Availability::Available { resolved_program }
        } else {
            Availability::MissingProgram {
                program: program.to_os_string(),
            }
        };
    }

    let effective_path = env_value(env_delta, OsStr::new("PATH"))
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
        .find_map(|candidate| resolve_executable(&candidate, env_delta))
        .map_or_else(
            || Availability::MissingProgram {
                program: program.to_os_string(),
            },
            |resolved_program| Availability::Available { resolved_program },
        )
}

#[cfg(not(windows))]
fn env_value<'a>(
    env_delta: &'a std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    key: &OsStr,
) -> Option<&'a std::ffi::OsString> {
    env_delta.get(key)
}

#[cfg(windows)]
fn env_value<'a>(
    env_delta: &'a std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
    key: &OsStr,
) -> Option<&'a std::ffi::OsString> {
    let key = key.to_string_lossy();
    env_delta
        .iter()
        .find(|(candidate, _)| candidate.to_string_lossy().eq_ignore_ascii_case(&key))
        .map(|(_, value)| value)
}

#[cfg(unix)]
fn resolve_executable(
    path: &Path,
    _env_delta: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .then(|| path.to_path_buf())
}

#[cfg(windows)]
fn resolve_executable(
    path: &Path,
    env_delta: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.extension().is_some() {
        return None;
    }
    windows_executable_extensions(env_delta)
        .into_iter()
        .map(|extension| path.with_extension(extension))
        .find(|candidate| candidate.is_file())
}

#[cfg(not(any(unix, windows)))]
fn resolve_executable(
    path: &Path,
    _env_delta: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Option<PathBuf> {
    path.is_file().then(|| path.to_path_buf())
}

#[cfg(windows)]
fn windows_executable_extensions(
    env_delta: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Vec<std::ffi::OsString> {
    let configured = env_value(env_delta, OsStr::new("PATHEXT"))
        .cloned()
        .or_else(|| env::var_os("PATHEXT"));
    configured
        .as_deref()
        .map(parse_windows_executable_extensions)
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| parse_windows_executable_extensions(OsStr::new(".COM;.EXE;.BAT;.CMD")))
}

#[cfg(any(windows, test))]
fn parse_windows_executable_extensions(value: &OsStr) -> Vec<std::ffi::OsString> {
    value
        .to_string_lossy()
        .split(';')
        .map(str::trim)
        .map(|extension| extension.trim_start_matches('.'))
        .filter(|extension| !extension.is_empty())
        .map(std::ffi::OsString::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    #[cfg(unix)]
    use std::collections::BTreeMap;

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

    #[test]
    fn windows_pathext_parser_preserves_search_order() {
        assert_eq!(
            parse_windows_executable_extensions(OsStr::new(".CMD; .EXE;;BAT")),
            [
                OsString::from("CMD"),
                OsString::from("EXE"),
                OsString::from("BAT")
            ]
        );
    }
}
