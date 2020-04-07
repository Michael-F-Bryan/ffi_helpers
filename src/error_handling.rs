//! Common error handling routines.
//!
//! The main error handling method employed is a thread-local variable called
//! `LAST_ERROR` which holds the most recent error as well as some convenience
//! functions for getting/clearing this variable.
//!
//! The theory is if a function fails then it should return an *"obviously
//! invalid"* value (typically `-1` or `0` when returning integers or `NULL` for
//! pointers, see the [`Nullable`] trait for more). The user can then check for
//! this and consult the most recent error for more information. Of course that
//! means all fallible operations *must* update the most recent error if they
//! fail and that you *must* check the returned value of any fallible operation.
//!
//! While it isn't as elegant as Rust's monad-style `Result<T, E>` with `?` and
//! the various combinators, it actually turns out to be a pretty robust error
//! handling technique in practice.
//!
//! > **Note:** It is highly recommended to have a skim through libgit2's
//! > [error handling docs][libgit2]. The error handling mechanism used here
//! > takes a lot of inspiration from `libgit2`.
//!
//! ## Examples
//!
//! The following shows a full example where our `write_data()` function will
//! try to write some data into a buffer. The first time through
//!
//! ```rust
//! #[macro_use]
//! extern crate ffi_helpers;
//! extern crate failure;
//! extern crate libc;
//!
//! use libc::{c_char, c_int};
//! use std::slice;
//! use ffi_helpers::error_handling;
//! # use failure::Error;
//!
//! fn main() {
//!     if unsafe { some_fallible_operation() } != 1 {
//!         // Before we can retrieve the message we need to know how long it is.
//!         let err_msg_length = error_handling::last_error_length();
//!
//!         // then allocate a big enough buffer
//!         let mut buffer = vec![0; err_msg_length as usize];
//!         let bytes_written = unsafe {
//!             let buf = buffer.as_mut_ptr() as *mut c_char;
//!             let len = buffer.len() as c_int;
//!             error_handling::error_message_utf8(buf, len)
//!         };
//!
//!         // then interpret the message
//!         match bytes_written {
//!             -1 => panic!("Our buffer wasn't big enough!"),
//!             0 => panic!("There wasn't an error message... Huh?"),
//!             len if len > 0 => {
//!                 buffer.truncate(len as usize - 1);
//!                 let msg = String::from_utf8(buffer).unwrap();
//!                 println!("Error: {}", msg);
//!             }
//!             _ => unreachable!(),
//!         }
//!     }
//! }
//!
//! /// pretend to do some complicated operation, returning whether the
//! /// operation was successful.
//! #[no_mangle]
//! unsafe extern "C" fn some_fallible_operation() -> c_int {
//!     match do_stuff() {
//!         Ok(_) => 1, // do_stuff() always errors, so this is unreachable
//!         Err(e) => {
//!             ffi_helpers::update_last_error(e);
//!             0
//!         }
//!     }
//! }
//!
//! # fn do_stuff() -> Result<(), Error> { Err(failure::err_msg("An error occurred")) }
//! ```
//!
//! [`Nullable`]: trait.Nullable.html
//! [libgit2]: https://github.com/libgit2/libgit2/blob/master/docs/error-handling.md

use failure::Error;
use libc::{c_char, c_int};
use std::{cell::RefCell, slice};

use crate::nullable::Nullable;

thread_local! {
    static LAST_ERROR: RefCell<Option<Error>> = RefCell::new(None);
}

/// Clear the `LAST_ERROR`.
pub extern "C" fn clear_last_error() { let _ = take_last_error(); }

/// Take the most recent error, clearing `LAST_ERROR` in the process.
pub fn take_last_error() -> Option<Error> {
    LAST_ERROR.with(|prev| prev.borrow_mut().take())
}

/// Update the `thread_local` error, taking ownership of the `Error`.
pub fn update_last_error<E: Into<Error>>(err: E) {
    LAST_ERROR.with(|prev| *prev.borrow_mut() = Some(err.into()));
}

/// Get the length of the last error message in bytes when encoded as UTF-8,
/// including the trailing null.
pub fn last_error_length() -> c_int {
    LAST_ERROR.with(|prev| {
        prev.borrow()
            .as_ref()
            .map(|e| e.to_string().len() + 1)
            .unwrap_or(0)
    }) as c_int
}

/// Get the length of the last error message in bytes when encoded as UTF-16,
/// including the trailing null.
pub fn last_error_length_utf16() -> c_int {
    LAST_ERROR.with(|prev| {
        prev.borrow()
            .as_ref()
            .map(|e| e.to_string().encode_utf16().count() + 1)
            .unwrap_or(0)
    }) as c_int
}

/// Peek at the most recent error and get its error message as a Rust `String`.
pub fn error_message() -> Option<String> {
    LAST_ERROR.with(|prev| prev.borrow().as_ref().map(|e| e.to_string()))
}

/// Peek at the most recent error and write its error message (`Display` impl)
/// into the provided buffer as a UTF-8 encoded string.
///
/// This returns the number of bytes written, or `-1` if there was an error.
pub unsafe fn error_message_utf8(buf: *mut c_char, length: c_int) -> c_int {
    crate::null_pointer_check!(buf);
    let buffer = slice::from_raw_parts_mut(buf as *mut u8, length as usize);

    copy_error_into_buffer(buffer, |msg| msg.into())
}

/// Peek at the most recent error and write its error message (`Display` impl)
/// into the provided buffer as a UTF-16 encoded string.
///
/// This returns the number of bytes written, or `-1` if there was an error.
pub unsafe fn error_message_utf16(buf: *mut u16, length: c_int) -> c_int {
    crate::null_pointer_check!(buf);
    let buffer = slice::from_raw_parts_mut(buf, length as usize);

    let ret =
        copy_error_into_buffer(buffer, |msg| msg.encode_utf16().collect());

    if ret > 0 {
        // utf16 uses two bytes per character
        ret * 2
    } else {
        ret
    }
}

fn copy_error_into_buffer<B, F>(buffer: &mut [B], error_msg: F) -> c_int
where
    F: FnOnce(String) -> Vec<B>,
    B: Copy + Nullable,
{
    let maybe_error_message: Option<Vec<B>> =
        error_message().map(|msg| error_msg(msg));

    let err_msg = match maybe_error_message {
        Some(msg) => msg,
        None => return 0,
    };

    if err_msg.len() + 1 > buffer.len() {
        // buffer isn't big enough
        return -1;
    }

    buffer[..err_msg.len()].copy_from_slice(&err_msg);
    // Make sure to add a trailing null in case people use this as a bare char*
    buffer[err_msg.len()] = B::NULL;

    (err_msg.len() + 1) as c_int
}

#[doc(hidden)]
#[macro_export]
macro_rules! export_c_symbol {
    (fn $name:ident($( $arg:ident : $type:ty ),*) -> $ret:ty) => {
        #[no_mangle]
        pub unsafe extern "C" fn $name($( $arg : $type),*) -> $ret {
            $crate::error_handling::$name($( $arg ),*)
        }
    };
    (fn $name:ident($( $arg:ident : $type:ty ),*)) => {
        export_c_symbol!(fn $name($( $arg : $type),*) -> ());
    }
}

/// As a workaround for rust-lang/rust#6342, you can use this macro to make sure
/// the symbols for `ffi_helpers`'s error handling are correctly exported in
/// your `cdylib`.
#[macro_export]
macro_rules! export_error_handling_functions {
    () => {
        #[allow(missing_docs)]
        #[doc(hidden)]
        pub mod __ffi_helpers_errors {
            export_c_symbol!(fn clear_last_error());
            export_c_symbol!(fn last_error_length() -> ::libc::c_int);
            export_c_symbol!(fn last_error_length_utf16() -> ::libc::c_int);
            export_c_symbol!(fn error_message_utf8(buf: *mut ::libc::c_char, length: ::libc::c_int) -> ::libc::c_int);
            export_c_symbol!(fn error_message_utf16(buf: *mut u16, length: ::libc::c_int) -> ::libc::c_int);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use failure;
    use std::str;

    fn clear_last_error() {
        let _ = LAST_ERROR.with(|e| e.borrow_mut().take());
    }

    #[test]
    fn update_the_error() {
        clear_last_error();

        let err_msg = "An Error Occurred";
        let e = failure::err_msg(err_msg);

        update_last_error(e);

        let got_err_msg =
            LAST_ERROR.with(|e| e.borrow_mut().take().unwrap().to_string());
        assert_eq!(got_err_msg, err_msg);
    }

    #[test]
    fn take_the_last_error() {
        clear_last_error();

        let err_msg = "An Error Occurred";
        let e = failure::err_msg(err_msg);
        update_last_error(e);

        let got_err_msg = take_last_error().unwrap().to_string();
        assert_eq!(got_err_msg, err_msg);
    }

    #[test]
    fn get_the_last_error_messages_length() {
        clear_last_error();

        let err_msg = "An Error Occurred";
        let should_be = err_msg.len() + 1;

        let e = failure::err_msg(err_msg);
        update_last_error(e);

        // Get a valid error message's length
        let got = last_error_length();
        assert_eq!(got, should_be as _);

        // Then clear the error message and make sure we get 0
        clear_last_error();
        let got = last_error_length();
        assert_eq!(got, 0);
    }

    #[test]
    fn write_the_last_error_message_into_a_buffer() {
        clear_last_error();

        let err_msg = "An Error Occurred";

        let e = failure::err_msg(err_msg);
        update_last_error(e);

        let mut buffer: Vec<u8> = vec![0; 40];
        let bytes_written = unsafe {
            error_message_utf8(
                buffer.as_mut_ptr() as *mut c_char,
                buffer.len() as _,
            )
        };

        assert!(bytes_written > 0);
        assert_eq!(bytes_written as usize, err_msg.len() + 1);

        let msg =
            str::from_utf8(&buffer[..bytes_written as usize - 1]).unwrap();
        assert_eq!(msg, err_msg);
    }
}
