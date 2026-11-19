//! packarch: drag'n'drop between Wayland and X11 clients (XDND, version 5)
//!
//! Wayland → X11: the compositor announces the drag with
//! [`X11Wm::dnd_source`], then follows the pointer over X windows with
//! [`X11Wm::dnd_motion`] and ends it with [`X11Wm::dnd_drop`]. The XWM owns
//! `XdndSelection` and speaks XDND to the X window under the pointer; X
//! clients read the data through [`XwmHandler::send_dnd_data`]. Answers of
//! the X window come back as [`XwmHandler::dnd_target_status`] and
//! [`XwmHandler::dnd_target_finished`].
//!
//! X11 → Wayland: while an X client drags (it owns `XdndSelection`), the
//! compositor shows a proxy window above every X window when the pointer is
//! over a Wayland surface ([`X11Wm::set_dnd_proxy`]). The proxy is the XDND
//! target: its messages come as [`XwmHandler::x11_drag_enter`],
//! [`XwmHandler::x11_drag_motion`], [`XwmHandler::x11_drag_leave`] and
//! [`XwmHandler::x11_drag_drop`]; the compositor answers with
//! [`X11Wm::x11_drag_status`], reads the data with [`X11Wm::x11_drag_receive`]
//! and ends with [`X11Wm::x11_drag_finished`].

use super::*;

use wayland_server::protocol::wl_data_device_manager::DndAction;
use x11rb::protocol::xproto::ClientMessageEvent;

/// Version of the protocol spoken by the XWM
const XDND_VERSION: u32 = 5;

/// Drag'n'drop state of the XWM
#[derive(Debug)]
pub(super) struct XDnd {
    /// Target window of X drags over Wayland surfaces
    pub(super) proxy: X11Window,
    proxy_shown: bool,
    /// Types of the Wayland drag (`XdndTypeList`)
    types: Vec<Atom>,
    /// Wayland drag over an X window
    out: Option<Outgoing>,
    /// X drag over the proxy window
    incoming: Option<IncomingDrag>,
}

#[derive(Debug)]
struct Outgoing {
    /// X window under the pointer (`XdndAware`)
    target: X11Window,
    /// Window receiving the messages (`XdndProxy` or the target)
    dest: X11Window,
    version: u32,
    /// An `XdndPosition` waits for its `XdndStatus`
    waiting_status: bool,
    /// Last position not yet sent (root coordinates, action)
    pending: Option<(i16, i16, Atom)>,
    accepted: bool,
    dropped: bool,
}

#[derive(Debug)]
struct IncomingDrag {
    /// Window of the X drag source
    source: X11Window,
    accepted: bool,
    action: DndAction,
    /// Time of `XdndDrop`
    time: u32,
}

impl XDnd {
    pub(super) fn new(conn: &RustConnection, screen: &Screen, atoms: &Atoms) -> Result<Self, ReplyOrIdError> {
        // Invisible window covering every screen, XDND target
        let proxy = conn.generate_id()?;
        conn.create_window(
            0,
            proxy,
            screen.root,
            0,
            0,
            i16::MAX as u16,
            i16::MAX as u16,
            0,
            WindowClass::INPUT_ONLY,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new().override_redirect(1),
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            proxy,
            atoms.XdndAware,
            AtomEnum::ATOM,
            &[XDND_VERSION],
        )?;
        Ok(XDnd {
            proxy,
            proxy_shown: false,
            types: Vec::new(),
            out: None,
            incoming: None,
        })
    }

    /// Time of the drop of the X drag, for `ConvertSelection`
    pub(super) fn drop_time(&self) -> u32 {
        self.incoming
            .as_ref()
            .map(|i| i.time)
            .unwrap_or(x11rb::CURRENT_TIME)
    }
}

/// Is this client message part of XDND?
pub(super) fn is_xdnd_message(atoms: &Atoms, type_: Atom) -> bool {
    [
        atoms.XdndEnter,
        atoms.XdndPosition,
        atoms.XdndStatus,
        atoms.XdndLeave,
        atoms.XdndDrop,
        atoms.XdndFinished,
    ]
    .contains(&type_)
}

fn action_atom(atoms: &Atoms, action: DndAction) -> Atom {
    if action.contains(DndAction::Copy) {
        atoms.XdndActionCopy
    } else if action.contains(DndAction::Move) {
        atoms.XdndActionMove
    } else if action.contains(DndAction::Ask) {
        atoms.XdndActionAsk
    } else {
        x11rb::NONE
    }
}

fn atom_action(atoms: &Atoms, atom: Atom) -> DndAction {
    match atom {
        x if x == atoms.XdndActionCopy || x == atoms.XdndActionPrivate => DndAction::Copy,
        x if x == atoms.XdndActionMove => DndAction::Move,
        x if x == atoms.XdndActionAsk => DndAction::Ask,
        _ => DndAction::empty(),
    }
}

fn mime_atom(conn: &RustConnection, atoms: &Atoms, mime: &str) -> Option<Atom> {
    Some(match mime {
        "text/plain;charset=utf-8" => atoms.UTF8_STRING,
        "text/plain" => atoms.TEXT,
        mime => conn.intern_atom(false, mime.as_bytes()).ok()?.reply().ok()?.atom,
    })
}

fn atom_mime(conn: &RustConnection, atoms: &Atoms, atom: Atom) -> Option<String> {
    match atom {
        x if x == atoms.UTF8_STRING => Some("text/plain;charset=utf-8".into()),
        x if x == atoms.TEXT => Some("text/plain".into()),
        x => {
            let name = conn.get_atom_name(x).ok()?.reply().ok()?.name;
            let name = String::from_utf8(name).ok()?;
            // hopefully a mime-type
            name.contains('/').then_some(name)
        }
    }
}

/// First value of a 32-bit property
fn property32(conn: &RustConnection, window: X11Window, prop: Atom) -> Result<Option<u32>, ConnectionError> {
    Ok(conn
        .get_property(false, window, prop, AtomEnum::ANY, 0, 1)?
        .reply_unchecked()?
        .and_then(|r| r.value32().and_then(|mut v| v.next())))
}

/// Atoms of a property (list)
fn atom_list(conn: &RustConnection, window: X11Window, prop: Atom) -> Result<Vec<Atom>, ConnectionError> {
    Ok(conn
        .get_property(false, window, prop, AtomEnum::ATOM, 0, 4096)?
        .reply_unchecked()?
        .and_then(|r| r.value32().map(|v| v.collect()))
        .unwrap_or_default())
}

fn send_message(
    conn: &RustConnection,
    dest: X11Window,
    window: X11Window,
    type_: Atom,
    data: [u32; 5],
) -> Result<(), ConnectionError> {
    let event = ClientMessageEvent::new(32, window, type_, data);
    conn.send_event(false, dest, EventMask::NO_EVENT, event)?;
    Ok(())
}

impl X11Wm {
    // --- Wayland → X11 ---------------------------------------------------

    /// packarch: a Wayland drag starts with the given mime types (the XWM
    /// owns `XdndSelection`), or ends (`None`)
    pub fn dnd_source(&mut self, mime_types: Option<Vec<String>>) -> Result<(), ConnectionError> {
        match mime_types {
            Some(mime_types) => {
                self.xdnd.out = None;
                self.xdnd.types = mime_types
                    .iter()
                    .filter_map(|m| mime_atom(&self.conn, &self.atoms, m))
                    .collect();
                self.conn.change_property32(
                    PropMode::REPLACE,
                    self.dnd.window,
                    self.atoms.XdndTypeList,
                    AtomEnum::ATOM,
                    &self.xdnd.types,
                )?;
                self.dnd.mime_types = mime_types;
                self.conn
                    .set_selection_owner(self.dnd.window, self.atoms.XdndSelection, x11rb::CURRENT_TIME)?;
            }
            None => {
                if self.xdnd.out.as_ref().is_some_and(|o| !o.dropped) {
                    self.dnd_leave_target()?;
                }
                self.xdnd.out = None;
                self.xdnd.types.clear();
                self.dnd.mime_types.clear();
                if self.dnd.owner == self.dnd.window {
                    self.conn
                        .set_selection_owner(x11rb::NONE, self.atoms.XdndSelection, self.dnd.timestamp)?;
                }
            }
        }
        self.conn.flush()
    }

    /// packarch: the Wayland drag moves over the X window `target` (`None`
    /// out of X windows); `location`: global logical position, `actions`:
    /// actions of the Wayland source
    pub fn dnd_motion(
        &mut self,
        target: Option<&X11Surface>,
        location: Point<f64, Logical>,
        actions: DndAction,
    ) -> Result<(), ConnectionError> {
        let target = target.map(|t| t.window_id());
        if self.xdnd.out.as_ref().map(|o| o.target) != target {
            self.dnd_leave_target()?;
            if let Some(window) = target {
                self.dnd_enter_target(window)?;
            }
        }
        let scale = self.client_scale.load(Ordering::Acquire);
        let x = (location.x * scale).round() as i16;
        let y = (location.y * scale).round() as i16;
        let action = action_atom(&self.atoms, actions);
        if let Some(out) = self.xdnd.out.as_mut() {
            if out.waiting_status {
                out.pending = Some((x, y, action));
            } else {
                self.dnd_send_position(x, y, action)?;
            }
        }
        self.conn.flush()
    }

    /// packarch: the Wayland drag is dropped: `XdndDrop` if the X window
    /// accepted it (true), `XdndLeave` otherwise (false)
    pub fn dnd_drop(&mut self) -> Result<bool, ConnectionError> {
        let Some(out) = self.xdnd.out.as_mut() else {
            return Ok(false);
        };
        if !out.accepted {
            self.dnd_leave_target()?;
            self.conn.flush()?;
            return Ok(false);
        }
        out.dropped = true;
        let (dest, target) = (out.dest, out.target);
        send_message(
            &self.conn,
            dest,
            target,
            self.atoms.XdndDrop,
            [self.dnd.window, 0, self.dnd.timestamp, 0, 0],
        )?;
        self.conn.flush()?;
        Ok(true)
    }

    fn dnd_enter_target(&mut self, window: X11Window) -> Result<(), ConnectionError> {
        // Not an XDND target: nothing to send
        let Some(version) = property32(&self.conn, window, self.atoms.XdndAware)? else {
            return Ok(());
        };
        if version < 3 {
            return Ok(());
        }
        let version = version.min(XDND_VERSION);
        let dest = property32(&self.conn, window, self.atoms.XdndProxy)?.unwrap_or(window);
        let types = &self.xdnd.types;
        let mut data = [
            self.dnd.window,
            (version << 24) | u32::from(types.len() > 3),
            0,
            0,
            0,
        ];
        for (i, t) in types.iter().take(3).enumerate() {
            data[2 + i] = *t;
        }
        send_message(&self.conn, dest, window, self.atoms.XdndEnter, data)?;
        debug!(window, version, "XDND: enter X window");
        self.xdnd.out = Some(Outgoing {
            target: window,
            dest,
            version,
            waiting_status: false,
            pending: None,
            accepted: false,
            dropped: false,
        });
        Ok(())
    }

    fn dnd_leave_target(&mut self) -> Result<(), ConnectionError> {
        if let Some(out) = self.xdnd.out.take() {
            send_message(
                &self.conn,
                out.dest,
                out.target,
                self.atoms.XdndLeave,
                [self.dnd.window, 0, 0, 0, 0],
            )?;
        }
        Ok(())
    }

    fn dnd_send_position(&mut self, x: i16, y: i16, action: Atom) -> Result<(), ConnectionError> {
        let Some(out) = self.xdnd.out.as_mut() else {
            return Ok(());
        };
        out.waiting_status = true;
        let pos = ((x as u16 as u32) << 16) | (y as u16 as u32);
        let action = if out.version >= 2 { action } else { 0 };
        send_message(
            &self.conn,
            out.dest,
            out.target,
            self.atoms.XdndPosition,
            [self.dnd.window, 0, pos, self.dnd.timestamp, action],
        )
    }

    // --- X11 → Wayland ---------------------------------------------------

    /// packarch: an X client owns `XdndSelection`: it may be dragging
    pub fn x11_drag_active(&self) -> bool {
        self.dnd.owner != x11rb::NONE && self.dnd.owner != self.dnd.window
    }

    /// packarch: shows the proxy window above the X windows (pointer of an X
    /// drag over a Wayland surface) or hides it
    pub fn set_dnd_proxy(&mut self, shown: bool) -> Result<(), ConnectionError> {
        if shown == self.xdnd.proxy_shown {
            return Ok(());
        }
        self.xdnd.proxy_shown = shown;
        if shown {
            self.conn.configure_window(
                self.xdnd.proxy,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?;
            self.conn.map_window(self.xdnd.proxy)?;
        } else {
            self.conn.unmap_window(self.xdnd.proxy)?;
        }
        self.conn.flush()
    }

    /// packarch: whether the Wayland surface under the X drag accepts it,
    /// with the chosen action (`XdndStatus`)
    pub fn x11_drag_status(&mut self, accepted: bool, action: DndAction) -> Result<(), ConnectionError> {
        let Some(inc) = self.xdnd.incoming.as_mut() else {
            return Ok(());
        };
        if inc.accepted == accepted && inc.action == action {
            return Ok(());
        }
        inc.accepted = accepted;
        inc.action = action;
        self.x11_drag_send_status()?;
        self.conn.flush()
    }

    /// packarch: the Wayland surface is done with the dropped X drag
    /// (`XdndFinished`)
    pub fn x11_drag_finished(&mut self, success: bool, action: DndAction) -> Result<(), ConnectionError> {
        let Some(inc) = self.xdnd.incoming.take() else {
            return Ok(());
        };
        let action = if success {
            action_atom(&self.atoms, action)
        } else {
            x11rb::NONE
        };
        send_message(
            &self.conn,
            inc.source,
            inc.source,
            self.atoms.XdndFinished,
            [self.xdnd.proxy, u32::from(success), action, 0, 0],
        )?;
        self.conn.flush()
    }

    /// packarch: writes the data of the X drag in the given mime type to `fd`
    pub fn x11_drag_receive<D>(
        &mut self,
        mime_type: String,
        fd: OwnedFd,
        loop_handle: LoopHandle<'_, D>,
    ) -> Result<(), SelectionError>
    where
        D: XwmHandler + 'static,
    {
        self.send_selection_impl(SelKind::Dnd, mime_type, fd, loop_handle)
    }

    fn x11_drag_send_status(&self) -> Result<(), ConnectionError> {
        let Some(inc) = self.xdnd.incoming.as_ref() else {
            return Ok(());
        };
        let accepted = inc.accepted && !inc.action.is_empty();
        let action = if accepted {
            action_atom(&self.atoms, inc.action)
        } else {
            x11rb::NONE
        };
        // bit 1: positions wanted even without moving out of a rectangle
        send_message(
            &self.conn,
            inc.source,
            inc.source,
            self.atoms.XdndStatus,
            [self.xdnd.proxy, u32::from(accepted) | 2, 0, 0, action],
        )
    }
}

/// XDND client message, to the proxy window (X drag) or to our source
/// window (answers of an X target)
pub(super) fn handle_message<D>(state: &mut D, xwm_id: XwmId, msg: ClientMessageEvent) -> Result<(), ReplyOrIdError>
where
    D: XwmHandler,
{
    let xwm = state.xwm_state(xwm_id);
    let atoms = xwm.atoms;
    let conn = xwm.conn.clone();
    let data = msg.data.as_data32();
    match msg.type_ {
        // Answers of the X window under a Wayland drag
        x if x == atoms.XdndStatus => {
            let Some(out) = xwm.xdnd.out.as_mut() else {
                return Ok(());
            };
            if data[0] != out.target {
                return Ok(());
            }
            out.waiting_status = false;
            out.accepted = data[1] & 1 != 0;
            let mut action = atom_action(&atoms, data[4]);
            if out.accepted && action.is_empty() {
                action = DndAction::Copy;
            }
            let accepted = out.accepted;
            if let Some((x, y, a)) = out.pending.take().filter(|_| !out.dropped) {
                xwm.dnd_send_position(x, y, a)?;
            }
            conn.flush()?;
            state.dnd_target_status(xwm_id, accepted, action);
        }
        x if x == atoms.XdndFinished => {
            let Some(out) = xwm.xdnd.out.take_if(|o| o.target == data[0]) else {
                return Ok(());
            };
            let success = out.version < 5 || data[1] & 1 != 0;
            let action = match atom_action(&atoms, data[2]) {
                a if a.is_empty() => DndAction::Copy,
                a => a,
            };
            debug!(window = out.target, success, "XDND: X window finished");
            state.dnd_target_finished(xwm_id, success, action);
        }
        // X drag over the proxy window
        x if x == atoms.XdndEnter => {
            let source = data[0];
            let types = if data[1] & 1 != 0 {
                atom_list(&conn, source, atoms.XdndTypeList)?
            } else {
                data[2..5].iter().copied().filter(|a| *a != x11rb::NONE).collect()
            };
            let mime_types: Vec<String> = types.into_iter().filter_map(|a| atom_mime(&conn, &atoms, a)).collect();
            let mut actions = atom_list(&conn, source, atoms.XdndActionList)?
                .into_iter()
                .fold(DndAction::empty(), |acc, a| acc | atom_action(&atoms, a));
            if actions.is_empty() {
                actions = DndAction::Copy | DndAction::Move;
            }
            debug!(source, ?mime_types, ?actions, "XDND: X drag entered");
            xwm.xdnd.incoming = Some(IncomingDrag {
                source,
                accepted: false,
                action: DndAction::empty(),
                time: x11rb::CURRENT_TIME,
            });
            state.x11_drag_enter(xwm_id, mime_types, actions);
        }
        x if x == atoms.XdndPosition => {
            if xwm.xdnd.incoming.as_ref().map(|i| i.source) != Some(data[0]) {
                return Ok(());
            }
            let scale = xwm.client_scale.load(Ordering::Acquire);
            let (x, y) = ((data[2] >> 16) as i16, (data[2] & 0xffff) as i16);
            let location = Point::from((x as f64 / scale, y as f64 / scale));
            let action = atom_action(&atoms, data[4]);
            state.x11_drag_motion(xwm_id, location, action);
            // Every position gets an answer
            let xwm = state.xwm_state(xwm_id);
            xwm.x11_drag_send_status()?;
            conn.flush()?;
        }
        x if x == atoms.XdndLeave => {
            if xwm.xdnd.incoming.take_if(|i| i.source == data[0]).is_some() {
                state.x11_drag_leave(xwm_id);
            }
        }
        x if x == atoms.XdndDrop => {
            let Some(inc) = xwm.xdnd.incoming.as_mut().filter(|i| i.source == data[0]) else {
                return Ok(());
            };
            inc.time = data[2];
            state.x11_drag_drop(xwm_id);
        }
        _ => {}
    }
    Ok(())
}
