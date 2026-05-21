use crate::clipboard::{Callbacks, Command, Options, Status, TargetOffer};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xfixes::{
    ConnectionExt as XfixesConnectionExt, SelectionEventMask,
    SelectionNotifyEvent as XfixesSelectionNotifyEvent,
};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as XprotoConnectionExt,
    CreateWindowAux, EventMask, PropMode, Property, PropertyNotifyEvent, SelectionNotifyEvent,
    SelectionRequestEvent, Window, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

const CLIPBUS_X11_COPY_FROM_PARENT: u8 = 0;
const CLIPBUS_X11_CURRENT_TIME: u32 = 0;
const CLIPBUS_X11_INCR_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    targets: Atom,
    timestamp: Atom,
    save_targets: Atom,
    incr: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
    atom: Atom,
    integer: Atom,
}

#[derive(Clone)]
struct PublishedTarget {
    atom: Atom,
    name: String,
    max_bytes: u64,
}

enum PendingRequest {
    OwnerData {
        event: SelectionRequestEvent,
        target: PublishedTarget,
        deadline: Instant,
    },
    ExternalTargets {
        property: Atom,
        deadline: Instant,
    },
    ExternalData {
        property: Atom,
        target_atom: Atom,
        native_target: String,
        max_bytes: u64,
        deadline: Instant,
    },
    ExternalIncrData {
        property: Atom,
        native_target: String,
        max_bytes: u64,
        data: Vec<u8>,
        deadline: Instant,
    },
    OwnerIncrData {
        requestor: Window,
        property: Atom,
        target_atom: Atom,
        data: Vec<u8>,
        offset: usize,
        chunk_size: usize,
        deadline: Instant,
    },
    OwnerIncrStream {
        requestor: Window,
        property: Atom,
        target_atom: Atom,
        native_target: String,
        max_bytes: u64,
        written_bytes: u64,
        chunk_size: usize,
        ready: bool,
        queued_chunk: Option<Vec<u8>>,
        queued_end: Option<Status>,
        deadline: Instant,
    },
}

struct OwnerIncrStreamState {
    requestor: Window,
    property: Atom,
    target_atom: Atom,
    native_target: String,
    max_bytes: u64,
    written_bytes: u64,
    chunk_size: usize,
    ready: bool,
    queued_chunk: Option<Vec<u8>>,
    queued_end: Option<Status>,
    deadline: Instant,
}

impl From<OwnerIncrStreamState> for PendingRequest {
    fn from(stream: OwnerIncrStreamState) -> Self {
        Self::OwnerIncrStream {
            requestor: stream.requestor,
            property: stream.property,
            target_atom: stream.target_atom,
            native_target: stream.native_target,
            max_bytes: stream.max_bytes,
            written_bytes: stream.written_bytes,
            chunk_size: stream.chunk_size,
            ready: stream.ready,
            queued_chunk: stream.queued_chunk,
            queued_end: stream.queued_end,
            deadline: stream.deadline,
        }
    }
}

impl PendingRequest {
    fn deadline(&self) -> Instant {
        match self {
            Self::OwnerData { deadline, .. }
            | Self::ExternalTargets { deadline, .. }
            | Self::ExternalData { deadline, .. }
            | Self::ExternalIncrData { deadline, .. }
            | Self::OwnerIncrData { deadline, .. }
            | Self::OwnerIncrStream { deadline, .. } => *deadline,
        }
    }

    fn matches_selection_notify(&self, event: &SelectionNotifyEvent, targets_atom: Atom) -> bool {
        match self {
            Self::ExternalTargets { property, .. } => {
                event.target == targets_atom
                    && (event.property == *property || event.property == AtomEnum::NONE.into())
            }
            Self::ExternalData {
                property,
                target_atom,
                ..
            } => {
                event.target == *target_atom
                    && (event.property == *property || event.property == AtomEnum::NONE.into())
            }
            Self::OwnerData { .. } => false,
            Self::ExternalIncrData { .. }
            | Self::OwnerIncrData { .. }
            | Self::OwnerIncrStream { .. } => false,
        }
    }
}

pub fn run(options: Options, callbacks: Callbacks, receiver: Receiver<Command>) {
    if let Err(error) = run_x11(options, callbacks, receiver) {
        callbacks.notify_error(Status::PlatformError, &error);
    }
}

pub fn run_fake(callbacks: Callbacks, receiver: Receiver<Command>) {
    let mut next_request_id = 1_u64;
    while let Ok(command) = receiver.recv() {
        match command {
            Command::RequestTargets { reply } => {
                let request_id = next_request_id;
                next_request_id = next_request_id.saturating_add(1).max(1);
                let _ = reply.send(Ok(request_id));
                callbacks.notify_target_list(request_id, Status::Ok, &[]);
            }
            Command::RequestTargetData {
                native_target,
                reply,
                ..
            } => {
                let request_id = next_request_id;
                next_request_id = next_request_id.saturating_add(1).max(1);
                let _ = reply.send(Ok(request_id));
                callbacks.notify_target_data(request_id, &native_target, Status::Unsupported, &[]);
            }
            Command::Stop => break,
            Command::PublishTargets(_)
            | Command::Clear
            | Command::CompleteRequest { .. }
            | Command::BeginRequestStream { .. }
            | Command::WriteRequestStream { .. }
            | Command::EndRequestStream { .. } => {}
        }
    }
}

fn run_x11(
    options: Options,
    callbacks: Callbacks,
    receiver: Receiver<Command>,
) -> Result<(), String> {
    let (conn, screen_num) = RustConnection::connect(options.display_name.as_deref())
        .map_err(|error| format!("failed to connect to X11 display: {error}"))?;
    let screen = &conn.setup().roots[screen_num];
    let window = conn
        .generate_id()
        .map_err(|error| format!("failed to allocate X11 window id: {error}"))?;
    conn.create_window(
        CLIPBUS_X11_COPY_FROM_PARENT,
        window,
        screen.root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )
    .map_err(|error| format!("failed to create hidden X11 window: {error}"))?;
    let atoms = Atoms {
        clipboard: intern(&conn, b"CLIPBOARD")?,
        targets: intern(&conn, b"TARGETS")?,
        timestamp: intern(&conn, b"TIMESTAMP")?,
        save_targets: intern(&conn, b"SAVE_TARGETS")?,
        incr: intern(&conn, b"INCR")?,
        net_wm_name: intern(&conn, b"_NET_WM_NAME")?,
        utf8_string: intern(&conn, b"UTF8_STRING")?,
        atom: AtomEnum::ATOM.into(),
        integer: AtomEnum::INTEGER.into(),
    };
    set_owner_window_name(&conn, window, atoms, &options.owner_window_name)?;
    if let Err(error) = select_xfixes_clipboard_events(&conn, window, atoms.clipboard) {
        callbacks.notify_error(Status::Unsupported, &error);
    }
    conn.flush()
        .map_err(|error| format!("failed to initialize X11 clipboard window: {error}"))?;

    let mut state = X11State {
        conn,
        callbacks,
        options,
        window,
        atoms,
        published: HashMap::new(),
        pending: HashMap::new(),
        next_request_id: 1,
    };
    state.event_loop(receiver)
}

fn intern(conn: &RustConnection, name: &[u8]) -> Result<Atom, String> {
    conn.intern_atom(false, name)
        .map_err(|error| format!("failed to intern atom {:?}: {error}", name))?
        .reply()
        .map_err(|error| format!("failed to read atom {:?}: {error}", name))
        .map(|reply| reply.atom)
}

fn select_xfixes_clipboard_events(
    conn: &RustConnection,
    window: Window,
    clipboard: Atom,
) -> Result<(), String> {
    conn.xfixes_query_version(5, 0)
        .map_err(|error| format!("XFixes unavailable for clipboard owner tracking: {error}"))?
        .reply()
        .map_err(|error| format!("XFixes unavailable for clipboard owner tracking: {error}"))?;
    conn.xfixes_select_selection_input(
        window,
        clipboard,
        SelectionEventMask::SET_SELECTION_OWNER
            | SelectionEventMask::SELECTION_WINDOW_DESTROY
            | SelectionEventMask::SELECTION_CLIENT_CLOSE,
    )
    .map_err(|error| format!("failed to subscribe to XFixes clipboard events: {error}"))?;
    Ok(())
}

fn set_owner_window_name(
    conn: &RustConnection,
    window: Window,
    atoms: Atoms,
    name: &str,
) -> Result<(), String> {
    let name = name.as_bytes();
    conn.change_property8(
        PropMode::REPLACE,
        window,
        atoms.net_wm_name,
        atoms.utf8_string,
        name,
    )
    .map_err(|error| format!("failed to set _NET_WM_NAME on clipboard window: {error}"))?;
    let wm_name_type = if name.is_ascii() {
        AtomEnum::STRING.into()
    } else {
        atoms.utf8_string
    };
    conn.change_property8(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        wm_name_type,
        name,
    )
    .map_err(|error| format!("failed to set WM_NAME on clipboard window: {error}"))?;
    Ok(())
}

struct X11State {
    conn: RustConnection,
    callbacks: Callbacks,
    options: Options,
    window: Window,
    atoms: Atoms,
    published: HashMap<Atom, PublishedTarget>,
    pending: HashMap<u64, PendingRequest>,
    next_request_id: u64,
}

impl X11State {
    fn event_loop(&mut self, receiver: Receiver<Command>) -> Result<(), String> {
        while let LoopState::Continue = self.drain_commands(&receiver)? {
            while let Some(event) = self
                .conn
                .poll_for_event()
                .map_err(|error| format!("failed to poll X11 event: {error}"))?
            {
                self.handle_event(event)?;
            }
            self.expire_pending()?;
            thread::sleep(Duration::from_millis(5));
        }
        self.clear_selection()?;
        Ok(())
    }

    fn drain_commands(&mut self, receiver: &Receiver<Command>) -> Result<LoopState, String> {
        loop {
            match receiver.try_recv() {
                Ok(Command::PublishTargets(targets)) => self.publish_targets(targets)?,
                Ok(Command::Clear) => self.clear_selection()?,
                Ok(Command::CompleteRequest {
                    request_id,
                    status,
                    data,
                }) => self.complete_request(request_id, status, &data)?,
                Ok(Command::BeginRequestStream {
                    request_id,
                    estimated_bytes,
                }) => self.begin_request_stream(request_id, estimated_bytes)?,
                Ok(Command::WriteRequestStream { request_id, data }) => {
                    self.write_request_stream(request_id, data)?
                }
                Ok(Command::EndRequestStream { request_id, status }) => {
                    self.end_request_stream(request_id, status)?
                }
                Ok(Command::RequestTargets { reply }) => {
                    let _ = reply.send(self.request_external_targets());
                }
                Ok(Command::RequestTargetData {
                    native_target,
                    max_bytes,
                    reply,
                }) => {
                    let _ = reply.send(self.request_external_target_data(native_target, max_bytes));
                }
                Ok(Command::Stop) => return Ok(LoopState::Stop),
                Err(TryRecvError::Empty) => return Ok(LoopState::Continue),
                Err(TryRecvError::Disconnected) => return Ok(LoopState::Stop),
            }
        }
    }

    fn publish_targets(&mut self, targets: Vec<TargetOffer>) -> Result<(), String> {
        self.published.clear();
        for target in targets {
            let atom = intern(&self.conn, target.native_target.as_bytes())?;
            self.published.insert(
                atom,
                PublishedTarget {
                    atom,
                    name: target.native_target,
                    max_bytes: if target.max_bytes == 0 {
                        self.options.max_inline_bytes
                    } else {
                        target.max_bytes
                    },
                },
            );
        }
        self.conn
            .set_selection_owner(self.window, self.atoms.clipboard, CLIPBUS_X11_CURRENT_TIME)
            .map_err(|error| format!("failed to own CLIPBOARD selection: {error}"))?;
        self.conn
            .flush()
            .map_err(|error| format!("failed to flush CLIPBOARD ownership: {error}"))?;
        Ok(())
    }

    fn request_external_targets(&mut self) -> Result<u64, Status> {
        let request_id = self.allocate_request_id();
        let owner = match self
            .conn
            .get_selection_owner(self.atoms.clipboard)
            .map_err(|_| Status::PlatformError)
            .and_then(|cookie| cookie.reply().map_err(|_| Status::PlatformError))
        {
            Ok(reply) => reply.owner,
            Err(status) => return Err(status),
        };
        if owner == 0 {
            self.callbacks
                .notify_target_list(request_id, Status::NotFound, &[]);
            return Ok(request_id);
        }
        if owner == self.window {
            let targets: Vec<String> = self
                .published
                .values()
                .map(|target| target.name.clone())
                .collect();
            self.callbacks
                .notify_target_list(request_id, Status::Ok, &targets);
            return Ok(request_id);
        }
        let property = self
            .request_property_atom(request_id, "CLIPBUS_TARGETS")
            .map_err(|_| Status::PlatformError)?;
        self.pending.insert(
            request_id,
            PendingRequest::ExternalTargets {
                property,
                deadline: Instant::now() + Duration::from_millis(self.options.request_timeout_ms),
            },
        );
        if self
            .conn
            .convert_selection(
                self.window,
                self.atoms.clipboard,
                self.atoms.targets,
                property,
                CLIPBUS_X11_CURRENT_TIME,
            )
            .and_then(|_| self.conn.flush())
            .is_err()
        {
            self.pending.remove(&request_id);
            return Err(Status::PlatformError);
        }
        Ok(request_id)
    }

    fn request_external_target_data(
        &mut self,
        native_target: String,
        max_bytes: u64,
    ) -> Result<u64, Status> {
        let request_id = self.allocate_request_id();
        let owner = match self
            .conn
            .get_selection_owner(self.atoms.clipboard)
            .map_err(|_| Status::PlatformError)
            .and_then(|cookie| cookie.reply().map_err(|_| Status::PlatformError))
        {
            Ok(reply) => reply.owner,
            Err(status) => return Err(status),
        };
        let effective_max_bytes = if max_bytes == 0 {
            self.options.max_inline_bytes
        } else {
            max_bytes
        };
        if owner == 0 {
            self.callbacks
                .notify_target_data(request_id, &native_target, Status::NotFound, &[]);
            return Ok(request_id);
        }
        let target_atom = match intern(&self.conn, native_target.as_bytes()) {
            Ok(atom) => atom,
            Err(_) => return Err(Status::PlatformError),
        };
        let property = self
            .request_property_atom(request_id, "CLIPBUS_DATA")
            .map_err(|_| Status::PlatformError)?;
        self.pending.insert(
            request_id,
            PendingRequest::ExternalData {
                property,
                target_atom,
                native_target,
                max_bytes: effective_max_bytes,
                deadline: Instant::now() + Duration::from_millis(self.options.request_timeout_ms),
            },
        );
        if self
            .conn
            .convert_selection(
                self.window,
                self.atoms.clipboard,
                target_atom,
                property,
                CLIPBUS_X11_CURRENT_TIME,
            )
            .and_then(|_| self.conn.flush())
            .is_err()
        {
            self.pending.remove(&request_id);
            return Err(Status::PlatformError);
        }
        Ok(request_id)
    }

    fn clear_selection(&mut self) -> Result<(), String> {
        self.published.clear();
        self.pending.clear();
        if self.current_selection_owner()? == self.window {
            self.conn
                .set_selection_owner(0_u32, self.atoms.clipboard, CLIPBUS_X11_CURRENT_TIME)
                .map_err(|error| format!("failed to clear CLIPBOARD selection: {error}"))?;
            self.conn
                .flush()
                .map_err(|error| format!("failed to flush CLIPBOARD clear: {error}"))?;
        }
        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::SelectionRequest(event) => self.handle_selection_request(event),
            Event::SelectionNotify(event) => self.handle_selection_notify(event),
            Event::PropertyNotify(event) => self.handle_property_notify(event),
            Event::XfixesSelectionNotify(event) => self.handle_xfixes_selection_notify(event),
            Event::SelectionClear(_) => {
                self.published.clear();
                self.pending.clear();
                self.callbacks.notify_owner_lost();
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn handle_selection_request(&mut self, event: SelectionRequestEvent) -> Result<(), String> {
        if event.selection != self.atoms.clipboard {
            return self.fail_selection_request(&event);
        }
        if event.target == self.atoms.targets {
            return self.respond_targets(&event);
        }
        if event.target == self.atoms.timestamp {
            return self.respond_u32(&event, self.atoms.integer, CLIPBUS_X11_CURRENT_TIME);
        }
        if event.target == self.atoms.save_targets {
            return self.respond_empty(&event);
        }
        let Some(target) = self.published.get(&event.target).cloned() else {
            return self.fail_selection_request(&event);
        };
        let request_id = self.allocate_request_id();
        let timeout = Duration::from_millis(self.options.request_timeout_ms);
        self.pending.insert(
            request_id,
            PendingRequest::OwnerData {
                event,
                target: target.clone(),
                deadline: Instant::now() + timeout,
            },
        );
        match self.callbacks.request_target(
            request_id,
            &target.name,
            target.max_bytes,
            self.options.request_timeout_ms,
        ) {
            Status::Ok | Status::Pending => Ok(()),
            status => {
                self.pending.remove(&request_id);
                self.callbacks
                    .notify_error(status, "target request callback rejected request");
                self.fail_selection_request(&event)
            }
        }
    }

    fn handle_selection_notify(&mut self, event: SelectionNotifyEvent) -> Result<(), String> {
        if event.selection != self.atoms.clipboard {
            return Ok(());
        }
        let Some(request_id) = self.pending.iter().find_map(|(request_id, pending)| {
            pending
                .matches_selection_notify(&event, self.atoms.targets)
                .then_some(*request_id)
        }) else {
            return Ok(());
        };
        let Some(pending) = self.pending.remove(&request_id) else {
            return Ok(());
        };
        if event.property == AtomEnum::NONE.into() {
            self.notify_external_pending_failure(request_id, pending, Status::Unsupported);
            return Ok(());
        }
        match pending {
            PendingRequest::ExternalTargets { property, .. } => {
                self.complete_external_targets(request_id, property)
            }
            PendingRequest::ExternalData {
                property,
                native_target,
                max_bytes,
                ..
            } => self.complete_external_data(request_id, property, &native_target, max_bytes),
            PendingRequest::OwnerData { .. } => Ok(()),
            PendingRequest::ExternalIncrData { .. }
            | PendingRequest::OwnerIncrData { .. }
            | PendingRequest::OwnerIncrStream { .. } => Ok(()),
        }
    }

    fn handle_property_notify(&mut self, event: PropertyNotifyEvent) -> Result<(), String> {
        if event.window == self.window && event.state == Property::NEW_VALUE {
            if let Some(request_id) = self.pending.iter().find_map(|(request_id, pending)| {
                matches!(
                    pending,
                    PendingRequest::ExternalIncrData { property, .. } if *property == event.atom
                )
                .then_some(*request_id)
            }) {
                return self.read_external_incr_chunk(request_id);
            }
        }

        if event.state == Property::DELETE {
            if let Some(request_id) =
                self.pending
                    .iter()
                    .find_map(|(request_id, pending)| match pending {
                        PendingRequest::OwnerIncrData {
                            requestor,
                            property,
                            ..
                        }
                        | PendingRequest::OwnerIncrStream {
                            requestor,
                            property,
                            ..
                        } if *requestor == event.window && *property == event.atom => {
                            Some(*request_id)
                        }
                        _ => None,
                    })
            {
                return self.advance_owner_incr_request(request_id);
            }
        }

        Ok(())
    }

    fn handle_xfixes_selection_notify(
        &self,
        event: XfixesSelectionNotifyEvent,
    ) -> Result<(), String> {
        if event.selection == self.atoms.clipboard && event.owner != self.window {
            self.callbacks.notify_targets_changed();
        }
        Ok(())
    }

    fn respond_targets(&self, event: &SelectionRequestEvent) -> Result<(), String> {
        let mut atoms = Vec::with_capacity(self.published.len() + 3);
        atoms.push(self.atoms.targets);
        atoms.push(self.atoms.timestamp);
        atoms.push(self.atoms.save_targets);
        atoms.extend(self.published.values().map(|target| target.atom));
        let property = self.response_property(event);
        self.conn
            .change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                self.atoms.atom,
                &atoms,
            )
            .map_err(|error| format!("failed to write TARGETS property: {error}"))?;
        self.send_selection_notify(event, property)
    }

    fn respond_u32(
        &self,
        event: &SelectionRequestEvent,
        property_type: Atom,
        value: u32,
    ) -> Result<(), String> {
        let property = self.response_property(event);
        self.conn
            .change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                property_type,
                &[value],
            )
            .map_err(|error| format!("failed to write u32 selection property: {error}"))?;
        self.send_selection_notify(event, property)
    }

    fn respond_empty(&self, event: &SelectionRequestEvent) -> Result<(), String> {
        let property = self.response_property(event);
        self.conn
            .change_property8(
                PropMode::REPLACE,
                event.requestor,
                property,
                event.target,
                &[],
            )
            .map_err(|error| format!("failed to write empty selection property: {error}"))?;
        self.send_selection_notify(event, property)
    }

    fn complete_request(
        &mut self,
        request_id: u64,
        status: Status,
        data: &[u8],
    ) -> Result<(), String> {
        let Some(PendingRequest::OwnerData {
            event,
            target,
            deadline: _,
        }) = self.pending.remove(&request_id)
        else {
            return Ok(());
        };
        if status != Status::Ok {
            return self.fail_selection_request(&event);
        }
        if data.len() as u64 > target.max_bytes {
            return self.fail_selection_request(&event);
        }
        if data.len() > self.inline_data_limit() {
            return self.begin_owner_incr_request(request_id, event, target, data.to_vec());
        }
        let property = self.response_property(&event);
        self.conn
            .change_property8(
                PropMode::REPLACE,
                event.requestor,
                property,
                event.target,
                data,
            )
            .map_err(|error| format!("failed to write target data: {error}"))?;
        self.send_selection_notify(&event, property)
    }

    fn begin_owner_incr_request(
        &mut self,
        request_id: u64,
        event: SelectionRequestEvent,
        target: PublishedTarget,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let property = self.response_property(&event);
        self.conn
            .change_window_attributes(
                event.requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(|error| {
                format!("failed to subscribe to INCR requestor property events: {error}")
            })?;
        let lower_bound = data.len().min(u32::MAX as usize) as u32;
        self.conn
            .change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                self.atoms.incr,
                &[lower_bound],
            )
            .map_err(|error| format!("failed to write INCR header: {error}"))?;
        self.send_selection_notify(&event, property)?;
        self.pending.insert(
            request_id,
            PendingRequest::OwnerIncrData {
                requestor: event.requestor,
                property,
                target_atom: target.atom,
                data,
                offset: 0,
                chunk_size: self.incr_chunk_size(),
                deadline: Instant::now() + Duration::from_millis(self.options.request_timeout_ms),
            },
        );
        Ok(())
    }

    fn begin_request_stream(
        &mut self,
        request_id: u64,
        estimated_bytes: u64,
    ) -> Result<(), String> {
        let Some(PendingRequest::OwnerData {
            event,
            target,
            deadline: _,
        }) = self.pending.remove(&request_id)
        else {
            self.callbacks
                .notify_error(Status::NotFound, "stream begin request id was not pending");
            return Ok(());
        };
        if estimated_bytes > target.max_bytes {
            self.callbacks.notify_error(
                Status::InvalidArgument,
                "stream estimated bytes exceeds target limit",
            );
            return self.fail_selection_request(&event);
        }

        let property = self.response_property(&event);
        self.conn
            .change_window_attributes(
                event.requestor,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )
            .map_err(|error| {
                format!("failed to subscribe to streaming INCR requestor events: {error}")
            })?;
        let lower_bound = estimated_bytes.min(u32::MAX as u64) as u32;
        self.conn
            .change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                self.atoms.incr,
                &[lower_bound],
            )
            .map_err(|error| format!("failed to write streaming INCR header: {error}"))?;
        self.send_selection_notify(&event, property)?;
        self.pending.insert(
            request_id,
            PendingRequest::OwnerIncrStream {
                requestor: event.requestor,
                property,
                target_atom: target.atom,
                native_target: target.name,
                max_bytes: target.max_bytes,
                written_bytes: 0,
                chunk_size: self.incr_chunk_size(),
                ready: false,
                queued_chunk: None,
                queued_end: None,
                deadline: Instant::now() + Duration::from_millis(self.options.request_timeout_ms),
            },
        );
        Ok(())
    }

    fn write_request_stream(&mut self, request_id: u64, data: Vec<u8>) -> Result<(), String> {
        let Some(PendingRequest::OwnerIncrStream {
            requestor,
            property,
            target_atom,
            native_target,
            max_bytes,
            written_bytes,
            chunk_size,
            ready,
            queued_chunk,
            queued_end,
            deadline,
        }) = self.pending.remove(&request_id)
        else {
            self.callbacks
                .notify_error(Status::NotFound, "stream write request id was not pending");
            return Ok(());
        };

        let mut stream = OwnerIncrStreamState {
            requestor,
            property,
            target_atom,
            native_target,
            max_bytes,
            written_bytes,
            chunk_size,
            ready,
            queued_chunk,
            queued_end,
            deadline,
        };

        if data.is_empty() || data.len() > stream.chunk_size {
            self.callbacks.notify_error(
                Status::InvalidArgument,
                "stream chunk is empty or exceeds max_chunk_bytes",
            );
            self.pending
                .insert(request_id, PendingRequest::from(stream));
            return Ok(());
        }
        if stream.written_bytes.saturating_add(data.len() as u64) > stream.max_bytes {
            self.callbacks
                .notify_error(Status::InvalidArgument, "stream exceeds target limit");
            self.abort_owner_stream(request_id, stream)?;
            return Ok(());
        }
        if stream.queued_end.is_some() {
            self.callbacks.notify_error(
                Status::InvalidArgument,
                "stream chunk written after stream end was queued",
            );
            self.pending
                .insert(request_id, PendingRequest::from(stream));
            return Ok(());
        }
        if stream.ready {
            self.write_owner_stream_chunk(request_id, stream, data)
        } else if stream.queued_chunk.is_none() {
            stream.queued_chunk = Some(data);
            stream.deadline = self.owner_request_deadline();
            self.pending
                .insert(request_id, PendingRequest::from(stream));
            Ok(())
        } else {
            self.callbacks.notify_error(
                Status::Pending,
                "stream already has a queued chunk waiting for requestor readiness",
            );
            self.pending
                .insert(request_id, PendingRequest::from(stream));
            Ok(())
        }
    }

    fn end_request_stream(&mut self, request_id: u64, status: Status) -> Result<(), String> {
        let Some(PendingRequest::OwnerIncrStream {
            requestor,
            property,
            target_atom,
            native_target,
            max_bytes,
            written_bytes,
            chunk_size,
            ready,
            queued_chunk,
            queued_end,
            deadline,
        }) = self.pending.remove(&request_id)
        else {
            self.callbacks
                .notify_error(Status::NotFound, "stream end request id was not pending");
            return Ok(());
        };

        let mut stream = OwnerIncrStreamState {
            requestor,
            property,
            target_atom,
            native_target,
            max_bytes,
            written_bytes,
            chunk_size,
            ready,
            queued_chunk,
            queued_end,
            deadline,
        };
        if status != Status::Ok {
            stream.queued_chunk = None;
            self.callbacks
                .notify_error(status, "stream ended with non-ok status");
        }
        stream.queued_end = Some(status);
        stream.deadline = self.owner_request_deadline();
        self.advance_owner_stream(request_id, stream)
    }

    fn expire_pending(&mut self) -> Result<(), String> {
        let now = Instant::now();
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter_map(|(id, pending)| (pending.deadline() <= now).then_some(*id))
            .collect();
        for request_id in expired {
            if let Some(pending) = self.pending.remove(&request_id) {
                self.callbacks
                    .notify_error(Status::Timeout, "selection request timed out");
                match pending {
                    PendingRequest::OwnerData { event, .. } => {
                        self.fail_selection_request(&event)?;
                    }
                    PendingRequest::OwnerIncrData {
                        requestor,
                        property,
                        target_atom,
                        ..
                    }
                    | PendingRequest::OwnerIncrStream {
                        requestor,
                        property,
                        target_atom,
                        ..
                    } => {
                        let _ = self.conn.change_property8(
                            PropMode::REPLACE,
                            requestor,
                            property,
                            target_atom,
                            &[],
                        );
                        let _ = self.conn.flush();
                    }
                    other => {
                        self.notify_external_pending_failure(request_id, other, Status::Timeout)
                    }
                }
            }
        }
        Ok(())
    }

    fn complete_external_targets(&self, request_id: u64, property: Atom) -> Result<(), String> {
        let reply = self
            .conn
            .get_property(false, self.window, property, self.atoms.atom, 0, 4096)
            .map_err(|error| format!("failed to request TARGETS property: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read TARGETS property: {error}"))?;
        let targets = if reply.type_ == self.atoms.atom && reply.format == 32 {
            let mut names = Vec::new();
            if let Some(values) = reply.value32() {
                for atom in values {
                    if let Ok(name) = self.atom_name(atom) {
                        names.push(name);
                    }
                }
            }
            names
        } else {
            Vec::new()
        };
        let _ = self.conn.delete_property(self.window, property);
        let _ = self.conn.flush();
        self.callbacks
            .notify_target_list(request_id, Status::Ok, &targets);
        Ok(())
    }

    fn complete_external_data(
        &mut self,
        request_id: u64,
        property: Atom,
        native_target: &str,
        max_bytes: u64,
    ) -> Result<(), String> {
        let units = max_bytes
            .saturating_add(3)
            .checked_div(4)
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32;
        let reply = self
            .conn
            .get_property(false, self.window, property, AtomEnum::ANY, 0, units)
            .map_err(|error| format!("failed to request target property: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read target property: {error}"))?;
        if reply.type_ == self.atoms.incr {
            return self.begin_external_incr_read(request_id, property, native_target, max_bytes);
        }
        let status = if reply.type_ == self.atoms.incr
            || reply.bytes_after != 0
            || reply.value.len() as u64 > max_bytes
        {
            Status::Unsupported
        } else {
            Status::Ok
        };
        let data = if status == Status::Ok {
            reply.value.as_slice()
        } else {
            &[]
        };
        let _ = self.conn.delete_property(self.window, property);
        let _ = self.conn.flush();
        self.callbacks
            .notify_target_data(request_id, native_target, status, data);
        Ok(())
    }

    fn begin_external_incr_read(
        &mut self,
        request_id: u64,
        property: Atom,
        native_target: &str,
        max_bytes: u64,
    ) -> Result<(), String> {
        let reply = self
            .conn
            .get_property(false, self.window, property, self.atoms.incr, 0, 1)
            .map_err(|error| format!("failed to request INCR header: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read INCR header: {error}"))?;
        let expected_bytes = reply
            .value32()
            .and_then(|mut values| values.next())
            .unwrap_or(0) as u64;
        if expected_bytes > max_bytes {
            let _ = self.conn.delete_property(self.window, property);
            let _ = self.conn.flush();
            self.callbacks
                .notify_target_data(request_id, native_target, Status::Unsupported, &[]);
            return Ok(());
        }
        self.conn
            .delete_property(self.window, property)
            .map_err(|error| format!("failed to delete INCR header property: {error}"))?;
        self.conn
            .flush()
            .map_err(|error| format!("failed to flush INCR header deletion: {error}"))?;
        let capacity = expected_bytes.min(max_bytes).min(usize::MAX as u64) as usize;
        self.pending.insert(
            request_id,
            PendingRequest::ExternalIncrData {
                property,
                native_target: native_target.to_owned(),
                max_bytes,
                data: Vec::with_capacity(capacity),
                deadline: Instant::now() + Duration::from_millis(self.options.request_timeout_ms),
            },
        );
        Ok(())
    }

    fn read_external_incr_chunk(&mut self, request_id: u64) -> Result<(), String> {
        let Some(PendingRequest::ExternalIncrData {
            property,
            native_target,
            max_bytes,
            mut data,
            deadline,
        }) = self.pending.remove(&request_id)
        else {
            return Ok(());
        };
        let remaining = max_bytes.saturating_sub(data.len() as u64);
        let units = property_units_for_bytes(remaining);
        let reply = self
            .conn
            .get_property(true, self.window, property, AtomEnum::ANY, 0, units)
            .map_err(|error| format!("failed to request INCR data chunk: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read INCR data chunk: {error}"))?;
        if reply.value.is_empty() && reply.bytes_after == 0 {
            let _ = self.conn.flush();
            self.callbacks
                .notify_target_data(request_id, &native_target, Status::Ok, &data);
            return Ok(());
        }
        if reply.bytes_after != 0 || reply.value.len() as u64 > remaining {
            let _ = self.conn.delete_property(self.window, property);
            let _ = self.conn.flush();
            self.callbacks
                .notify_target_data(request_id, &native_target, Status::Unsupported, &[]);
            return Ok(());
        }
        data.extend_from_slice(&reply.value);
        let _ = self.conn.flush();
        self.pending.insert(
            request_id,
            PendingRequest::ExternalIncrData {
                property,
                native_target,
                max_bytes,
                data,
                deadline,
            },
        );
        Ok(())
    }

    fn advance_owner_incr_request(&mut self, request_id: u64) -> Result<(), String> {
        match self.pending.get(&request_id) {
            Some(PendingRequest::OwnerIncrData { .. }) => {
                self.send_next_owner_incr_chunk(request_id)
            }
            Some(PendingRequest::OwnerIncrStream { .. }) => {
                self.mark_owner_stream_ready(request_id)
            }
            _ => Ok(()),
        }
    }

    fn mark_owner_stream_ready(&mut self, request_id: u64) -> Result<(), String> {
        let Some(PendingRequest::OwnerIncrStream {
            requestor,
            property,
            target_atom,
            native_target,
            max_bytes,
            written_bytes,
            chunk_size,
            ready: _,
            queued_chunk,
            queued_end,
            deadline: _,
        }) = self.pending.remove(&request_id)
        else {
            return Ok(());
        };
        let stream = OwnerIncrStreamState {
            requestor,
            property,
            target_atom,
            native_target,
            max_bytes,
            written_bytes,
            chunk_size,
            ready: true,
            queued_chunk,
            queued_end,
            deadline: self.owner_request_deadline(),
        };
        self.advance_owner_stream(request_id, stream)
    }

    fn advance_owner_stream(
        &mut self,
        request_id: u64,
        mut stream: OwnerIncrStreamState,
    ) -> Result<(), String> {
        if !stream.ready {
            self.pending
                .insert(request_id, PendingRequest::from(stream));
            return Ok(());
        }
        if let Some(chunk) = stream.queued_chunk.take() {
            return self.write_owner_stream_chunk(request_id, stream, chunk);
        }
        if stream.queued_end.is_some() {
            return self.finish_owner_stream(request_id, stream);
        }
        stream.deadline = self.owner_request_deadline();
        self.callbacks.notify_stream_ready(
            request_id,
            &stream.native_target,
            stream.chunk_size as u64,
            self.options.request_timeout_ms,
        );
        self.pending
            .insert(request_id, PendingRequest::from(stream));
        Ok(())
    }

    fn write_owner_stream_chunk(
        &mut self,
        request_id: u64,
        mut stream: OwnerIncrStreamState,
        chunk: Vec<u8>,
    ) -> Result<(), String> {
        self.conn
            .change_property8(
                PropMode::REPLACE,
                stream.requestor,
                stream.property,
                stream.target_atom,
                &chunk,
            )
            .map_err(|error| format!("failed to write streaming INCR chunk: {error}"))?;
        self.conn
            .flush()
            .map_err(|error| format!("failed to flush streaming INCR chunk: {error}"))?;
        stream.written_bytes = stream.written_bytes.saturating_add(chunk.len() as u64);
        stream.ready = false;
        stream.deadline = self.owner_request_deadline();
        self.pending
            .insert(request_id, PendingRequest::from(stream));
        Ok(())
    }

    fn finish_owner_stream(
        &mut self,
        _request_id: u64,
        stream: OwnerIncrStreamState,
    ) -> Result<(), String> {
        self.conn
            .change_property8(
                PropMode::REPLACE,
                stream.requestor,
                stream.property,
                stream.target_atom,
                &[],
            )
            .map_err(|error| format!("failed to write streaming INCR terminator: {error}"))?;
        self.conn
            .flush()
            .map_err(|error| format!("failed to flush streaming INCR terminator: {error}"))
    }

    fn abort_owner_stream(
        &mut self,
        request_id: u64,
        mut stream: OwnerIncrStreamState,
    ) -> Result<(), String> {
        stream.queued_chunk = None;
        stream.queued_end = Some(Status::InvalidArgument);
        self.advance_owner_stream(request_id, stream)
    }

    fn send_next_owner_incr_chunk(&mut self, request_id: u64) -> Result<(), String> {
        let Some(PendingRequest::OwnerIncrData {
            requestor,
            property,
            target_atom,
            data,
            offset,
            chunk_size,
            deadline,
        }) = self.pending.remove(&request_id)
        else {
            return Ok(());
        };
        let end = offset.saturating_add(chunk_size).min(data.len());
        let chunk = &data[offset..end];
        self.conn
            .change_property8(PropMode::REPLACE, requestor, property, target_atom, chunk)
            .map_err(|error| format!("failed to write INCR data chunk: {error}"))?;
        self.conn
            .flush()
            .map_err(|error| format!("failed to flush INCR data chunk: {error}"))?;
        if chunk.is_empty() {
            return Ok(());
        }
        self.pending.insert(
            request_id,
            PendingRequest::OwnerIncrData {
                requestor,
                property,
                target_atom,
                data,
                offset: end,
                chunk_size,
                deadline,
            },
        );
        Ok(())
    }

    fn notify_external_pending_failure(
        &self,
        request_id: u64,
        pending: PendingRequest,
        status: Status,
    ) {
        match pending {
            PendingRequest::ExternalTargets { .. } => {
                self.callbacks.notify_target_list(request_id, status, &[]);
            }
            PendingRequest::ExternalData { native_target, .. } => {
                self.callbacks
                    .notify_target_data(request_id, &native_target, status, &[]);
            }
            PendingRequest::ExternalIncrData { native_target, .. } => {
                self.callbacks
                    .notify_target_data(request_id, &native_target, status, &[]);
            }
            PendingRequest::OwnerData { .. }
            | PendingRequest::OwnerIncrData { .. }
            | PendingRequest::OwnerIncrStream { .. } => {}
        }
    }

    fn fail_selection_request(&self, event: &SelectionRequestEvent) -> Result<(), String> {
        self.send_selection_notify(event, AtomEnum::NONE.into())
    }

    fn response_property(&self, event: &SelectionRequestEvent) -> Atom {
        if event.property == AtomEnum::NONE.into() {
            event.target
        } else {
            event.property
        }
    }

    fn send_selection_notify(
        &self,
        event: &SelectionRequestEvent,
        property: Atom,
    ) -> Result<(), String> {
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: event.time,
            requestor: event.requestor,
            selection: event.selection,
            target: event.target,
            property,
        };
        self.conn
            .send_event(false, event.requestor, EventMask::NO_EVENT, notify)
            .map_err(|error| format!("failed to send SelectionNotify: {error}"))?;
        self.conn
            .flush()
            .map_err(|error| format!("failed to flush SelectionNotify: {error}"))
    }

    fn allocate_request_id(&mut self) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1).max(1);
        request_id
    }

    fn request_property_atom(&self, request_id: u64, prefix: &str) -> Result<Atom, String> {
        intern(&self.conn, format!("{prefix}_{request_id}").as_bytes())
    }

    fn atom_name(&self, atom: Atom) -> Result<String, String> {
        self.conn
            .get_atom_name(atom)
            .map_err(|error| format!("failed to request atom name: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read atom name: {error}"))
            .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
    }

    fn current_selection_owner(&self) -> Result<Window, String> {
        self.conn
            .get_selection_owner(self.atoms.clipboard)
            .map_err(|error| format!("failed to request CLIPBOARD owner: {error}"))?
            .reply()
            .map_err(|error| format!("failed to read CLIPBOARD owner: {error}"))
            .map(|reply| reply.owner)
    }

    fn inline_data_limit(&self) -> usize {
        (self.options.max_inline_bytes.min(usize::MAX as u64) as usize)
            .min(self.max_change_property_payload_bytes())
    }

    fn incr_chunk_size(&self) -> usize {
        CLIPBUS_X11_INCR_CHUNK_BYTES.min(self.max_change_property_payload_bytes())
    }

    fn max_change_property_payload_bytes(&self) -> usize {
        self.conn
            .maximum_request_bytes()
            .saturating_sub(1024)
            .max(1)
    }

    fn owner_request_deadline(&self) -> Instant {
        Instant::now() + Duration::from_millis(self.options.request_timeout_ms)
    }
}

enum LoopState {
    Continue,
    Stop,
}

fn property_units_for_bytes(bytes: u64) -> u32 {
    bytes
        .saturating_add(3)
        .checked_div(4)
        .unwrap_or(0)
        .min(u32::MAX as u64) as u32
}
