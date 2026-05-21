use crate::clipboard::{Callbacks, Command, Options, Status, TargetOffer};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as XprotoConnectionExt, CreateWindowAux, EventMask, PropMode,
    SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as WrapperConnectionExt;

const CLIPBUS_X11_COPY_FROM_PARENT: u8 = 0;
const CLIPBUS_X11_CURRENT_TIME: u32 = 0;

#[derive(Clone, Copy)]
struct Atoms {
    clipboard: Atom,
    targets: Atom,
    timestamp: Atom,
    save_targets: Atom,
    atom: Atom,
    integer: Atom,
}

#[derive(Clone)]
struct PublishedTarget {
    atom: Atom,
    name: String,
    max_bytes: u64,
}

struct PendingRequest {
    event: SelectionRequestEvent,
    target: PublishedTarget,
    deadline: Instant,
}

pub fn run(options: Options, callbacks: Callbacks, receiver: Receiver<Command>) {
    if let Err(error) = run_x11(options, callbacks, receiver) {
        callbacks.notify_error(Status::PlatformError, &error);
    }
}

pub fn run_fake(_callbacks: Callbacks, receiver: Receiver<Command>) {
    while let Ok(command) = receiver.recv() {
        if matches!(command, Command::Stop) {
            break;
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
        atom: AtomEnum::ATOM.into(),
        integer: AtomEnum::INTEGER.into(),
    };
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

    fn clear_selection(&mut self) -> Result<(), String> {
        self.published.clear();
        self.pending.clear();
        self.conn
            .set_selection_owner(0_u32, self.atoms.clipboard, CLIPBUS_X11_CURRENT_TIME)
            .map_err(|error| format!("failed to clear CLIPBOARD selection: {error}"))?;
        self.conn
            .flush()
            .map_err(|error| format!("failed to flush CLIPBOARD clear: {error}"))?;
        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::SelectionRequest(event) => self.handle_selection_request(event),
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
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1).max(1);
        let timeout = Duration::from_millis(self.options.request_timeout_ms);
        self.pending.insert(
            request_id,
            PendingRequest {
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
        let Some(pending) = self.pending.remove(&request_id) else {
            return Ok(());
        };
        if status != Status::Ok {
            return self.fail_selection_request(&pending.event);
        }
        if data.len() as u64 > pending.target.max_bytes {
            return self.fail_selection_request(&pending.event);
        }
        let property = self.response_property(&pending.event);
        self.conn
            .change_property8(
                PropMode::REPLACE,
                pending.event.requestor,
                property,
                pending.event.target,
                data,
            )
            .map_err(|error| format!("failed to write target data: {error}"))?;
        self.send_selection_notify(&pending.event, property)
    }

    fn expire_pending(&mut self) -> Result<(), String> {
        let now = Instant::now();
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter_map(|(id, pending)| (pending.deadline <= now).then_some(*id))
            .collect();
        for request_id in expired {
            if let Some(pending) = self.pending.remove(&request_id) {
                self.callbacks
                    .notify_error(Status::Timeout, "selection request timed out");
                self.fail_selection_request(&pending.event)?;
            }
        }
        Ok(())
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
}

enum LoopState {
    Continue,
    Stop,
}
