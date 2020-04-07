//! A crate to help make working with FFI easier.

extern crate failure;
#[macro_use]
extern crate failure_derive;
extern crate libc;

#[macro_use]
mod nullable;
#[macro_use]
pub mod task;

pub mod error_handling;
pub mod panic;
mod split;

pub use crate::{
    error_handling::{error_message, take_last_error, update_last_error},
    nullable::{NullPointer, Nullable},
    panic::catch_panic,
    split::{split_closure, Split},
    task::Task,
};
