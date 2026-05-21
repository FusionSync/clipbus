use crate::abi::{
    clipbus_error_cb, clipbus_owner_lost_cb, clipbus_status, clipbus_target_request_cb,
    clipbus_targets_changed_cb,
};
use crate::x11;
use std::ffi::{c_void, CString};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Ok,
    Pending,
    Unsupported,
    Timeout,
    PlatformError,
    InvalidArgument,
    AlreadyStarted,
    NotStarted,
    NotFound,
    Panic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendKind {
    X11,
    Fake,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub backend: BackendKind,
    pub display_name: Option<String>,
    pub request_timeout_ms: u64,
    pub max_inline_bytes: u64,
}

#[derive(Clone, Copy)]
pub struct Callbacks {
    pub user: usize,
    pub target_request: clipbus_target_request_cb,
    #[allow(dead_code)]
    pub targets_changed: clipbus_targets_changed_cb,
    pub owner_lost: clipbus_owner_lost_cb,
    pub error: clipbus_error_cb,
}

impl Callbacks {
    fn user_ptr(self) -> *mut c_void {
        self.user as *mut c_void
    }

    pub fn request_target(
        self,
        request_id: u64,
        native_target: &str,
        max_inline_bytes: u64,
        timeout_ms: u64,
    ) -> Status {
        let Some(callback) = self.target_request else {
            return Status::Unsupported;
        };
        let Ok(native_target) = CString::new(native_target) else {
            return Status::InvalidArgument;
        };
        let status = unsafe {
            callback(
                self.user_ptr(),
                request_id,
                native_target.as_ptr(),
                max_inline_bytes,
                timeout_ms,
            )
        };
        match status {
            clipbus_status::CLIPBUS_OK => Status::Ok,
            clipbus_status::CLIPBUS_PENDING => Status::Pending,
            clipbus_status::CLIPBUS_UNSUPPORTED => Status::Unsupported,
            clipbus_status::CLIPBUS_TIMEOUT => Status::Timeout,
            clipbus_status::CLIPBUS_PLATFORM_ERROR => Status::PlatformError,
            clipbus_status::CLIPBUS_INVALID_ARGUMENT => Status::InvalidArgument,
            clipbus_status::CLIPBUS_ALREADY_STARTED => Status::AlreadyStarted,
            clipbus_status::CLIPBUS_NOT_STARTED => Status::NotStarted,
            clipbus_status::CLIPBUS_NOT_FOUND => Status::NotFound,
            clipbus_status::CLIPBUS_PANIC => Status::Panic,
        }
    }

    #[allow(dead_code)]
    pub fn notify_targets_changed(self) {
        if let Some(callback) = self.targets_changed {
            unsafe { callback(self.user_ptr()) };
        }
    }

    pub fn notify_owner_lost(self) {
        if let Some(callback) = self.owner_lost {
            unsafe { callback(self.user_ptr()) };
        }
    }

    pub fn notify_error(self, status: Status, message: &str) {
        let Some(callback) = self.error else {
            return;
        };
        let code = match status {
            Status::Ok => 0,
            Status::Pending => 1,
            Status::Unsupported => 2,
            Status::Timeout => 3,
            Status::PlatformError => 4,
            Status::InvalidArgument => 5,
            Status::AlreadyStarted => 6,
            Status::NotStarted => 7,
            Status::NotFound => 8,
            Status::Panic => 9,
        };
        let sanitized = message.replace('\0', " ");
        if let Ok(message) = CString::new(sanitized) {
            unsafe { callback(self.user_ptr(), code, message.as_ptr()) };
        }
    }
}

#[derive(Clone, Debug)]
pub struct TargetOffer {
    pub native_target: String,
    #[allow(dead_code)]
    pub is_promised: bool,
    pub max_bytes: u64,
}

#[derive(Debug)]
pub enum Command {
    PublishTargets(Vec<TargetOffer>),
    Clear,
    CompleteRequest {
        request_id: u64,
        status: Status,
        data: Vec<u8>,
    },
    Stop,
}

struct RuntimeHandle {
    sender: Sender<Command>,
    join: Option<JoinHandle<()>>,
}

struct ClipboardState {
    runtime: Option<RuntimeHandle>,
}

pub struct Clipboard {
    options: Options,
    callbacks: Callbacks,
    state: Mutex<ClipboardState>,
}

impl Clipboard {
    pub fn new(options: Options, callbacks: Callbacks) -> Self {
        Self {
            options,
            callbacks,
            state: Mutex::new(ClipboardState { runtime: None }),
        }
    }

    pub fn start(&self) -> Status {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return Status::PlatformError,
        };
        if state.runtime.is_some() {
            return Status::AlreadyStarted;
        }
        let (sender, receiver) = mpsc::channel();
        let options = self.options.clone();
        let callbacks = self.callbacks;
        let join =
            thread::Builder::new()
                .name("clipbus-x11".to_owned())
                .spawn(move || match options.backend {
                    BackendKind::X11 => x11::run(options, callbacks, receiver),
                    BackendKind::Fake => x11::run_fake(callbacks, receiver),
                });
        let join = match join {
            Ok(join) => join,
            Err(_) => return Status::PlatformError,
        };
        state.runtime = Some(RuntimeHandle {
            sender,
            join: Some(join),
        });
        Status::Ok
    }

    pub fn stop(&self) -> Status {
        let runtime = {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return Status::PlatformError,
            };
            state.runtime.take()
        };
        let Some(mut runtime) = runtime else {
            return Status::Ok;
        };
        let _ = runtime.sender.send(Command::Stop);
        if let Some(join) = runtime.join.take() {
            if join.join().is_err() {
                return Status::PlatformError;
            }
        }
        Status::Ok
    }

    pub fn publish_targets(&self, offers: Vec<TargetOffer>) -> Status {
        self.send(Command::PublishTargets(offers))
    }

    pub fn clear(&self) -> Status {
        self.send(Command::Clear)
    }

    pub fn complete_request(&self, request_id: u64, status: Status, data: Vec<u8>) -> Status {
        self.send(Command::CompleteRequest {
            request_id,
            status,
            data,
        })
    }

    fn send(&self, command: Command) -> Status {
        let state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return Status::PlatformError,
        };
        let Some(runtime) = &state.runtime else {
            return Status::NotStarted;
        };
        runtime
            .sender
            .send(command)
            .map(|_| Status::Ok)
            .unwrap_or(Status::PlatformError)
    }
}
