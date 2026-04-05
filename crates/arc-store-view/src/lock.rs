//! Lock-file based crash-consistent persistence primitives.
//!
//! This module implements native lock-file techniques inspired by robust VCS
//! reference stores. It provides two complementary guards:
//!
//! 1. [`LockMarker`] for mutual exclusion on metadata publication.
//! 2. [`LockFile`] for write-then-commit resource updates.
//!
//! # Drop-guard semantics
//!
//! Both guards are RAII resources:
//!
//! - If a guard is dropped before commit/release, its lock file is removed.
//! - If a process exits unexpectedly, stale lock cleanup on next acquire
//!   allows bounded recovery.
//!
//! # Crash-consistency guarantees
//!
//! [`LockFile::commit`] guarantees:
//!
//! - bytes are fully written and fsynced to the lock file,
//! - publish occurs via atomic rename semantics,
//! - parent directory is synced on supported platforms.
//!
//! This prevents partial-state publication for mutable pointer files.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime};

use tracing::instrument;

use crate::tempfile as temp_registry;

const MAX_LOCK_ATTEMPTS: usize = 512;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(2);
const STALE_LOCK_TTL: Duration = Duration::from_secs(30);

/// Mutual-exclusion lock marker.
///
/// Acquire with [`LockMarker::acquire`]. The lock file is deleted on drop.
pub struct LockMarker {
    path: PathBuf,
    registry_id: Option<usize>,
    released: bool,
}

impl LockMarker {
    /// Acquire an exclusive marker lock at `path`.
    ///
    /// The lock is represented by a `create_new` file creation and therefore
    /// cannot be simultaneously acquired by competing writers.
    #[instrument(skip_all)]
    pub fn acquire(path: &Path) -> std::io::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("lock path has no parent: {}", path.display()),
            )
        })?;
        fs::create_dir_all(parent)?;

        for _ in 0..MAX_LOCK_ATTEMPTS {
            match OpenOptions::new().create_new(true).write(true).open(path) {
                Ok(mut file) => {
                    let _ = writeln!(file, "pid={}", std::process::id());
                    let _ = file.sync_all();
                    let registry_id = register_temp(path.to_path_buf());
                    return Ok(Self {
                        path: path.to_path_buf(),
                        registry_id,
                        released: false,
                    });
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if can_reap_marker(path)? {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(err) => return Err(err),
            }
        }

        Err(std::io::Error::new(
            ErrorKind::TimedOut,
            format!("timed out acquiring lock {}", path.display()),
        ))
    }

    /// Explicitly release the marker lock.
    #[instrument(skip_all)]
    pub fn release(mut self) -> std::io::Result<()> {
        if !self.released {
            self.released = true;
            deregister_temp(self.registry_id);
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }
}

impl Drop for LockMarker {
    fn drop(&mut self) {
        if !self.released {
            deregister_temp(self.registry_id);
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Lock-file writer for atomic update of one target file.
///
/// Write bytes into the `.lock` path, then call [`LockFile::commit`] to
/// publish atomically.
pub struct LockFile {
    target_path: PathBuf,
    lock_path: PathBuf,
    owner_path: PathBuf,
    registry_id: Option<usize>,
    file: Option<File>,
    committed: bool,
}

impl LockFile {
    /// Acquire a lock file for writing a future update to `target_path`.
    #[instrument(skip_all)]
    pub fn acquire_for_update(target_path: &Path) -> std::io::Result<Self> {
        let parent = target_path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("target path has no parent: {}", target_path.display()),
            )
        })?;
        fs::create_dir_all(parent)?;

        let lock_path = lock_path_for(target_path);
        let owner_path = owner_path_for(&lock_path);
        for _ in 0..MAX_LOCK_ATTEMPTS {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&lock_path)
            {
                Ok(file) => {
                    if let Err(err) = write_owner_file(&owner_path) {
                        let _ = fs::remove_file(&lock_path);
                        return Err(err);
                    }
                    let registry_id = register_temp(lock_path.clone());
                    return Ok(Self {
                        target_path: target_path.to_path_buf(),
                        lock_path,
                        owner_path,
                        registry_id,
                        file: Some(file),
                        committed: false,
                    });
                }
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    if can_reap_lock(&lock_path, &owner_path)? {
                        let _ = fs::remove_file(&lock_path);
                        let _ = fs::remove_file(&owner_path);
                        continue;
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(err) => return Err(err),
            }
        }

        Err(std::io::Error::new(
            ErrorKind::TimedOut,
            format!(
                "timed out acquiring lock file for {}",
                target_path.display()
            ),
        ))
    }

    /// Write full payload bytes into the lock file.
    #[instrument(skip_all)]
    pub fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("lock file is not open for writing"))?;
        file.write_all(bytes)
    }

    /// Fsync and atomically publish the lock file into the target path.
    #[instrument(skip_all)]
    pub fn commit(mut self) -> std::io::Result<()> {
        if let Some(file) = self.file.as_mut() {
            file.sync_all()?;
        }
        let _ = self.file.take();

        publish_lockfile(&self.lock_path, &self.target_path)?;
        let _ = fs::remove_file(&self.owner_path);
        deregister_temp(self.registry_id);
        if let Some(parent) = self.target_path.parent() {
            sync_directory(parent)?;
        }
        self.committed = true;
        Ok(())
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.file.take();
            deregister_temp(self.registry_id);
            let _ = fs::remove_file(&self.lock_path);
            let _ = fs::remove_file(&self.owner_path);
        }
    }
}

#[cfg(not(test))]
fn register_temp(path: PathBuf) -> Option<usize> {
    Some(temp_registry::register(path))
}

#[cfg(test)]
fn register_temp(_path: PathBuf) -> Option<usize> {
    None
}

fn deregister_temp(id: Option<usize>) {
    if let Some(id) = id {
        temp_registry::deregister(id);
    }
}

fn lock_path_for(target_path: &Path) -> PathBuf {
    let file_name = target_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("state");
    target_path.with_file_name(format!("{file_name}.lock"))
}

fn owner_path_for(lock_path: &Path) -> PathBuf {
    let file_name = lock_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("state.lock");
    lock_path.with_file_name(format!("{file_name}.owner"))
}

fn write_owner_file(path: &Path) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    writeln!(file, "pid={}", std::process::id())?;
    file.sync_all()
}

fn can_reap_marker(marker_path: &Path) -> std::io::Result<bool> {
    let meta = match fs::metadata(marker_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let modified = match meta.modified() {
        Ok(ts) => ts,
        Err(_) => return Ok(false),
    };
    let age = match SystemTime::now().duration_since(modified) {
        Ok(d) => d,
        Err(_) => Duration::ZERO,
    };
    if age <= STALE_LOCK_TTL {
        return Ok(false);
    }

    let owner_pid = read_pid(marker_path)?;
    Ok(owner_pid.is_some_and(|pid| !is_pid_alive(pid)))
}

fn can_reap_lock(lock_path: &Path, owner_path: &Path) -> std::io::Result<bool> {
    let lock_meta = match fs::metadata(lock_path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    let modified = match lock_meta.modified() {
        Ok(ts) => ts,
        Err(_) => return Ok(false),
    };
    let age = match SystemTime::now().duration_since(modified) {
        Ok(d) => d,
        Err(_) => Duration::ZERO,
    };
    if age <= STALE_LOCK_TTL {
        return Ok(false);
    }

    let owner_pid = read_pid(owner_path)?;
    Ok(owner_pid.is_some_and(|pid| !is_pid_alive(pid)))
}

fn read_pid(path: &Path) -> std::io::Result<Option<u32>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    let pid = content
        .lines()
        .find_map(|line| line.strip_prefix("pid="))
        .and_then(|value| value.trim().parse::<u32>().ok());
    Ok(pid)
}

fn is_pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let filter = format!("PID eq {pid}");
        let output = Command::new("tasklist")
            .args(["/FI", &filter, "/NH"])
            .output();
        match output {
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                text.contains(&pid.to_string()) && !text.contains("No tasks are running")
            }
            Err(_) => true,
        }
    }

    #[cfg(not(windows))]
    {
        let status = Command::new("kill").arg("-0").arg(pid.to_string()).status();
        match status {
            Ok(code) => code.success(),
            Err(_) => true,
        }
    }
}

fn publish_lockfile(lock_path: &Path, target_path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if target_path.exists() {
            let file_name = target_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("state");
            let backup_path = target_path.with_file_name(format!("{file_name}.bak"));
            let staged_backup_path = target_path.with_file_name(format!("{file_name}.bak.new"));

            if staged_backup_path.exists() {
                let _ = fs::remove_file(&staged_backup_path);
            }

            fs::rename(target_path, &staged_backup_path)?;

            if let Err(err) = fs::rename(lock_path, target_path) {
                let _ = fs::rename(&staged_backup_path, target_path);
                return Err(err);
            }

            if backup_path.exists() {
                let _ = fs::remove_file(&backup_path);
            }
            let _ = fs::rename(&staged_backup_path, &backup_path);
            return Ok(());
        }
        fs::rename(lock_path, target_path)
    }

    #[cfg(not(windows))]
    {
        fs::rename(lock_path, target_path)
    }
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x02000000;
        match OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
        {
            Ok(file) => match file.sync_all() {
                Ok(()) => Ok(()),
                Err(err) if err.kind() == ErrorKind::PermissionDenied => Ok(()),
                Err(err) => Err(err),
            },
            Err(err) if err.kind() == ErrorKind::PermissionDenied => Ok(()),
            Err(err) => Err(err),
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{LockFile, LockMarker};

    #[test]
    fn lock_file_drop_cleans_staged_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("state.bin");

        {
            let mut lock = LockFile::acquire_for_update(&target).expect("acquire lock file");
            lock.write_all(b"abc").expect("write");
        }

        assert!(
            !dir.path().join("state.bin.lock").exists(),
            "drop should clean staged lock file"
        );
        assert!(
            !target.exists(),
            "target should not be published without commit"
        );
    }

    #[test]
    fn lock_file_commit_publishes_atomically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("state.bin");

        let mut lock = LockFile::acquire_for_update(&target).expect("acquire lock file");
        lock.write_all(b"abc").expect("write");
        lock.commit().expect("commit");

        let got = std::fs::read(&target).expect("read target");
        assert_eq!(got, b"abc");
        assert!(
            !dir.path().join("state.bin.lock").exists(),
            "commit should remove lock file"
        );
    }

    #[test]
    fn marker_release_removes_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock_path = dir.path().join("publish.lock");

        let lock = LockMarker::acquire(&lock_path).expect("acquire marker");
        assert!(lock_path.exists());
        lock.release().expect("release marker");
        assert!(!lock_path.exists());
    }
}
