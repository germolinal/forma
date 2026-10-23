#![warn(clippy::dbg_macro)]
#![deny(missing_docs)]
//! Backends that allow Forma to store data, or not

/// Stores data in the local file system.
pub mod file;

/// Stores data in MongoDB.
pub mod mongo;

/// Stores data in memory.
pub mod none;
pub use none::NoneDB;

/// A Forma Database client taht connects to
/// different backends
pub mod multi;
pub use multi::MultiDB;

/// The client that handles the storage and persistent of data. Every backend
/// is compiled in, and which one a server uses is chosen when it starts.
pub type FormaDBClient = multi::MultiDB;
