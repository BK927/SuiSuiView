//! Opt-in responsiveness diagnostics. The API accepts only fixed stage names;
//! paths, book identifiers, image data and error strings cannot enter this log.

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Stage {
    StateLoad = 1,
    StartupPath,
    WindowEvent,
    RedrawGlow,
    RedrawWgpu,
    UpdateFrame,
    OpenPath,
    ClassifyPath,
    OpenSource,
    PrepareBook,
    InstallBook,
    SiblingNavigation,
    SiblingEntries,
    ComparePaths,
    BookmarkList,
    BookmarkJump,
    BookmarkToggle,
    ReadBookRecord,
    ScanBookRecords,
    CollectBookRecords,
    RedirectMetadata,
    WriteState,
    Thumbnail,
    ReadPage,
    PreparePage,
}

#[cfg(feature = "stall-diagnostics")]
mod enabled;
#[cfg(feature = "stall-diagnostics")]
pub use enabled::{scope, start_session};

#[cfg(not(feature = "stall-diagnostics"))]
#[inline]
pub fn scope(_stage: Stage) {}

#[cfg(not(feature = "stall-diagnostics"))]
#[inline]
pub fn start_session() {}
