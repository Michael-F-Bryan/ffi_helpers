use anyhow::Error;
use std::{
    any::Any,
    panic::{self, UnwindSafe},
};
use thiserror::Error;

use crate::error_handling;

const DEFAULT_PANIC_MSG: &str = "The program panicked";

/// A convenience macro for running a fallible operation (which may panic) and
/// returning `Nullable::NULL` if there are any errors.
///
/// This is a simple wrapper around [`catch_panic()`] so if there are any errors
/// the `LAST_ERROR` will be updated accordingly.
///
/// # Examples
///
/// TODO: Insert an example or two here
///
/// [`catch_panic()`]: fn.catch_panic.html
#[macro_export]
macro_rules! catch_panic {
    ($($tokens:tt)*) => {{
        let result = $crate::catch_panic(|| { $($tokens)* });
        match result {
            Ok(value) => value,
            Err(_) => return $crate::Nullable::NULL,
        }
    }};
}

/// Try to execute some function, catching any panics and translating them into
/// errors to make sure Rust doesn't unwind across the FFI boundary.
///
/// If the function returns an error or panics the `Error` is passed into
/// [`update_last_error()`].
///
/// [`update_last_error()`]: fn.update_last_error.html
pub fn catch_panic<T, F>(func: F) -> Result<T, ()>
where
    F: FnOnce() -> Result<T, Error> + UnwindSafe,
{
    let result = panic::catch_unwind(func)
        .map_err(|e| {
            let panic_msg = recover_panic_message(e)
                .unwrap_or_else(|| DEFAULT_PANIC_MSG.to_string());
            Error::from(Panic::new(panic_msg))
        })
        .and_then(|v| v);

    match result {
        Ok(v) => Ok(v),
        Err(e) => {
            error_handling::update_last_error(e);
            Err(())
        },
    }
}

/// A caught panic message.
#[derive(Debug, Clone, PartialEq, Error)]
#[error("Panic: {}", message)]
pub struct Panic {
    /// The panic message.
    pub message: String,
}

impl Panic {
    fn new<S: Into<String>>(msg: S) -> Panic {
        Panic {
            message: msg.into(),
        }
    }
}

/// Try to recover the error message from a panic.
///
/// `std::panic::catch_unwind()` gives you a `Box<Any + Send + 'static>` instead
/// of a concrete error type. This will attempt to downcast the error to various
/// "common" panic error types, falling back to some stock message if we can't
/// figure out what the original panic message was.
pub fn recover_panic_message(
    e: Box<dyn Any + Send + 'static>,
) -> Option<String> {
    if let Some(msg) = e.downcast_ref::<String>() {
        Some(msg.clone())
    } else if let Some(msg) = e.downcast_ref::<&str>() {
        Some(msg.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_handling::*;

    #[test]
    fn able_to_catch_panics_and_recover_the_panic_message() {
        let _ = take_last_error();
        let err_msg = "Miscellaneous panic message";

        let got: Result<(), ()> = catch_panic(|| panic!("{}", err_msg));
        assert!(got.is_err());

        let got_error = take_last_error().unwrap();

        match got_error.downcast_ref::<Panic>() {
            Some(p) => assert_eq!(p.message, err_msg),
            _ => unreachable!(),
        }
    }
}
