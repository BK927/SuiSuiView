use super::super::{
    AppSettings, BookRecordAdoption, BookRecordInput, FitMode, PageBookmarkPathRebase,
    PersistedState, ReadingDirection, StateStore,
};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

include!("tests/support.rs");
include!("tests/common.rs");
include!("tests/persistence.rs");
include!("tests/adoption.rs");
include!("tests/archive.rs");
