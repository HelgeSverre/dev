use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

use super::{state_file, CacheError, ChoiceStore, CACHE_SCHEMA};

const LOCK_TIMEOUT: Duration = Duration::from_millis(250);
static UNIQUE_FILE_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn update_store(update: impl FnOnce(&mut ChoiceStore)) -> Result<(), CacheError> {
    let state_file = state_file()?;
    let directory = state_file.parent().ok_or(CacheError::NoStateDirectory)?;
    prepare_directory(directory)?;
    let lock_path = directory.join("choices.lock");
    let lock = open_lock(&lock_path)?;
    acquire(&lock)?;
    sweep_temporary_files(directory)?;

    let mut store = load_locked(&state_file)?;
    update(&mut store);
    write_atomic(directory, &state_file, &store)?;
    FileExt::unlock(&lock).map_err(|source| CacheError::Io {
        path: lock_path,
        source,
    })
}

pub(super) fn quarantine_corrupt(path: &Path) -> Result<(), CacheError> {
    let directory = path.parent().ok_or(CacheError::NoStateDirectory)?;
    prepare_directory(directory)?;
    let lock_path = directory.join("choices.lock");
    let lock = open_lock(&lock_path)?;
    acquire(&lock)?;
    if std::fs::read(path)
        .ok()
        .is_some_and(|contents| serde_json::from_slice::<ChoiceStore>(&contents).is_err())
    {
        let _ = quarantine_locked(path);
    }
    FileExt::unlock(&lock).map_err(|source| CacheError::Io {
        path: lock_path,
        source,
    })
}

fn prepare_directory(directory: &Path) -> Result<(), CacheError> {
    std::fs::create_dir_all(directory).map_err(|source| CacheError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    secure_directory(directory)
}

fn open_lock(path: &Path) -> Result<File, CacheError> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path).map_err(|source| CacheError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    secure_file(&file, path)?;
    Ok(file)
}

fn acquire(lock: &File) -> Result<(), CacheError> {
    let started = Instant::now();
    loop {
        match FileExt::try_lock(lock) {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if started.elapsed() < LOCK_TIMEOUT => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => return Err(CacheError::LockTimeout),
            Err(TryLockError::Error(source)) => {
                return Err(CacheError::Io {
                    path: super::state_file()?,
                    source,
                });
            }
        }
    }
}

fn load_locked(path: &Path) -> Result<ChoiceStore, CacheError> {
    secure_existing_file(path)?;
    match std::fs::read(path) {
        Ok(contents) => match serde_json::from_slice::<ChoiceStore>(&contents) {
            Ok(store) if store.schema_version == CACHE_SCHEMA => Ok(store),
            Ok(_) => Ok(empty_store()),
            Err(error) => {
                eprintln!(
                    "dev: warning: ignored corrupt cache `{}`: {error}",
                    path.display()
                );
                let _ = quarantine_locked(path);
                Ok(empty_store())
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(empty_store()),
        Err(source) => Err(CacheError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
pub(super) fn secure_existing_file(path: &Path) -> Result<(), CacheError> {
    match File::open(path) {
        Ok(file) => secure_file(&file, path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CacheError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(not(unix))]
pub(super) fn secure_existing_file(_path: &Path) -> Result<(), CacheError> {
    Ok(())
}

fn write_atomic(directory: &Path, path: &Path, store: &ChoiceStore) -> Result<(), CacheError> {
    let (file, mut temporary) = create_temporary(directory)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, store)?;
    writer.write_all(b"\n").map_err(|source| CacheError::Io {
        path: temporary.path.clone(),
        source,
    })?;
    writer.flush().map_err(|source| CacheError::Io {
        path: temporary.path.clone(),
        source,
    })?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|source| CacheError::Io {
            path: temporary.path.clone(),
            source,
        })?;
    std::fs::rename(&temporary.path, path).map_err(|source| CacheError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    temporary.keep = true;
    sync_directory(directory)
}

fn create_temporary(directory: &Path) -> Result<(File, PendingTemporary), CacheError> {
    loop {
        let unique = UNIQUE_FILE_ID.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("choices.{}.{unique}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&path) {
            Ok(file) => return Ok((file, PendingTemporary { path, keep: false })),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(CacheError::Io { path, source }),
        }
    }
}

fn sweep_temporary_files(directory: &Path) -> Result<(), CacheError> {
    let entries = std::fs::read_dir(directory).map_err(|source| CacheError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("choices.") && name.ends_with(".tmp") {
            let path = entry.path();
            if path.is_file() {
                std::fs::remove_file(&path).map_err(|source| CacheError::Io { path, source })?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn secure_directory(directory: &Path) -> Result<(), CacheError> {
    let mut permissions = std::fs::metadata(directory)
        .map_err(|source| CacheError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(directory, permissions).map_err(|source| CacheError::Io {
        path: directory.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn secure_directory(_directory: &Path) -> Result<(), CacheError> {
    Ok(())
}

#[cfg(unix)]
fn secure_file(file: &File, path: &Path) -> Result<(), CacheError> {
    let mut permissions = file
        .metadata()
        .map_err(|source| CacheError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_mode(0o600);
    file.set_permissions(permissions)
        .map_err(|source| CacheError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn secure_file(_file: &File, _path: &Path) -> Result<(), CacheError> {
    Ok(())
}

fn quarantine_locked(path: &Path) -> Result<(), CacheError> {
    let unique = UNIQUE_FILE_ID.fetch_add(1, Ordering::Relaxed);
    let quarantine = path.with_extension(format!("corrupt-{}-{unique}.json", super::now_millis()));
    std::fs::rename(path, &quarantine).map_err(|source| CacheError::Io {
        path: quarantine,
        source,
    })
}

fn empty_store() -> ChoiceStore {
    ChoiceStore {
        schema_version: CACHE_SCHEMA,
        entries: Vec::new(),
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), CacheError> {
    File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| CacheError::Io {
            path: directory.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> Result<(), CacheError> {
    Ok(())
}

struct PendingTemporary {
    path: std::path::PathBuf,
    keep: bool,
}

impl Drop for PendingTemporary {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrupt_store_is_quarantined_before_returning_empty_state() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let path = temporary.path().join("choices.json");
        std::fs::write(&path, "{not-json")?;

        let store = load_locked(&path)?;

        assert!(store.entries.is_empty());
        assert!(!path.exists());
        let quarantined = std::fs::read_dir(temporary.path())?
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("choices.corrupt-") && name.ends_with(".json")
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert_eq!(quarantined.len(), 1);
        #[cfg(unix)]
        assert_eq!(
            std::fs::metadata(&quarantined[0])?.permissions().mode() & 0o777,
            0o600
        );
        Ok(())
    }

    #[test]
    fn orphaned_atomic_temps_are_swept_without_touching_other_files() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let stale = temporary.path().join("choices.123.0.tmp");
        let unrelated = temporary.path().join("choices.keep");
        std::fs::write(&stale, "partial")?;
        std::fs::write(&unrelated, "keep")?;

        sweep_temporary_files(temporary.path())?;

        assert!(!stale.exists());
        assert!(unrelated.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn state_directory_and_files_are_private() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let directory = temporary.path().join("dev");
        prepare_directory(&directory)?;
        let lock_path = directory.join("choices.lock");
        let lock = open_lock(&lock_path)?;
        let store_path = directory.join("choices.json");
        write_atomic(&directory, &store_path, &empty_store())?;

        assert_eq!(
            std::fs::metadata(&directory)?.permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(lock.metadata()?.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(&store_path)?.permissions().mode() & 0o777,
            0o600
        );
        Ok(())
    }
}
