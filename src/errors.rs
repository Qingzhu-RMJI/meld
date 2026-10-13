use thiserror::Error;

#[derive(Error, Debug)]
pub enum MeldError {
    #[error("sessions root not found: {0}")]
    RootNotFound(String),

    #[error("another meld instance is running (lock file: {0}). If background sync is on (`meld service status`), syncing already happens automatically; to restore, turn it off first with `meld service uninstall`.")]
    Locked(String),

    #[error("no snapshot available to restore")]
    NoSnapshot,

    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

pub type Result<T> = anyhow::Result<T>;
