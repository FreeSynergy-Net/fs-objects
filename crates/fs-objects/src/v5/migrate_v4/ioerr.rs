//! The error of a file operation of the scaffold. Split from `mod.rs` (only modules and re-exports live there).
//!
//! Pattern: Adapter (an `std::io::Error` as the layer's error).

/// A file operation that failed, as the layer's error.
pub(super) fn io(what: &str, e: &std::io::Error) -> crate::v5::error::Error {
    crate::v5::error::Error::Unavailable(format!("{what}: {e}"))
}
