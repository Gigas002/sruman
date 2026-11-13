//! Serializes tests that spawn processes against tests that check a lock
//! was released.
//!
//! A spawned child shares every open fd of the test binary between its
//! fork and exec, and flocks follow the open file, so a lock another test
//! just dropped can still look held for a moment. Tests asserting a release
//! take [`checks`]; tests that spawn take [`spawns`].

use std::sync::{PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

static SPAWN: RwLock<()> = RwLock::new(());

pub(crate) fn checks() -> RwLockReadGuard<'static, ()> {
    SPAWN.read().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn spawns() -> RwLockWriteGuard<'static, ()> {
    SPAWN.write().unwrap_or_else(PoisonError::into_inner)
}
