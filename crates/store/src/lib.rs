//! On-disk storage for Sendito collections. See FORMAT.md for the file formats.

pub mod collection;
pub mod config;
pub mod cookies;
pub mod env;
pub mod history;
pub mod init;
pub mod request_file;
pub mod session;
pub mod themes;
pub mod tomlfmt;

use std::path::{Path, PathBuf};

pub use collection::{Collection, Folder, RequestEntry};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {msg}", path.display())]
    Parse { path: PathBuf, msg: String },
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    #[error("{0}")]
    Invalid(String),
    #[error("secret store: {0}")]
    Secret(String),
}

impl StoreError {
    pub fn io(path: &Path, source: std::io::Error) -> Self {
        StoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;
