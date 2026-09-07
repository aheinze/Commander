use dualpane_core::VPath;
use dualpane_vfs::VfsError;
use thiserror::Error;

/// Failures from directory indexing and worker orchestration.
#[derive(Debug, Error)]
pub enum IndexError {
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Vfs(#[from] VfsError),
    #[error("failed to spawn {worker} worker: {source}")]
    Spawn {
        worker: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("{operation} failed for {path}: {message}")]
    Watch {
        operation: &'static str,
        path: VPath,
        message: String,
    },
    #[error("worker thread panicked")]
    WorkerPanicked,
}
