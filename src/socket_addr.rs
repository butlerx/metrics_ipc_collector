use interprocess::local_socket::{GenericFilePath, GenericNamespaced, Name, prelude::*};
use std::{io, path::PathBuf};

/// Default namespaced socket name shared by the collector and recorder.
pub const DEFAULT_SOCKET_NAME: &str = "metrics_collector.sock";

/// Where a socket collector listens and a socket recorder connects.
#[derive(Debug, Clone)]
pub enum SocketAddr {
    /// A namespaced name. See `IPCSocketCollector::socket` for how it maps to
    /// each platform.
    Namespaced(String),
    /// A filesystem path.
    Path(PathBuf),
}

impl Default for SocketAddr {
    fn default() -> Self {
        Self::Namespaced(DEFAULT_SOCKET_NAME.into())
    }
}

impl std::fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Namespaced(name) => write!(f, "socket name {name:?}"),
            Self::Path(path) => write!(f, "socket path {}", path.display()),
        }
    }
}

impl SocketAddr {
    pub fn to_name(&self) -> io::Result<Name<'_>> {
        match self {
            Self::Namespaced(name) => name.as_str().to_ns_name::<GenericNamespaced>(),
            Self::Path(path) => path.as_path().to_fs_name::<GenericFilePath>(),
        }
    }
}
