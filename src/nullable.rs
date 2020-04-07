/// An object which has an "obviously invalid" value, for use with the
/// [`null_pointer_check!()`][npc] macro.
///
/// This trait is implemented for all integer types and raw pointers, returning
/// `0` and `null` respectively.
///
/// [npc]: macro.null_pointer_check.html
pub trait Nullable {
    const NULL: Self;

    fn is_null(&self) -> bool;
}

macro_rules! impl_nullable {
    ($type:ty) => {
        impl_nullable!($type, <>);
    };
    ($type:ty, <$($letter:ident),*>) => {
        impl<$($letter)*> Nullable for $type {
            const NULL: Self = 0 as $type;

            #[inline]
            fn is_null(&self) -> bool {
                *self == Self::NULL
            }
        }
    };
}

macro_rules! impl_nullables {
    ($($type:ty),*) => {
        $(
            impl_nullable!($type);
        )*
    }
}

impl_nullable!(*const T, <T>);
impl_nullable!(*mut T, <T>);
impl_nullables!(u8, i8, u16, i16, u32, i32, u64, i64, usize, isize);

impl<T> Nullable for Option<T> {
    const NULL: Self = None;

    #[inline]
    fn is_null(&self) -> bool { self.is_none() }
}

impl Nullable for () {
    const NULL: Self = ();

    #[inline]
    fn is_null(&self) -> bool { true }
}

/// Check if we've been given a null pointer, if so we'll return early.
///
/// The returned value is the [`NULL`] value for whatever type the calling
/// function returns. The `LAST_ERROR` thread-local variable is also updated
/// with [`NullPointer`].
///
///
/// # Examples
///
/// The typical use case is to call `null_pointer_check!()` before doing an
/// operation with a raw pointer. For example, say a C function passes you a
/// pointer to some Rust object so it can get a reference to something inside:
///
/// ```rust,no_run
/// # #[macro_use]
/// # extern crate ffi_helpers;
/// struct Foo {
///   data: Vec<u8>,
/// }
///
/// #[no_mangle]
/// unsafe extern "C" fn foo_get_data(foo: *const Foo) -> *const u8 {
///     null_pointer_check!(foo);
///
///     let foo = &*foo;
///     foo.data.as_ptr()
/// }
/// # fn main() {}
/// ```
///
///
/// Because `Nullable` is implemented for `()` you can also use the macro as a
/// cheap way to return early from a function. As an example, destructors are a
/// common place where you don't want to do anything if passed a `NULL` pointer.
///
/// ```rust,no_run
/// # #[macro_use]
/// # extern crate ffi_helpers;
/// struct Foo {
///   data: Vec<u8>,
/// }
///
/// #[no_mangle]
/// unsafe extern "C" fn foo_destroy(foo: *mut Foo) {
///     null_pointer_check!(foo);
///
///     let foo = Box::from_raw(foo);
///     drop(foo);
/// }
/// # fn main() {}
/// ```
///
/// Sometimes when there's an error you'll use something different. For example
/// when writing data into a buffer you'll usually return the number of bytes
/// written. Because `0` (the [`NULL`] value for an integer) is typically a
/// valid number of bytes, you'll return `-1` to indicate there was an error.
///
/// The `null_pointer_check!()` macro accepts a second argument to allow this.
///
/// ```rust,no_run
/// # #[macro_use]
/// # extern crate ffi_helpers;
/// # extern crate libc;
/// use libc::{c_char, c_int};
/// use std::slice;
///
/// #[no_mangle]
/// unsafe extern "C" fn write_message(buf: *mut c_char, length: c_int) -> c_int {
///     null_pointer_check!(buf, -1);
///     let mut buffer = slice::from_raw_parts_mut(buf as *mut u8, length as usize);
///
///     /* write some data into the buffer */
/// # 0
/// }
/// # fn main() {}
/// ```
///
/// [`NULL`]: trait.Nullable.html#associatedconstant.NULL
/// [`NullPointer`]: struct.NullPointer.html
#[macro_export]
macro_rules! null_pointer_check {
    ($ptr:expr) => {
        null_pointer_check!($ptr, Nullable::NULL)
    };
    ($ptr:expr, $null:expr) => {{
        #[allow(unused_imports)]
        use $crate::Nullable;
        if <_ as $crate::Nullable>::is_null(&$ptr) {
            $crate::error_handling::update_last_error($crate::NullPointer);
            return $null;
        }
    }};
}

/// A `null` pointer was encountered where it wasn't expected.
#[derive(Debug, Copy, Clone, PartialEq, Fail)]
#[fail(display = "A null pointer was passed in where it wasn't expected")]
pub struct NullPointer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_pointer_check_on_garbage_address_doesnt_segfault() {
        let random_address = 12345 as *const u8;
        assert!(!random_address.is_null());
    }

    #[test]
    fn can_detect_null_pointers() {
        let null = 0 as *const u8;
        assert!(<_ as Nullable>::is_null(&null));
    }

    #[test]
    fn can_detect_non_null_pointers() {
        let thing = 123;
        let not_null = &thing as *const i32;
        assert!(!<_ as Nullable>::is_null(&not_null));
    }
}
