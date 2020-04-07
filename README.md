# FFI Helpers

[![License](https://img.shields.io/github/license/Michael-F-Bryan/ffi_helpers.svg)](https://raw.githubusercontent.com/Michael-F-Bryan/ffi_helpers/master/LICENSE)
[![Crates.io](https://img.shields.io/crates/v/ffi_helpers.svg)](https://crates.io/crates/ffi_helpers)
[![Documentation](https://docs.rs/ffi_helpers/badge.svg)](https://docs.rs/ffi_helpers)

A crate to make working with FFI code easier.

This is the open-source version of a utility crate we use at work. The original
purpose was to make it easier for Rust modules (DLLs) to integrate with our
main GUI application. We found it to be particularly elegant and robust to use,
so thought it'd be a nice thing to share with the world.

## Features

This tries to give you a set of abstractions upon which **safe** APIs can be
built. It tries to deal with several issues commonly encountered when writing
FFI code.

### Error Handling

Error handling is done via a private thread-local `LAST_ERROR` variable which
lets you indicate a error using a similar mechanism to `errno`.

The idea is if a Rust function returns a `Result::Err(_)`, it'll pass that
error to `LAST_ERROR` and then return an *obviously wrong* value (e.g. `null`
or `0`). The caller then checks for this return and can inspect `LAST_ERROR`
for more information.

A macro is provided to let you inspect `LAST_ERROR` from C.

### Null Pointers

The `null_pointer_check!()` macro will check whether some *nullable* thing is
null, if so it'll bail with an erroneous return value (`null` for functions
returning pointers or `0` for integers) and set the `LAST_ERROR` to indicate
a null pointer was encountered.

We use a `Nullable` trait to represent anything which has some sort of
"*obviously invalid*" value (e.g. `null` pointers, `0`).

```rust
pub trait Nullable {
    const NULL: Self;

    fn is_null(&self) -> bool;
}
```

The `null_pointer_check!()` then lets you check whether a particular thing is
invalid, setting the `LAST_ERROR`, and returning early from the current function
with `Nullable::NULL`.

In practice, this turns out to make handling the possibility of invalid input
quite ergonomic.

```rust
struct Foo {
  data: Vec<u8>,
}

#[no_mangle]
unsafe extern "C" fn foo_get_data(foo: *const Foo) -> *const u8 {
    null_pointer_check!(foo);

    let foo = &*foo;
    foo.data.as_ptr()
}
```

### Exception Safety

Exception safety becomes a concern when a bit of Rust code panics and tries to
unwind across the FFI barrier. At the moment this will abort the program and,
while no longer straight up *Undefined Behaviour*, this is still a massive pain
to work around.

There is a `catch_panic()` function that lets you execute some code and will
catch any unwinding, updating the `LAST_ERROR` appropriately. The
`catch_panic!()` macro makes this a little easier and works with the `Nullable`
trait so you can bail out of a function, returning an error (`Nullable::NULL`).

### Asynchronous Tasks

The *Task API* helps handle the tricky concurrency issues you encounter when
running a job on a background thread and then trying to expose this to C, while
maintaining memory- and thread-safety.

The `Task` trait itself is quite simple:

```rust
pub trait Task: Send + Sync + Clone {
    type Output: Send + Sync;
    fn run(&self, cancel_tok: &CancellationToken) -> Result<Self::Output, Error>;
}
```

You then generate the bindings via the `export_task!()` macro. This will declare
various `extern "C"` functions for spawning the `Task` on a background thread,
periodically checking whether it's done, allowing you to cancel the task, then
retrieve the result and clean everything up properly afterwards.

This is probably the crate's **killer feature** as it lets you to painlessly
run Rust tasks in the background, allowing you to integrate it into a larger
application/GUI.

It is highly recommended to visit the `task` module's docs for a more detailed
explanation.
