//! Process-wide environment access serialization.
//!
//! `std::env::set_var` / `remove_var` mutate a process-global table and are not
//! thread-safe: a write racing with any other environment read or write from a
//! different thread is undefined behavior. Code that mutates the environment
//! (notably the `.env` loading in [`crate::settings`]) or that depends on a stable
//! view of it must serialize through [`env_lock`].

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Returns the shared, process-wide environment mutex.
///
/// All framework code that mutates `std::env` (or needs a consistent snapshot
/// while another thread might mutate it) takes this single lock, so the writes are
/// serialized against each other. A separate per-module lock would not help, since
/// two distinct locks do not exclude one another.
pub(crate) fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Locks [`env_lock`], recovering the guard if a previous holder panicked.
///
/// A poisoned environment lock carries no invariant of its own (the guarded data
/// is `()`), so it is safe to keep using it after a panic elsewhere.
pub(crate) fn env_guard() -> MutexGuard<'static, ()> {
    env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
