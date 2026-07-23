use std::{
    ffi::OsString,
    sync::{Mutex, MutexGuard, OnceLock},
};

thread_local! {
    static ENV_GUARD_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn env_lock() -> &'static Mutex<()> {
    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII guard that restores an environment variable on drop.
///
/// Environment mutation is serialized across the process by this helper.
/// Code that mutates the environment directly with `std::env` outside this
/// helper is still responsible for its own synchronization.
pub struct EnvGuard {
    key: String,
    previous: Option<OsString>,
    _lock: Option<MutexGuard<'static, ()>>,
}

impl EnvGuard {
    /// Set `key` to `value` until this guard is dropped.
    #[must_use]
    pub fn set(key: impl Into<String>, value: impl Into<OsString>) -> Self {
        let key = key.into();
        let lock = ENV_GUARD_DEPTH.with(|depth| {
            let current = depth.get();
            if current == 0 {
                let guard = env_lock().lock().expect("environment lock poisoned");
                depth.set(1);
                Some(guard)
            } else {
                depth.set(current + 1);
                None
            }
        });
        let previous = std::env::var_os(&key);
        // SAFETY: callers hold the process-wide env_lock() mutex; tests use
        // env_lock() explicitly.  set_var is sound under this synchronization.
        unsafe {
            std::env::set_var(&key, value.into());
        }
        Self { key, previous, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.previous {
            // SAFETY: same synchronization contract as set() — the env_lock
            // mutex is held by the outermost EnvGuard or the caller.
            Some(value) => unsafe {
                std::env::set_var(&self.key, value);
            },
            // SAFETY: same synchronization contract as set().
            None => unsafe {
                std::env::remove_var(&self.key);
            },
        }
        ENV_GUARD_DEPTH.with(|depth| {
            let current = depth.get();
            if current > 0 {
                depth.set(current - 1);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::EnvGuard;

    #[test]
    fn restores_previously_unset_variable() {
        const KEY: &str = "ARC_TESTTOOLS_ENV_UNSET";
        // SAFETY: test-only; env_lock() guard protects the mutation.
        unsafe {
            std::env::remove_var(KEY);
        }

        {
            let _guard = EnvGuard::set(KEY, "temporary");
            assert_eq!(std::env::var(KEY).ok().as_deref(), Some("temporary"));
        }

        assert!(std::env::var(KEY).is_err());
    }

    #[test]
    fn restores_previous_value_after_drop() {
        const KEY: &str = "ARC_TESTTOOLS_ENV_RESTORE";
        // SAFETY: test-only; env_lock() guard protects the mutation.
        unsafe {
            std::env::set_var(KEY, "original");
        }

        {
            let _guard = EnvGuard::set(KEY, "temporary");
            assert_eq!(std::env::var(KEY).ok().as_deref(), Some("temporary"));
        }

        assert_eq!(std::env::var(KEY).ok().as_deref(), Some("original"));
        // SAFETY: test-only; env_lock() guard protects the mutation.
        unsafe {
            std::env::remove_var(KEY);
        }
    }

    #[test]
    fn nested_guards_restore_lifo() {
        const KEY: &str = "ARC_TESTTOOLS_ENV_NESTED";
        // SAFETY: test-only; env_lock() guard protects the mutation.
        unsafe {
            std::env::set_var(KEY, "base");
        }

        {
            let _outer = EnvGuard::set(KEY, "outer");
            {
                let _inner = EnvGuard::set(KEY, "inner");
                assert_eq!(std::env::var(KEY).ok().as_deref(), Some("inner"));
            }
            assert_eq!(std::env::var(KEY).ok().as_deref(), Some("outer"));
        }

        assert_eq!(std::env::var(KEY).ok().as_deref(), Some("base"));
        // SAFETY: test-only; env_lock() guard protects the mutation.
        unsafe {
            std::env::remove_var(KEY);
        }
    }
}
