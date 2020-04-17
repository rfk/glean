use std::ffi::CString;
use std::os::raw::c_char;

use ffi_support::IntoFfi;

use crate::glean_str_free;
use glean_core::upload::PingUploadTask;

/// Result values of attempted ping uploads encoded for FFI use.
///
/// These are defined in `glean-core/src/upload/result.rs`,
/// but for cbindgen to also export them in header files we need to define them here as constants.
///
/// Tests below ensure they match.
#[allow(dead_code)]
mod upload_result {
    /// A recoverable error.
    pub const UPLOAD_RESULT_RECOVERABLE: u32 = 0x1;

    /// An unrecoverable error.
    pub const UPLOAD_RESULT_UNRECOVERABLE: u32 = 0x2;

    /// A HTTP response code.
    ///
    /// The actual response code is encoded in the lower bits.
    pub const UPLOAD_RESULT_HTTP_STATUS: u32 = 0x8000;
}

/// A FFI-compatible representation for the PingUploadTask
///
/// The order of variants should be the same as in `glean-core/src/upload/mod.rs`
/// and `glean-core/android/src/main/java/mozilla/telemetry/glean/net/Upload.kt`.
/// cbindgen:prefix-with-name
#[repr(u8)]
pub enum FfiPingUploadTask {
    Upload {
        document_id: *mut c_char,
        path: *mut c_char,
        body: *mut c_char,
        headers: *mut c_char,
    },
    Wait,
    Done,
}

impl From<PingUploadTask> for FfiPingUploadTask {
    fn from(task: PingUploadTask) -> Self {
        match task {
            PingUploadTask::Upload(request) => {
                // Safe unwraps:
                // 1. CString::new(..) should not fail as we are the ones that created the strings being transformed;
                // 2. serde_json::to_string(&request.body) should not fail as request.body is a JsonValue;
                // 3. serde_json::to_string(&request.headers) should not fail as request.headers is a HashMap of Strings.
                let document_id = CString::new(request.document_id.to_owned()).unwrap();
                let path = CString::new(request.path.to_owned()).unwrap();
                let body = CString::new(serde_json::to_string(&request.body).unwrap()).unwrap();
                let headers =
                    CString::new(serde_json::to_string(&request.headers).unwrap()).unwrap();
                FfiPingUploadTask::Upload {
                    document_id: document_id.into_raw(),
                    path: path.into_raw(),
                    body: body.into_raw(),
                    headers: headers.into_raw(),
                }
            }
            PingUploadTask::Wait => FfiPingUploadTask::Wait,
            PingUploadTask::Done => FfiPingUploadTask::Done,
        }
    }
}

impl Drop for FfiPingUploadTask {
    fn drop(&mut self) {
        if let FfiPingUploadTask::Upload {
            document_id,
            path,
            body,
            headers,
        } = self
        {
            // We need to free the previously allocated strings before dropping.
            unsafe {
                glean_str_free(*document_id);
                glean_str_free(*path);
                glean_str_free(*body);
                glean_str_free(*headers);
            }
        }
    }
}

unsafe impl IntoFfi for FfiPingUploadTask {
    type Value = FfiPingUploadTask;

    #[inline]
    fn ffi_default() -> FfiPingUploadTask {
        FfiPingUploadTask::Done
    }

    #[inline]
    fn into_ffi_value(self) -> FfiPingUploadTask {
        self
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn constants_match_with_glean_core() {
        assert_eq!(
            upload_result::UPLOAD_RESULT_RECOVERABLE,
            glean_core::upload::ffi_upload_result::UPLOAD_RESULT_RECOVERABLE
        );
        assert_eq!(
            upload_result::UPLOAD_RESULT_UNRECOVERABLE,
            glean_core::upload::ffi_upload_result::UPLOAD_RESULT_UNRECOVERABLE
        );
        assert_eq!(
            upload_result::UPLOAD_RESULT_HTTP_STATUS,
            glean_core::upload::ffi_upload_result::UPLOAD_RESULT_HTTP_STATUS
        );
    }
}
