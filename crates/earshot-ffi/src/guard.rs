//! The panic barrier, error codes and argument checks shared by every export,
//! adapted from yakvc's.

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};

use crate::{EARSHOT_ERR_INVALID_ARGUMENT, EARSHOT_ERR_PANIC, EARSHOT_OK};

/// A failed call: the code returned to Java and the message kept for
/// `earshot_last_error`.
#[derive(Debug)]
pub(crate) struct FfiError {
    pub code: i32,
    pub message: String,
}

impl FfiError {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        FfiError {
            code,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        FfiError::new(EARSHOT_ERR_INVALID_ARGUMENT, message)
    }
}

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

/// This thread's last error message, or an empty string.
pub(crate) fn last_error() -> String {
    LAST_ERROR.with(|last| last.borrow().clone())
}

fn fail(error: FfiError) -> i32 {
    LAST_ERROR.with(|last| *last.borrow_mut() = error.message);
    error.code
}

/// Runs `f` and returns its result as a code. A panic must never unwind
/// through `extern "C"` into Java's FFM call — the JVM aborts, taking the
/// game with it — so it is caught and reported as [`EARSHOT_ERR_PANIC`].
pub(crate) fn guard(f: impl FnOnce() -> Result<(), FfiError>) -> i32 {
    // Unwind safety: the world is only ever replaced section by section, so
    // a panic leaves it whole.
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => EARSHOT_OK,
        Ok(Err(error)) => fail(error),
        Err(payload) => fail(panicked(payload)),
    }
}

/// Like [`guard`], for an export that returns `fallback` rather than an
/// error code. The panic must not unwind through `extern "C"` into Java's
/// FFM call (the JVM would abort), so it is caught and `fallback` returned;
/// the message is kept for `earshot_last_error` all the same.
pub(crate) fn guard_value<T>(f: impl FnOnce() -> T, fallback: T) -> T {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            fail(panicked(payload));
            fallback
        }
    }
}

/// What a panicking `payload` becomes: [`EARSHOT_ERR_PANIC`] with its
/// message, kept for `earshot_last_error`.
///
/// Takes the box by value: `&Box<dyn Any + Send>` coerced to `&dyn Any` loses
/// the payload's concrete `TypeId`, so `downcast_ref` would always come back
/// empty and every panic would report "unknown panic".
fn panicked(payload: Box<dyn std::any::Any + Send>) -> FfiError {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic");
    FfiError::new(EARSHOT_ERR_PANIC, format!("engine panicked: {message}"))
}

/// `len` elements at `ptr`.
///
/// # Safety
/// `ptr` must be null or readable for `len` elements.
pub(crate) unsafe fn slice_arg<'a, T>(
    ptr: *const T,
    len: usize,
    what: &str,
) -> Result<&'a [T], FfiError> {
    if len == 0 {
        return Ok(&[]);
    }
    if ptr.is_null() {
        return Err(FfiError::invalid(format!("{what} is null")));
    }
    if !ptr.is_aligned() {
        return Err(FfiError::invalid(format!("{what} is misaligned")));
    }
    check_len::<T>(len, what)?;
    // SAFETY: non-null and aligned (checked above), readable for `len`
    // elements (caller's contract).
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// Refuses a `len` whose elements would not fit in one slice: a slice may
/// not exceed `isize::MAX` bytes, and a length Java could hand over that
/// breaks this would otherwise be undefined behaviour, or a panic inside
/// `guard` (reported as [`EARSHOT_ERR_PANIC`]) in debug builds.
fn check_len<T>(len: usize, what: &str) -> Result<(), FfiError> {
    match len.checked_mul(std::mem::size_of::<T>()) {
        Some(bytes) if bytes <= isize::MAX as usize => Ok(()),
        _ => Err(FfiError::invalid(format!("{what} is too large"))),
    }
}

/// `N` elements at `ptr`, as an array.
///
/// # Safety
/// `ptr` must be null or readable for `N` elements.
pub(crate) unsafe fn array_arg<T: Copy, const N: usize>(
    ptr: *const T,
    what: &str,
) -> Result<[T; N], FfiError> {
    // SAFETY: forwarded from the caller.
    let slice = unsafe { slice_arg(ptr, N, what) }?;
    // Cannot fire: `slice_arg` returns exactly the `N` elements it was asked
    // for (a compile-time constant Java cannot alter), or an error. And every
    // caller sits inside `guard`/`with_engine`, so even here a panic would be
    // caught as [`EARSHOT_ERR_PANIC`] rather than unwinding into Java.
    Ok(slice.try_into().expect("slice_arg returns N elements"))
}

/// `len` writable elements at `ptr`.
///
/// # Safety
/// `ptr` must be null or writable for `len` elements.
pub(crate) unsafe fn slice_mut_arg<'a, T>(
    ptr: *mut T,
    len: usize,
    what: &str,
) -> Result<&'a mut [T], FfiError> {
    if len == 0 {
        return Ok(&mut []);
    }
    if ptr.is_null() {
        return Err(FfiError::invalid(format!("{what} is null")));
    }
    if !ptr.is_aligned() {
        return Err(FfiError::invalid(format!("{what} is misaligned")));
    }
    check_len::<T>(len, what)?;
    // SAFETY: non-null and aligned (checked above), writable for `len`
    // elements (caller's contract).
    Ok(unsafe { std::slice::from_raw_parts_mut(ptr, len) })
}

/// The value at `ptr`, to write.
///
/// # Safety
/// `ptr` must be null or writable.
pub(crate) unsafe fn out_arg<'a, T>(ptr: *mut T, what: &str) -> Result<&'a mut T, FfiError> {
    if !ptr.is_aligned() {
        return Err(FfiError::invalid(format!("{what} is misaligned")));
    }
    // SAFETY: aligned (checked above); null is handled by `as_mut`; writable
    // otherwise (caller's contract).
    unsafe { ptr.as_mut() }.ok_or_else(|| FfiError::invalid(format!("{what} is null")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panics_become_codes_with_a_message() {
        assert_eq!(guard(|| panic!("boom")), EARSHOT_ERR_PANIC);
        assert_eq!(last_error(), "engine panicked: boom");
    }

    #[test]
    fn absurd_lengths_are_refused() {
        let words = [0u64; 4];
        // SAFETY: refused before anything is read: a byte length past
        // isize::MAX is one no slice can describe.
        let err = unsafe {
            slice_arg(
                words.as_ptr().cast::<u8>(),
                (isize::MAX as usize) + 1,
                "xyz",
            )
        }
        .unwrap_err();
        assert_eq!(err.message, "xyz is too large");
        // SAFETY: as above; the byte size overflows usize entirely.
        let err = unsafe { slice_arg(words.as_ptr(), usize::MAX, "xyz") }.unwrap_err();
        assert_eq!(err.message, "xyz is too large");
        let mut words = words;
        // SAFETY: as above, for a writable slice; nothing is written.
        let err = unsafe { slice_mut_arg(words.as_mut_ptr(), usize::MAX, "xyz") }.unwrap_err();
        assert_eq!(err.message, "xyz is too large");
    }

    #[test]
    fn null_and_misaligned_arguments_are_refused() {
        // SAFETY: null is refused before anything is read.
        let err = unsafe { slice_arg::<f64>(std::ptr::null(), 3, "xyz") }.unwrap_err();
        assert_eq!(err.message, "xyz is null");
        let words = [0u64; 2];
        let misaligned = words.as_ptr().cast::<u8>().wrapping_add(1).cast::<f64>();
        // SAFETY: misalignment is refused before anything is read.
        let err = unsafe { slice_arg(misaligned, 1, "xyz") }.unwrap_err();
        assert_eq!(err.message, "xyz is misaligned");
    }
}
