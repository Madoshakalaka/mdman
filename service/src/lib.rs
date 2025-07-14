pub mod config;
pub mod sync;
pub mod watcher;

pub use config::Config;
pub use sync::{check_diff, sync_all_files, DiffReport, SyncStats};
pub use watcher::FileWatcher;