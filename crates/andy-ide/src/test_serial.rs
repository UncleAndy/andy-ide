//! One language server at a time.
//!
//! The tests that ask a real `rust-analyzer` about this very project each start a
//! server of their own. Run in parallel they measure each other: every server indexes
//! the same big workspace, sixteen cores are not enough for ten of them, and a test
//! whose whole point is "the answer comes back" starts failing for a reason that has
//! nothing to do with the answer. One at a time, the answer is the only thing being
//! measured.

#![cfg(test)]

use std::sync::{Mutex, MutexGuard};

/// Guards the language-server tests against one another.
static ONE_SERVER: Mutex<()> = Mutex::new(());

/// Holds the slot until the returned guard is dropped.
///
/// A test that panics while holding it poisons the mutex; the poison is ignored on
/// purpose — one failed test is not a reason to fail the next one.
pub fn one_server() -> MutexGuard<'static, ()> {
    ONE_SERVER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
