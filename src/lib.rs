//! A crate to help make working with FFI easier.

extern crate failure;
#[macro_use]
extern crate failure_derive;
extern crate libc;

#[macro_use]
mod nullable;
pub mod error_handling;
pub mod panic;
mod split;
pub mod task;

pub use error_handling::{error_message, take_last_error, update_last_error};
pub use nullable::{NullPointer, Nullable};
pub use panic::catch_panic;
pub use split::{split_closure, Split};
pub use task::Task;
