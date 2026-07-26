//! Embedded prelude source — compiled into the binary so the language
//! runtime can prepend it to every user module without file I/O.

/// The Nulang prelude, providing `Option[T]` and `Result[Ok, Err]` in
/// every module without explicit imports.
pub const PRELUDE_SOURCE: &str = include_str!("prelude.nula");
