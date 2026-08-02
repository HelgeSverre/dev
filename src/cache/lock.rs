use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};

use super::{state_file, CacheError, ChoiceStore, CACHE_SCHEMA};

const LOCK_TIMEOUT: Duration = Duration::from_millis(250);
static UNIQUE_FILE_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn update_store(update: impl FnOnce(&mut ChoiceStore)) -> Result<(), CacheError> {
    let state_file = state_file()?;
    let directory = state_file.parent().ok_or(CacheError::NoStateDirectory)?;
    std::fs::create_dir_all(directory).map_err(|source| CacheError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let lock_path = directory.join("choices.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|source| CacheError::Io {
            path: lock_path.clone(),
            source,
        })?;
    acquire(&lock)?;

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
    let lock_path = directory.join("choices.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|source| CacheError::Io {
            path: lock_path.clone(),
            source,
        })?;
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
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => return Ok((file, PendingTemporary { path, keep: false })),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(CacheError::Io { path, source }),
        }
    }
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
