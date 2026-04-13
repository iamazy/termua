use gpui::actions;

mod preview;
mod state;
mod view;

pub use state::{Entry, EntryKind, SortColumn, SortDirection, SortSpec, TreeState, VisibleRow};
pub use view::SftpView;

actions!(
    sftp,
    [
        /// Refresh the currently selected directory (or root if none).
        Refresh,
        /// Upload a local file into the selected directory (or root).
        Upload,
        /// Download the selected remote file to a local path.
        Download,
        /// Create a directory under the selected directory (or root).
        NewFolder,
        /// Rename the selected entry.
        Rename,
        /// Delete the selected entry.
        Delete,
    ]
);
