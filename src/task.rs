//! Management of asynchronous tasks in an FFI context.
//!
//! The *Task API* is very similar to the that exposed by Rust [futures], with
//! the aim to be usable from other languages.
//!
//! The main idea is your C users will:
//!
//! 1. Create a `Task` struct (who's `run()` method will execute the job)
//! 2. Spawn the `Task` on a background thread, receiving an opaque `TaskHandle`
//!    through which the task can be monitored
//! 3. Periodically `poll()` the `TaskHandle` to see whether it's done or if
//!    there was an error
//! 4. Retrieve the result when the `Task` completes
//! 5. destroy the original `TaskHandle` when it's longer need it
//!
//! # Implementing The Task API
//!
//! To use the *Task API* you just need to create a struct which implements the
//! `Task` trait. This is essentially just a trait with a `run()` function
//! that'll be given a [`CancellationToken`].
//!
//!
//! Implementors of the `Task` trait should periodically check the provided
//! [`CancellationToken`] to see whether the caller wants them to stop early.
//!
//! # Examples
//!
//! Once you have a type implementing `Task` you can use the [`export_task!()`]
//! macro to generate `extern "C"` functions for spawning the task and
//! monitoring its progress. This is usually the most
//! annoying/error-prone/tedious part of exposing running a `Task` in the
//! background using just a C API.
//!
//! For this example we're defining a `Spin` task which will count up until it
//! receives a cancel signal, then return the number of spins.
//!
//! ```rust
//! # #[macro_use]
//! # extern crate ffi_helpers;
//! # extern crate failure;
//! # use failure::Error;
//! # use ffi_helpers::task::CancellationToken;
//! # use ffi_helpers::Task;
//! # use ffi_helpers::error_handling::*;
//! # use std::thread;
//! # use std::time::Duration;
//! #[derive(Debug, Clone, Copy)]
//! pub struct Spin;
//!
//! impl Task for Spin {
//!     type Output = usize;
//!
//!     fn run(&self, cancel_tok: &CancellationToken) -> Result<Self::Output, Error> {
//!         let mut spins = 0;
//!
//!         while !cancel_tok.cancelled() {
//!             thread::sleep(Duration::from_millis(10));
//!             spins += 1;
//!         }
//!
//!         Ok(spins)
//!     }
//! }
//!
//! // Generate the various `extern "C"` utility functions for working with the
//! // `Spin` task. The `spawn` function will be called `spin_spawn`, and so on.
//! export_task! {
//!     Task: Spin;
//!     spawn: spin_spawn;
//!     wait: spin_wait;
//!     poll: spin_poll;
//!     cancel: spin_cancel;
//!     cancelled: spin_cancelled;
//!     handle_destroy: spin_handle_destroy;
//!     result_destroy: spin_result_destroy;
//! }
//!
//! fn main() {
//!     // create our `Spin` task
//!     let s = Spin;
//!
//!     unsafe {
//!         // spawn the task in the background and get a handle to it
//!         let handle = spin_spawn(&s);
//!         assert_eq!(spin_cancelled(handle), 0,
//!             "The spin shouldn't have been cancelled yet");
//!
//!         // poll the task. The result can vary depending on the outcome:
//!         // - If the task completed, get a pointer to the `Output`
//!         // - If it completed with an error, return `null` and update the
//!         //   LAST_ERROR appropriately
//!         // - Return `null` and *don't* set LAST_ERROR if the task isn't done
//!         clear_last_error();
//!         let ret = spin_poll(handle);
//!         assert_eq!(last_error_length(), 0, "There shouldn't have been any errors");
//!         assert!(ret.is_null(), "The task should still be running");
//!
//!         // tell the task to stop spinning by sending the cancel signal
//!         spin_cancel(handle);
//!
//!         // wait for the task to finish and retrieve a pointer to its result
//!         // Note: this will automatically free the handle, so we don't need
//!         //       to manually call `spin_handle_destroy()`.
//!         let got = spin_wait(handle);
//!
//!         assert_eq!(last_error_length(), 0, "There shouldn't have been any errors");
//!         assert!(!got.is_null(), "Oops!");
//!
//!         let num_spins: usize = *got;
//!
//!         // don't forget the result is heap allocated so we need to free it
//!         spin_result_destroy(got);
//!     }
//! }
//! ```
//!
//! # Managing Task Output Lifetimes
//!
//! The result of a `Task` will be allocated on the heap and then a pointer
//! returned to the user from the `poll` and `wait` functions. It is the
//! caller's responsibility to ensure this gets free'd once you're done with it.
//!
//! The `export_task!()` macro lets you define a `results_destroy` function
//! which will free the object for you.
//!
//! Zero-sized types (like `()` - Rust's equivalent of the C `void` or Python
//! `None`) won't incur an allocation, meaning the `results_destroy` function
//! will be a noop.
//!
//! [futures]: https://github.com/rust-lang-nursery/futures-rs
//! [`CancellationToken`]: struct.CancellationToken.html
//! [`export_task!()`]: ../macro.export_task.html

use failure::{self, Error};
use std::{
    panic::UnwindSafe,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
        Arc,
    },
    thread,
};

use error_handling;
use panic;

/// Convenience macro to define the FFI bindings for working with a [`Task`].
///
/// This is implemented as an incremental TT muncher which lets you define the
/// functions you'll need. These are:
///
/// - `spawn`: The function for spawning a task on a background thread,
///   returning a [`TaskHandle`]
/// - `poll`: A function for receiving the result if it's available
/// - `wait`: Block the current thread until we get either a result or an error
/// - `cancel`: Cancel the background task
/// - `cancelled`: Has the task already been cancelled?
/// - `result_destroy`: A destructor for the task's result
/// - `handle_destroy`: A destructor for the [`TaskHandle`], for cleaning up the
///   task once you're done with it
///
/// You'll always need to provide the concrete [`Task`] type in the macro's
/// first "argument".
///
/// [`Task`]: task/trait.Task.html
/// [`TaskHandle`]: task/struct.TaskHandle.html
#[macro_export]
macro_rules! export_task {
    ($( #[$attr:meta] )* Task: $Task:ty; spawn: $spawn:ident; $( $tokens:tt )*) => {
        /// Spawn a task in the background, returning a pointer to the task
        /// handle.
        #[allow(dead_code)]
        #[no_mangle]
        $( #[$attr] )*
        pub unsafe extern "C" fn $spawn(task: *const $Task) -> *mut $crate::task::TaskHandle<<$Task as $crate::Task>::Output> {
            null_pointer_check!(task);
            let task = (&*task).clone();
            let handle = $crate::task::TaskHandle::spawn(task);
            Box::into_raw(Box::new(handle))
        }

        export_task!($( #[$attr] )* Task: $Task; $( $tokens )*);
    };
    ($( #[$attr:meta] )* Task: $Task:ty; poll: $poll:ident; $( $tokens:tt )*) => {
        /// Poll the task handle and retrieve the result it's ready.
        ///
        /// # Note
        ///
        /// This will return `null` if there was no result **or** if there was
        /// an error. If there is an error, we update the last error accordingly.
        ///
        /// You probably want to call `ffi_helpers::error_handling::clear_last_error()`
        /// beforehand to make sure there
        #[allow(dead_code)]
        #[no_mangle]
        $( #[$attr] )*
        pub unsafe extern "C" fn $poll(handle: *mut $crate::task::TaskHandle<<$Task as $crate::Task>::Output>) -> *mut <$Task as $crate::Task>::Output {
            null_pointer_check!(handle);
            match (&*handle).poll() {
                Some(Ok(value)) => Box::into_raw(Box::new(value)),
                Some(Err(e)) => {
                    $crate::error_handling::update_last_error(e);
                    ::std::ptr::null_mut()
                }
                None => ::std::ptr::null_mut()
            }
        }

        export_task!($( #[$attr] )* Task: $Task; $( $tokens )*);
    };
    ($( #[$attr:meta] )* Task: $Task:ty; handle_destroy: $handle_destructor:ident; $( $tokens:tt )*) => {
        /// Destroy a task handle once you no longer need it, cancelling the
        /// task if it hasn't yet completed.
        ///
        /// # Warning
        ///
        /// This conflicts with the `wait` function, which also destroys its
        /// task handle.
        #[allow(dead_code)]
        #[no_mangle]
        $( #[$attr] )*
        pub unsafe extern "C" fn $handle_destructor(handle: *mut $crate::task::TaskHandle<<$Task as $crate::Task>::Output>) {
            null_pointer_check!(handle);
            let handle = Box::from_raw(handle);
            drop(handle);
        }

        export_task!($( #[$attr] )* Task: $Task; $( $tokens )*);
    };
    ($( #[$attr:meta] )* Task: $Task:ty; result_destroy: $result_destroy:ident; $( $tokens:tt )*) => {
        /// Destroy the result of a task once you are done with it.
        #[allow(dead_code)]
        #[no_mangle]
        $( #[$attr] )*
        pub unsafe extern "C" fn $result_destroy(result: *mut <$Task as $crate::Task>::Output) {
            null_pointer_check!(result);
            let result = Box::from_raw(result);
            drop(result);
        }

        export_task!($( #[$attr] )* Task: $Task; $( $tokens )*);
    };
    ($( #[$attr:meta] )* Task: $Task:ty; wait: $wait:ident; $( $tokens:tt )*) => {
        /// Wait for the task to finish, returning the boxed result and consuming
        /// the task handle in the process.
        ///
        /// # Warning
        ///
        /// This will consume the task handle, meaning you **should not** call
        /// the handle destructor afterwards.
        #[allow(dead_code)]
        #[no_mangle]
        $( #[$attr] )*
        pub unsafe extern "C" fn $wait(handle: *mut $crate::task::TaskHandle<<$Task as $crate::Task>::Output>)
            -> *mut <$Task as $crate::Task>::Output
        {
            null_pointer_check!(handle);
            let handle = Box::from_raw(handle);
            let result = handle.wait();

            match result {
                Ok(value) => Box::into_raw(Box::new(value)),
                Err(e) => {
                    $crate::update_last_error(e);
                    ::std::ptr::null_mut()
                }
            }
        }

        export_task!($( #[$attr] )* Task: $Task; $( $tokens )*);
    };
    ($( #[$attr:meta] )* Task: $Task:ty; cancel: $cancel:ident; $( $tokens:tt )*) => {
        /// Cancel the task.
        #[allow(dead_code)]
        #[no_mangle]
        $( #[$attr] )*
        pub unsafe extern "C" fn $cancel(handle: *mut $crate::task::TaskHandle<<$Task as $crate::Task>::Output>) {
            null_pointer_check!(handle);
            (&*handle).cancel();
        }

        export_task!($( #[$attr] )* Task: $Task; $( $tokens )*);
    };
    ($( #[$attr:meta] )* Task: $Task:ty; cancelled: $cancelled:ident; $( $tokens:tt )*) => {
        /// Has the task already been cancelled?
        #[allow(dead_code)]
        #[no_mangle]
        $( #[$attr] )*
        pub unsafe extern "C" fn $cancelled(handle: *mut $crate::task::TaskHandle<<$Task as $crate::Task>::Output>) -> ::std::os::raw::c_int {
            null_pointer_check!(handle);
            if (&*handle).cancelled() {
                1
            } else {
                0
            }
        }

        export_task!($( #[$attr] )* Task: $Task; $( $tokens )*);
    };
    ($( #[$attr:meta] )* Task: $Task:ty;) => {};
}

/// A cancellable task which is meant to be run in a background thread.
///
/// For more information on the *Task API*, refer to the [module documentation].
///
/// [module documentation]: ./index.html
pub trait Task: Send + Sync + Clone {
    type Output: Send + Sync;

    /// Run this task to completion *synchronously*, exiting early if the
    /// provided `CancellationToken` is triggered.
    ///
    /// You probably shouldn't call this function directly. Instead prefer
    /// higher level abstractions like [`TaskHandle::spawn()`] or bindings
    /// generated by the [`export_task!()`] macro.
    ///
    /// [`TaskHandle::spawn()`]: struct.TaskHandle.html#method.spawn
    /// [`export_task!()`]: ../macro.export_task.html
    fn run(
        &self,
        cancel_tok: &CancellationToken,
    ) -> Result<Self::Output, Error>;
}

/// A shareable token to let you notify other tasks they should stop what they
/// are doing and exit early.
#[derive(Debug, Clone)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Create a new `CancellationToken`.
    pub fn new() -> CancellationToken {
        CancellationToken(Arc::new(AtomicBool::new(false)))
    }

    /// Has this token already been cancelled?
    pub fn cancelled(&self) -> bool { self.0.load(Ordering::SeqCst) }

    /// Cancel the token, notifying anyone else listening that they should halt
    /// what they are doing.
    pub fn cancel(&self) { self.0.store(true, Ordering::SeqCst); }

    pub fn is_done(&self) -> Result<(), Cancelled> {
        if self.cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Default for CancellationToken {
    fn default() -> CancellationToken { CancellationToken::new() }
}

/// An error to indicate a task was cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Fail)]
#[fail(display = "The task was cancelled")]
pub struct Cancelled;

/// An opaque handle to some task which is running in the background.
pub struct TaskHandle<T> {
    result: Receiver<Result<T, Error>>,
    token: CancellationToken,
}

impl<T> TaskHandle<T> {
    /// Spawn a `Task` in the background, returning the a `TaskHandle` so you
    /// can cancel it or retrieve the result later on.
    pub fn spawn<K>(task: K) -> TaskHandle<T>
    where
        K: Task<Output = T> + UnwindSafe + Send + Sync + 'static,
        T: Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let cancel_tok = CancellationToken::new();
        let tok_2 = cancel_tok.clone();

        thread::spawn(move || {
            error_handling::clear_last_error();

            let got =
                panic::catch_panic(move || task.run(&tok_2)).map_err(|_| {
                    // we want to preserve panic messages and pass them back to
                    // the main thread so we manually take
                    // LAST_ERROR
                    let e = error_handling::take_last_error();
                    e.unwrap_or_else(|| failure::err_msg("The task failed"))
                });

            tx.send(got).ok();
        });

        TaskHandle {
            result: rx,
            token: cancel_tok,
        }
    }

    /// Check if the background task has finished.
    ///
    /// If the other end hangs up for whatever reason this will return an error.
    pub fn poll(&self) -> Option<Result<T, Error>> {
        // This looks an awful lot like the Futures API, doesn't it?

        match self.result.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Empty) => None,
            Err(e) => Some(Err(e.into())),
        }
    }

    /// Block the current thread until the task has finished and returned a
    /// result.
    pub fn wait(self) -> Result<T, Error> {
        match self.result.recv() {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(e),
            Err(recv_err) => Err(recv_err.into()),
        }
    }

    /// Cancel the background task.
    pub fn cancel(&self) { self.token.cancel(); }

    /// Has this task been cancelled?
    pub fn cancelled(&self) -> bool { self.token.cancelled() }
}

impl<T> Drop for TaskHandle<T> {
    fn drop(&mut self) { self.token.cancel(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panic::Panic;
    use std::time::Duration;

    #[derive(Debug, Clone, Copy)]
    pub struct Spin;

    impl Task for Spin {
        type Output = usize;

        fn run(
            &self,
            cancel_tok: &CancellationToken,
        ) -> Result<Self::Output, Error> {
            let mut spins = 0;

            while !cancel_tok.cancelled() {
                thread::sleep(Duration::from_millis(10));
                spins += 1;
            }

            Ok(spins)
        }
    }

    #[test]
    fn spawn_a_task() {
        let task = Spin;

        let handle = TaskHandle::spawn(task);

        // wait for about 100 ms
        for _ in 0..10 {
            thread::sleep(Duration::from_millis(10));
            let got = handle.poll();
            assert!(got.is_none());
        }

        handle.cancel();

        let got = handle.wait().unwrap();
        // the task should have spun 9-12 times (depending on the OS's
        // scheduler)
        assert!(9 <= got && got <= 12);
    }

    export_task! {
        Task: Spin;
        spawn: spin_spawn;
        wait: spin_wait;
        poll: spin_poll;
        cancel: spin_cancel;
        cancelled: spin_cancelled;
        handle_destroy: spin_handle_destroy;
        result_destroy: spin_result_destroy;
    }

    #[test]
    fn use_the_c_api() {
        use error_handling::*;

        let s = Spin;

        unsafe {
            let handle = spin_spawn(&s);
            assert_eq!(
                spin_cancelled(handle),
                0,
                "The spin shouldn't have been cancelled yet"
            );

            // poll the task
            clear_last_error();
            let ret = spin_poll(handle);
            assert!(ret.is_null(), "The task should still be running");
            assert_eq!(
                last_error_length(),
                0,
                "There shouldn't have been any errors"
            );

            // tell the task to stop spinning
            spin_cancel(handle);

            // wait for the task to finish and retrieve its result
            let got = spin_wait(handle);

            assert_eq!(
                last_error_length(),
                0,
                "There shouldn't have been any errors"
            );
            assert!(!got.is_null(), "Oops!");
        }
    }

    #[derive(Copy, Clone)]
    struct PanicTask;
    const PANIC_MESSAGE: &str = "Oops";

    impl Task for PanicTask {
        type Output = ();

        fn run(&self, _: &CancellationToken) -> Result<Self::Output, Error> {
            panic!(PANIC_MESSAGE)
        }
    }

    #[test]
    fn task_can_catch_panic_messages() {
        let task = PanicTask;

        let err = TaskHandle::spawn(task).wait().unwrap_err();

        if let Some(p) = err.downcast_ref::<Panic>() {
            assert_eq!(p.message, PANIC_MESSAGE);
        } else {
            panic!("Expected a panic failure, got {}", err);
        }
    }
}
