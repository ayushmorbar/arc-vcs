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

use tracing::{debug, instrument, warn};

use crate::tempfile as temp_registry;

const MAX_LOCK_ATTEMPTS: usize = 512;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(2);
const STALE_LOCK_TTL: Duration = Duration::from_secs(30);
const CREATE_DIR_RETRY_LIMIT: usize = 6;
const CREATE_DIR_RETRY_DELAY: Duration = Duration::from_millis(2);

/// Configure lock acquisition behavior when contention is detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockFailMode {
    /// Fail immediately if lock is not currently available.
    Immediately,
    /// Retry with fixed backoff until timeout budget is exceeded.
    AfterDurationWithBackoff(Duration),
}

impl Default for LockFailMode {
    fn default() -> Self {
        Self::AfterDurationWithBackoff(default_lock_timeout())
    }
}

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
        Self::acquire_with(path, LockFailMode::AfterDurationWithBackoff(default_lock_timeout()))
    }

    /// Acquire a marker lock using explicit contention behavior.
    #[instrument(skip_all)]
    pub fn acquire_with(path: &Path, mode: LockFailMode) -> std::io::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("lock path has no parent: {}", path.display()),
            )
        })?;
        create_dir_all_retry(parent)?;

        let deadline = deadline_for_mode(mode);
        let mut attempts = 0usize;
        loop {
            attempts += 1;
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
                    if should_fail_now(mode, deadline) {
                        break;
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(err) => return Err(err),
            }

            if attempts >= MAX_LOCK_ATTEMPTS && should_fail_now(mode, deadline) {
                break;
            }
        }

        Err(std::io::Error::new(
            ErrorKind::TimedOut,
            format!(
                "timed out acquiring lock {} after {} attempt(s)",
                path.display(),
                attempts
            ),
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
        Self::acquire_for_update_with(
            target_path,
            LockFailMode::AfterDurationWithBackoff(default_lock_timeout()),
        )
    }

    /// Acquire a lock file using explicit contention behavior.
    #[instrument(skip_all)]
    pub fn acquire_for_update_with(target_path: &Path, mode: LockFailMode) -> std::io::Result<Self> {
        let parent = target_path.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("target path has no parent: {}", target_path.display()),
            )
        })?;
        create_dir_all_retry(parent)?;

        let lock_path = lock_path_for(target_path);
        let owner_path = owner_path_for(&lock_path);
        let deadline = deadline_for_mode(mode);
        let mut attempts = 0usize;
        loop {
            attempts += 1;
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
                    if should_fail_now(mode, deadline) {
                        break;
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(err) => return Err(err),
            }

            if attempts >= MAX_LOCK_ATTEMPTS && should_fail_now(mode, deadline) {
                break;
            }
        }

        Err(std::io::Error::new(
            ErrorKind::TimedOut,
            format!(
                "timed out acquiring lock file for {} after {} attempt(s)",
                target_path.display(),
                attempts
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

fn create_dir_all_retry(parent: &Path) -> std::io::Result<()> {
    for attempt in 0..CREATE_DIR_RETRY_LIMIT {
        match fs::create_dir_all(parent) {
            Ok(()) => return Ok(()),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::NotFound
                ) && attempt + 1 < CREATE_DIR_RETRY_LIMIT =>
            {
                debug!(
                    path = %parent.display(),
                    attempt = attempt + 1,
                    max_attempts = CREATE_DIR_RETRY_LIMIT,
                    kind = %err.kind(),
                    "retrying directory creation for lock state"
                );
                thread::sleep(CREATE_DIR_RETRY_DELAY);
            }
            Err(err) => {
                warn!(
                    path = %parent.display(),
                    attempt = attempt + 1,
                    max_attempts = CREATE_DIR_RETRY_LIMIT,
                    kind = %err.kind(),
                    "failed to create lock parent directory"
                );
                return Err(err);
            }
        }
    }

    Err(std::io::Error::other(
        "exhausted lock parent directory creation retries",
    ))
}

fn default_lock_timeout() -> Duration {
    Duration::from_millis((MAX_LOCK_ATTEMPTS as u64) * (LOCK_RETRY_DELAY.as_millis() as u64))
}

fn deadline_for_mode(mode: LockFailMode) -> Option<SystemTime> {
    match mode {
        LockFailMode::Immediately => None,
        LockFailMode::AfterDurationWithBackoff(timeout) => {
            let now = SystemTime::now();
            Some(now.checked_add(timeout).unwrap_or(now))
        }
    }
}

fn should_fail_now(mode: LockFailMode, deadline: Option<SystemTime>) -> bool {
    match mode {
        LockFailMode::Immediately => true,
        LockFailMode::AfterDurationWithBackoff(_) => {
            deadline.is_some_and(|limit| SystemTime::now().duration_since(limit).is_ok())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

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

    #[test]
    fn concurrent_parent_creation_is_race_tolerant() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().join("nested").join("shared").join("locks");
        let barrier = Arc::new(Barrier::new(2));

        let mut handles = Vec::new();
        for idx in 0..2 {
            let barrier = Arc::clone(&barrier);
            let lock_path = parent.join(format!("lock-{idx}.marker"));
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let lock = LockMarker::acquire(&lock_path)?;
                lock.release()
            }));
        }

        for handle in handles {
            handle
                .join()
                .expect("thread join")
                .expect("lock acquire/release");
        }
        assert!(parent.is_dir());
    }

    #[test]
    fn parent_creation_fails_when_parent_is_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_parent = dir.path().join("not-a-dir");
        std::fs::write(&file_parent, b"x").expect("write parent file");

        let err = super::create_dir_all_retry(&file_parent).expect_err("file parent must fail");
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }
}
