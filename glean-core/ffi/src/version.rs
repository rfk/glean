/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

#![allow(unknown_lints)]
#![warn(rust_2018_idioms)]

use std::ffi::CString;
use std::os::raw::c_char;

/// This exists to allow consumers who dynamically load `libglean_ffi.so` to
/// ensure they are using a compatible version, and avoid potential memory-
/// safety issues that might arise if they construct bindings that expect
/// a different ABI.
///
/// It may be possible to achieve this via some other mechanism (such as
/// versioning `DT_SONAME` within the .so itself) but it's not entirely obvious
/// how that should work with our rust pipeline; see e.g. the discussion in
/// https://github.com/rust-lang/rust/issues/22399. Doing an explicit version
/// check is a nice safety layer in the meantime.
///
/// Critically, that means this function must be ABI stable! It needs to take no
/// arguments, and return either null, or a NUL-terminated C string. Failure to
/// do this will result in memory unsafety when an old version of a consumer
/// loads a newer library!
///
/// If we ever need to change that (which seems unlikely, since we could encode
/// whatever we want in a string if it came to it), we must change the function's
/// name as well.
#[no_mangle]
pub extern "C" fn glean_get_version() -> *const c_char {
    VERSION_PTR.0
}

static VERSION: Option<&str> = option_env!("CARGO_PKG_VERSION");

// For now it's tricky for this string to get freed, so just allocate one and save its pointer.
lazy_static::lazy_static! {
    static ref VERSION_PTR: StaticCStringPtr = StaticCStringPtr(
        VERSION.and_then(|s| CString::new(s).ok())
            .map_or(std::ptr::null(), |cs| cs.into_raw()));
}

// Wrapper that lets us keep a raw pointer in a lazy_static
#[repr(transparent)]
#[derive(Copy, Clone)]
struct StaticCStringPtr(*const c_char);
unsafe impl Send for StaticCStringPtr {}
unsafe impl Sync for StaticCStringPtr {}
