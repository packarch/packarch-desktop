//! packarch: drag'n'drop offered to Wayland clients for a foreign source
//! (an X client through XWayland). No pointer grab: the pointer stays with
//! the X client that drags, the compositor moves the drag with
//! [`ForeignDnd::motion`] and ends it with [`ForeignDnd::drop`] or
//! [`ForeignDnd::leave`].

use std::{
    cell::RefCell,
    fmt,
    os::unix::io::OwnedFd,
    sync::{Arc, Mutex},
};

use wayland_server::{
    backend::{protocol::Message, ClientId, Handle, ObjectData, ObjectId},
    protocol::{
        wl_data_device_manager::DndAction,
        wl_data_offer::{self, WlDataOffer},
        wl_surface::WlSurface,
    },
    DisplayHandle, Resource,
};

use crate::{
    input::{Seat, SeatHandler},
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::selection::seat_data::SeatData,
};

use super::DataDeviceHandler;

/// Answers of the Wayland client under a foreign drag
pub trait ForeignDndHandler: DataDeviceHandler {
    /// The surface under the drag accepts it (a mime type and an action) or not
    fn foreign_dnd_status(&mut self, accepted: bool, action: DndAction) {
        let _ = (accepted, action);
    }

    /// The surface reads the data in the given mime type
    fn foreign_dnd_receive(&mut self, mime_type: String, fd: OwnedFd);

    /// The surface is done with the drop (`success`: data read)
    fn foreign_dnd_finished(&mut self, success: bool, action: DndAction) {
        let _ = (success, action);
    }
}

#[derive(Debug)]
struct OfferState {
    mime_types: Vec<String>,
    actions: DndAction,
    active: bool,
    accepted: bool,
    dropped: bool,
    finished: bool,
    action: DndAction,
}

/// Drag'n'drop of a foreign source over Wayland surfaces
pub struct ForeignDnd<D: SeatHandler> {
    dh: DisplayHandle,
    seat: Seat<D>,
    mime_types: Vec<String>,
    actions: DndAction,
    focus: Option<WlSurface>,
    offer: Option<Arc<Mutex<OfferState>>>,
}

impl<D: SeatHandler> fmt::Debug for ForeignDnd<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ForeignDnd")
            .field("mime_types", &self.mime_types)
            .field("actions", &self.actions)
            .field("focus", &self.focus)
            .field("offer", &self.offer)
            .finish()
    }
}

impl<D> ForeignDnd<D>
where
    D: ForeignDndHandler + 'static,
{
    /// New drag offering the given mime types and actions
    pub fn new(dh: &DisplayHandle, seat: &Seat<D>, mime_types: Vec<String>, actions: DndAction) -> Self {
        seat.user_data()
            .insert_if_missing(|| RefCell::new(SeatData::<D::SelectionUserData>::new()));
        ForeignDnd {
            dh: dh.clone(),
            seat: seat.clone(),
            mime_types,
            actions,
            focus: None,
            offer: None,
        }
    }

    /// Surface under the drag
    pub fn focus(&self) -> Option<&WlSurface> {
        self.focus.as_ref()
    }

    fn for_devices(&self, surface: &WlSurface, mut f: impl FnMut(&wayland_server::protocol::wl_data_device::WlDataDevice)) {
        let Some(seat_data) = self
            .seat
            .user_data()
            .get::<RefCell<SeatData<D::SelectionUserData>>>()
        else {
            return;
        };
        for device in seat_data
            .borrow()
            .known_data_devices()
            .filter(|d| d.id().same_client_as(&surface.id()))
        {
            f(device);
        }
    }

    /// The drag moves over `target` (surface-local position), or out of any
    /// Wayland surface (`None`)
    pub fn motion(&mut self, target: Option<(&WlSurface, Point<f64, Logical>)>, time: u32) {
        if target.map(|(s, _)| s) != self.focus.as_ref() {
            self.leave();
        }
        let Some((surface, loc)) = target else {
            return;
        };
        if self.focus.is_some() {
            self.for_devices(surface, |d| d.motion(time, loc.x, loc.y));
            return;
        }
        let Ok(client) = self.dh.get_client(surface.id()) else {
            return;
        };
        let state = Arc::new(Mutex::new(OfferState {
            mime_types: self.mime_types.clone(),
            actions: self.actions,
            active: true,
            accepted: false,
            dropped: false,
            finished: false,
            action: DndAction::empty(),
        }));
        let handle = self.dh.backend_handle();
        let serial = SERIAL_COUNTER.next_serial();
        self.for_devices(surface, |device| {
            let Ok(offer) = handle.create_object::<D>(
                client.id(),
                WlDataOffer::interface(),
                device.version(),
                Arc::new(ForeignOffer { state: state.clone() }),
            ) else {
                return;
            };
            let Ok(offer) = WlDataOffer::from_id(&self.dh, offer) else {
                return;
            };
            device.data_offer(&offer);
            for mime_type in &self.mime_types {
                offer.offer(mime_type.clone());
            }
            if offer.version() >= wl_data_offer::EVT_SOURCE_ACTIONS_SINCE {
                offer.source_actions(self.actions);
            }
            device.enter(serial.into(), surface, loc.x, loc.y, Some(&offer));
        });
        self.offer = Some(state);
        self.focus = Some(surface.clone());
    }

    /// The drag left the Wayland surfaces or was cancelled
    pub fn leave(&mut self) {
        if let Some(surface) = self.focus.take() {
            self.for_devices(&surface, |d| d.leave());
        }
        if let Some(state) = self.offer.take() {
            state.lock().unwrap().active = false;
        }
    }

    /// The drag is dropped: true when the surface accepted it (it will read
    /// the data, then finish)
    pub fn drop(&mut self) -> bool {
        let validated = self.offer.as_ref().is_some_and(|s| {
            let mut s = s.lock().unwrap();
            let validated = s.active && s.accepted && !s.action.is_empty();
            if validated {
                s.dropped = true;
            } else {
                s.active = false;
            }
            validated
        });
        if let Some(surface) = self.focus.take() {
            self.for_devices(&surface, |d| {
                if validated {
                    d.drop();
                }
                d.leave();
            });
        }
        self.offer = None;
        validated
    }
}

#[derive(Debug)]
struct ForeignOffer {
    state: Arc<Mutex<OfferState>>,
}

impl<D> ObjectData<D> for ForeignOffer
where
    D: ForeignDndHandler + 'static,
{
    fn request(
        self: Arc<Self>,
        dh: &Handle,
        handler: &mut D,
        _client_id: ClientId,
        msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData<D>>> {
        let dh = DisplayHandle::from(dh.clone());
        if let Ok((resource, request)) = WlDataOffer::parse_request(&dh, msg) {
            handle_request(handler, &resource, request, &self.state);
        }
        None
    }

    fn destroyed(self: Arc<Self>, _handle: &Handle, _data: &mut D, _client_id: ClientId, _object_id: ObjectId) {}
}

fn handle_request<D>(
    handler: &mut D,
    offer: &WlDataOffer,
    request: wl_data_offer::Request,
    state: &Mutex<OfferState>,
) where
    D: ForeignDndHandler,
{
    use wl_data_offer::Request;
    let mut st = state.lock().unwrap();
    match request {
        Request::Accept { mime_type, .. } => {
            st.accepted = mime_type.is_some_and(|m| st.mime_types.contains(&m));
            if st.active && !st.dropped {
                let (accepted, action) = (st.accepted && !st.action.is_empty(), st.action);
                drop(st);
                handler.foreign_dnd_status(accepted, action);
            }
        }
        Request::Receive { mime_type, fd } => {
            if st.active && st.mime_types.contains(&mime_type) {
                drop(st);
                handler.foreign_dnd_receive(mime_type, fd);
            }
        }
        Request::Destroy => {
            if st.dropped && !st.finished {
                st.finished = true;
                drop(st);
                handler.foreign_dnd_finished(false, DndAction::empty());
            }
        }
        Request::Finish => {
            if !st.active || !st.accepted || !st.dropped || st.action.is_empty() {
                offer.post_error(
                    wl_data_offer::Error::InvalidFinish,
                    "Cannot finish a data offer that is not dropped, accepted, with an action.",
                );
                return;
            }
            st.active = false;
            st.finished = true;
            let action = st.action;
            drop(st);
            handler.foreign_dnd_finished(true, action);
        }
        Request::SetActions {
            dnd_actions,
            preferred_action,
        } => {
            let dnd_actions = dnd_actions.into_result().unwrap_or(DndAction::None);
            let preferred_action = preferred_action.into_result().unwrap_or(DndAction::None);
            if ![DndAction::None, DndAction::Move, DndAction::Copy, DndAction::Ask].contains(&preferred_action) {
                offer.post_error(wl_data_offer::Error::InvalidAction, "Invalid preferred action.");
                return;
            }
            let possible = st.actions & dnd_actions;
            let (active, dropped) = (st.active, st.dropped);
            drop(st);
            let chosen = handler.action_choice(possible, preferred_action);
            let mut st = state.lock().unwrap();
            if chosen != st.action {
                st.action = chosen;
                offer.action(chosen);
                if active && !dropped {
                    let accepted = st.accepted && !chosen.is_empty();
                    drop(st);
                    handler.foreign_dnd_status(accepted, chosen);
                }
            }
        }
        _ => {}
    }
}
