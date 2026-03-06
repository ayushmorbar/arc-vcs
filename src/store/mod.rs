pub mod cas;
pub mod change;
pub mod graph;
pub mod repo;
pub mod view;

/// Errors produced by the arc object store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] Box<bincode::ErrorKind>),
}
