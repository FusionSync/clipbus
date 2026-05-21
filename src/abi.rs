#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::clipboard::{Callbacks, Clipboard, Options, TargetOffer};
use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::slice;

pub const CLIPBUS_ABI_VERSION: u32 = 4;

#[repr(C)]
pub struct clipbus_clipboard {
    inner: Clipboard,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(non_camel_case_types)]
pub enum clipbus_status {
    CLIPBUS_OK = 0,
    CLIPBUS_PENDING = 1,
    CLIPBUS_UNSUPPORTED = 2,
    CLIPBUS_TIMEOUT = 3,
    CLIPBUS_PLATFORM_ERROR = 4,
    CLIPBUS_INVALID_ARGUMENT = 5,
    CLIPBUS_ALREADY_STARTED = 6,
    CLIPBUS_NOT_STARTED = 7,
    CLIPBUS_NOT_FOUND = 8,
    CLIPBUS_PANIC = 9,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(non_camel_case_types)]
pub enum clipbus_backend {
    CLIPBUS_BACKEND_AUTO = 0,
    CLIPBUS_BACKEND_X11 = 1,
    CLIPBUS_BACKEND_FAKE = 2,
}

#[repr(C)]
pub struct clipbus_options {
    pub abi_version: u32,
    pub backend: clipbus_backend,
    pub display_name: *const c_char,
    pub request_timeout_ms: u64,
    pub max_inline_bytes: u64,
    pub owner_window_name: *const c_char,
}

#[repr(C)]
pub struct clipbus_target_offer {
    pub native_target: *const c_char,
    pub is_promised: u8,
    pub reserved0: u8,
    pub reserved1: u16,
    pub max_bytes: u64,
}

#[allow(non_camel_case_types)]
pub type clipbus_request_id_t = u64;

#[allow(non_camel_case_types)]
pub type clipbus_target_request_cb = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        request_id: clipbus_request_id_t,
        native_target: *const c_char,
        max_inline_bytes: u64,
        timeout_ms: u64,
    ) -> clipbus_status,
>;

#[allow(non_camel_case_types)]
pub type clipbus_targets_changed_cb = Option<unsafe extern "C" fn(user: *mut c_void)>;

#[allow(non_camel_case_types)]
pub type clipbus_owner_lost_cb = Option<unsafe extern "C" fn(user: *mut c_void)>;

#[allow(non_camel_case_types)]
pub type clipbus_error_cb =
    Option<unsafe extern "C" fn(user: *mut c_void, code: i32, message: *const c_char)>;

#[allow(non_camel_case_types)]
pub type clipbus_target_list_cb = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        request_id: clipbus_request_id_t,
        status: clipbus_status,
        native_targets: *const *const c_char,
        target_count: usize,
    ),
>;

#[allow(non_camel_case_types)]
pub type clipbus_target_data_cb = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        request_id: clipbus_request_id_t,
        native_target: *const c_char,
        status: clipbus_status,
        data: *const u8,
        data_len: usize,
    ),
>;

#[allow(non_camel_case_types)]
pub type clipbus_stream_ready_cb = Option<
    unsafe extern "C" fn(
        user: *mut c_void,
        request_id: clipbus_request_id_t,
        native_target: *const c_char,
        max_chunk_bytes: u64,
        timeout_ms: u64,
    ),
>;

#[repr(C)]
pub struct clipbus_callbacks {
    pub abi_version: u32,
    pub target_request: clipbus_target_request_cb,
    pub targets_changed: clipbus_targets_changed_cb,
    pub owner_lost: clipbus_owner_lost_cb,
    pub error: clipbus_error_cb,
    pub target_list: clipbus_target_list_cb,
    pub target_data: clipbus_target_data_cb,
    pub stream_ready: clipbus_stream_ready_cb,
}

impl From<crate::clipboard::Status> for clipbus_status {
    fn from(status: crate::clipboard::Status) -> Self {
        match status {
            crate::clipboard::Status::Ok => Self::CLIPBUS_OK,
            crate::clipboard::Status::Pending => Self::CLIPBUS_PENDING,
            crate::clipboard::Status::Unsupported => Self::CLIPBUS_UNSUPPORTED,
            crate::clipboard::Status::Timeout => Self::CLIPBUS_TIMEOUT,
            crate::clipboard::Status::PlatformError => Self::CLIPBUS_PLATFORM_ERROR,
            crate::clipboard::Status::InvalidArgument => Self::CLIPBUS_INVALID_ARGUMENT,
            crate::clipboard::Status::AlreadyStarted => Self::CLIPBUS_ALREADY_STARTED,
            crate::clipboard::Status::NotStarted => Self::CLIPBUS_NOT_STARTED,
            crate::clipboard::Status::NotFound => Self::CLIPBUS_NOT_FOUND,
            crate::clipboard::Status::Panic => Self::CLIPBUS_PANIC,
        }
    }
}

impl From<clipbus_status> for crate::clipboard::Status {
    fn from(status: clipbus_status) -> Self {
        match status {
            clipbus_status::CLIPBUS_OK => Self::Ok,
            clipbus_status::CLIPBUS_PENDING => Self::Pending,
            clipbus_status::CLIPBUS_UNSUPPORTED => Self::Unsupported,
            clipbus_status::CLIPBUS_TIMEOUT => Self::Timeout,
            clipbus_status::CLIPBUS_PLATFORM_ERROR => Self::PlatformError,
            clipbus_status::CLIPBUS_INVALID_ARGUMENT => Self::InvalidArgument,
            clipbus_status::CLIPBUS_ALREADY_STARTED => Self::AlreadyStarted,
            clipbus_status::CLIPBUS_NOT_STARTED => Self::NotStarted,
            clipbus_status::CLIPBUS_NOT_FOUND => Self::NotFound,
            clipbus_status::CLIPBUS_PANIC => Self::Panic,
        }
    }
}

fn convert_result<F>(f: F) -> clipbus_status
where
    F: FnOnce() -> crate::clipboard::Status,
{
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(status) => status.into(),
        Err(_) => clipbus_status::CLIPBUS_PANIC,
    }
}

fn str_from_ptr(ptr: *const c_char) -> Result<Option<String>, crate::clipboard::Status> {
    if ptr.is_null() {
        return Ok(None);
    }
    let value = unsafe { CStr::from_ptr(ptr) };
    let string = value
        .to_str()
        .map_err(|_| crate::clipboard::Status::InvalidArgument)?
        .to_owned();
    Ok(Some(string))
}

fn parse_options(options: *const clipbus_options) -> Result<Options, crate::clipboard::Status> {
    if options.is_null() {
        return Err(crate::clipboard::Status::InvalidArgument);
    }
    let options = unsafe { &*options };
    if options.abi_version != CLIPBUS_ABI_VERSION {
        return Err(crate::clipboard::Status::InvalidArgument);
    }
    let backend = match options.backend {
        clipbus_backend::CLIPBUS_BACKEND_AUTO => crate::clipboard::BackendKind::X11,
        clipbus_backend::CLIPBUS_BACKEND_X11 => crate::clipboard::BackendKind::X11,
        clipbus_backend::CLIPBUS_BACKEND_FAKE => crate::clipboard::BackendKind::Fake,
    };
    Ok(Options {
        backend,
        display_name: str_from_ptr(options.display_name)?,
        request_timeout_ms: if options.request_timeout_ms == 0 {
            30_000
        } else {
            options.request_timeout_ms
        },
        max_inline_bytes: if options.max_inline_bytes == 0 {
            16 * 1024 * 1024
        } else {
            options.max_inline_bytes
        },
        owner_window_name: str_from_ptr(options.owner_window_name)?
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "clipbus".to_owned()),
    })
}

fn parse_callbacks(
    callbacks: *const clipbus_callbacks,
    user: *mut c_void,
) -> Result<Callbacks, crate::clipboard::Status> {
    if callbacks.is_null() {
        return Err(crate::clipboard::Status::InvalidArgument);
    }
    let callbacks = unsafe { &*callbacks };
    if callbacks.abi_version != CLIPBUS_ABI_VERSION {
        return Err(crate::clipboard::Status::InvalidArgument);
    }
    Ok(Callbacks {
        user: user as usize,
        target_request: callbacks.target_request,
        targets_changed: callbacks.targets_changed,
        owner_lost: callbacks.owner_lost,
        error: callbacks.error,
        target_list: callbacks.target_list,
        target_data: callbacks.target_data,
        stream_ready: callbacks.stream_ready,
    })
}

fn parse_offers(
    targets: *const clipbus_target_offer,
    target_count: usize,
) -> Result<Vec<TargetOffer>, crate::clipboard::Status> {
    if target_count == 0 {
        return Ok(Vec::new());
    }
    if targets.is_null() {
        return Err(crate::clipboard::Status::InvalidArgument);
    }
    let targets = unsafe { slice::from_raw_parts(targets, target_count) };
    let mut offers = Vec::with_capacity(target_count);
    for target in targets {
        let native_target =
            str_from_ptr(target.native_target)?.ok_or(crate::clipboard::Status::InvalidArgument)?;
        if native_target.is_empty() {
            return Err(crate::clipboard::Status::InvalidArgument);
        }
        offers.push(TargetOffer {
            native_target,
            is_promised: target.is_promised != 0,
            max_bytes: target.max_bytes,
        });
    }
    Ok(offers)
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_create(
    options: *const clipbus_options,
    callbacks: *const clipbus_callbacks,
    user: *mut c_void,
    out: *mut *mut clipbus_clipboard,
) -> clipbus_status {
    convert_result(|| {
        if out.is_null() {
            return crate::clipboard::Status::InvalidArgument;
        }
        unsafe {
            *out = ptr::null_mut();
        }
        let options = match parse_options(options) {
            Ok(options) => options,
            Err(status) => return status,
        };
        let callbacks = match parse_callbacks(callbacks, user) {
            Ok(callbacks) => callbacks,
            Err(status) => return status,
        };
        let clipboard = Box::new(clipbus_clipboard {
            inner: Clipboard::new(options, callbacks),
        });
        unsafe {
            *out = Box::into_raw(clipboard);
        }
        crate::clipboard::Status::Ok
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_start(clipboard: *mut clipbus_clipboard) -> clipbus_status {
    convert_result(|| {
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        clipboard.inner.start()
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_stop(clipboard: *mut clipbus_clipboard) -> clipbus_status {
    convert_result(|| {
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        clipboard.inner.stop()
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_destroy(clipboard: *mut clipbus_clipboard) {
    if clipboard.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let boxed = unsafe { Box::from_raw(clipboard) };
        let _ = boxed.inner.stop();
    }));
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_publish_targets(
    clipboard: *mut clipbus_clipboard,
    targets: *const clipbus_target_offer,
    target_count: usize,
) -> clipbus_status {
    convert_result(|| {
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        let offers = match parse_offers(targets, target_count) {
            Ok(offers) => offers,
            Err(status) => return status,
        };
        clipboard.inner.publish_targets(offers)
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_clear(clipboard: *mut clipbus_clipboard) -> clipbus_status {
    convert_result(|| {
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        clipboard.inner.clear()
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_request_targets(
    clipboard: *mut clipbus_clipboard,
    out_request_id: *mut clipbus_request_id_t,
) -> clipbus_status {
    convert_result(|| {
        if out_request_id.is_null() {
            return crate::clipboard::Status::InvalidArgument;
        }
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        match clipboard.inner.request_targets() {
            Ok(request_id) => {
                unsafe {
                    *out_request_id = request_id;
                }
                crate::clipboard::Status::Ok
            }
            Err(status) => status,
        }
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_request_target_data(
    clipboard: *mut clipbus_clipboard,
    native_target: *const c_char,
    max_bytes: u64,
    out_request_id: *mut clipbus_request_id_t,
) -> clipbus_status {
    convert_result(|| {
        if out_request_id.is_null() {
            return crate::clipboard::Status::InvalidArgument;
        }
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        let native_target = match str_from_ptr(native_target) {
            Ok(Some(native_target)) if !native_target.is_empty() => native_target,
            _ => return crate::clipboard::Status::InvalidArgument,
        };
        match clipboard
            .inner
            .request_target_data(native_target, max_bytes)
        {
            Ok(request_id) => {
                unsafe {
                    *out_request_id = request_id;
                }
                crate::clipboard::Status::Ok
            }
            Err(status) => status,
        }
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_complete_request(
    clipboard: *mut clipbus_clipboard,
    request_id: clipbus_request_id_t,
    status: clipbus_status,
    data: *const u8,
    data_len: usize,
) -> clipbus_status {
    convert_result(|| {
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        if data_len > 0 && data.is_null() {
            return crate::clipboard::Status::InvalidArgument;
        }
        let bytes = if data_len == 0 {
            Vec::new()
        } else {
            unsafe { slice::from_raw_parts(data, data_len) }.to_vec()
        };
        clipboard
            .inner
            .complete_request(request_id, status.into(), bytes)
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_begin_request_stream(
    clipboard: *mut clipbus_clipboard,
    request_id: clipbus_request_id_t,
    estimated_bytes: u64,
) -> clipbus_status {
    convert_result(|| {
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        clipboard
            .inner
            .begin_request_stream(request_id, estimated_bytes)
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_write_request_stream(
    clipboard: *mut clipbus_clipboard,
    request_id: clipbus_request_id_t,
    data: *const u8,
    data_len: usize,
) -> clipbus_status {
    convert_result(|| {
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        if data_len == 0 || data.is_null() {
            return crate::clipboard::Status::InvalidArgument;
        }
        let bytes = unsafe { slice::from_raw_parts(data, data_len) }.to_vec();
        clipboard.inner.write_request_stream(request_id, bytes)
    })
}

#[no_mangle]
pub extern "C" fn clipbus_clipboard_end_request_stream(
    clipboard: *mut clipbus_clipboard,
    request_id: clipbus_request_id_t,
    status: clipbus_status,
) -> clipbus_status {
    convert_result(|| {
        let Some(clipboard) = (unsafe { clipboard.as_mut() }) else {
            return crate::clipboard::Status::InvalidArgument;
        };
        clipboard
            .inner
            .end_request_stream(request_id, status.into())
    })
}

#[no_mangle]
pub extern "C" fn clipbus_status_name(status: clipbus_status) -> *const c_char {
    match status {
        clipbus_status::CLIPBUS_OK => c"ok".as_ptr(),
        clipbus_status::CLIPBUS_PENDING => c"pending".as_ptr(),
        clipbus_status::CLIPBUS_UNSUPPORTED => c"unsupported".as_ptr(),
        clipbus_status::CLIPBUS_TIMEOUT => c"timeout".as_ptr(),
        clipbus_status::CLIPBUS_PLATFORM_ERROR => c"platform_error".as_ptr(),
        clipbus_status::CLIPBUS_INVALID_ARGUMENT => c"invalid_argument".as_ptr(),
        clipbus_status::CLIPBUS_ALREADY_STARTED => c"already_started".as_ptr(),
        clipbus_status::CLIPBUS_NOT_STARTED => c"not_started".as_ptr(),
        clipbus_status::CLIPBUS_NOT_FOUND => c"not_found".as_ptr(),
        clipbus_status::CLIPBUS_PANIC => c"panic".as_ptr(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn create_rejects_missing_output() {
        let options = clipbus_options {
            abi_version: CLIPBUS_ABI_VERSION,
            backend: clipbus_backend::CLIPBUS_BACKEND_FAKE,
            display_name: ptr::null(),
            request_timeout_ms: 0,
            max_inline_bytes: 0,
            owner_window_name: ptr::null(),
        };
        let callbacks = clipbus_callbacks {
            abi_version: CLIPBUS_ABI_VERSION,
            target_request: None,
            targets_changed: None,
            owner_lost: None,
            error: None,
            target_list: None,
            target_data: None,
            stream_ready: None,
        };
        assert_eq!(
            clipbus_clipboard_create(&options, &callbacks, ptr::null_mut(), ptr::null_mut()),
            clipbus_status::CLIPBUS_INVALID_ARGUMENT
        );
    }

    #[test]
    fn fake_backend_lifecycle() {
        let options = clipbus_options {
            abi_version: CLIPBUS_ABI_VERSION,
            backend: clipbus_backend::CLIPBUS_BACKEND_FAKE,
            display_name: ptr::null(),
            request_timeout_ms: 0,
            max_inline_bytes: 0,
            owner_window_name: ptr::null(),
        };
        let callbacks = clipbus_callbacks {
            abi_version: CLIPBUS_ABI_VERSION,
            target_request: None,
            targets_changed: None,
            owner_lost: None,
            error: None,
            target_list: None,
            target_data: None,
            stream_ready: None,
        };
        let mut clipboard = ptr::null_mut();
        assert_eq!(
            clipbus_clipboard_create(&options, &callbacks, ptr::null_mut(), &mut clipboard),
            clipbus_status::CLIPBUS_OK
        );
        assert_eq!(
            clipbus_clipboard_start(clipboard),
            clipbus_status::CLIPBUS_OK
        );
        assert_eq!(
            clipbus_clipboard_start(clipboard),
            clipbus_status::CLIPBUS_ALREADY_STARTED
        );
        assert_eq!(
            clipbus_clipboard_stop(clipboard),
            clipbus_status::CLIPBUS_OK
        );
        clipbus_clipboard_destroy(clipboard);
    }
}
