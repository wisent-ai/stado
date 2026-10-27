//! Holding a process-wide lock after a holder panicked.
//!
//! A panic while a `std::sync::Mutex` is held poisons it, and every later
//! `lock()` answers an error; refusing on that error turns one panic into a
//! refusal that lasts until the process ends. For a value every holder changes
//! by whole steps (one insert, remove, replace or read), what a panicking
//! holder left is still a state: the lock is taken back and its poison
//! cleared. A value a holder changes in several steps keeps its refusal.

use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// The guard of `lock`, taken back from a poisoned lock instead of refused.
pub(crate) fn held<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    match lock.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            lock.clear_poison();
            poisoned.into_inner()
        }
    }
}

/// The write guard of `lock`, taken back from a poisoned lock instead of
/// panicking or refusing.
pub(crate) fn written<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    match lock.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            lock.clear_poison();
            poisoned.into_inner()
        }
    }
}

/// The read guard of `lock`, taken back from a poisoned lock instead of
/// panicking or refusing.
pub(crate) fn read_held<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    match lock.read() {
        Ok(guard) => guard,
        Err(poisoned) => {
            lock.clear_poison();
            poisoned.into_inner()
        }
    }
}
