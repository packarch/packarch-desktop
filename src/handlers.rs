// SPDX-License-Identifier: MIT OR Apache-2.0
//! Protocoles Wayland du prototype (cahier des charges § 9.1)

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::utils::{on_commit_buffer_handler, with_renderer_surface_state};
use smithay::desktop::{
    LayerSurface, PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy, Window,
    find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output,
};
use smithay::input::pointer::{CursorImageStatus, Focus};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecoMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_output, wl_seat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, Resource};
use smithay::utils::Serial;
use smithay::utils::Transform;
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, SurfaceData, get_parent, is_sync_subsurface,
    send_surface_state, with_states,
};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::fractional_scale::{FractionalScaleHandler, with_fractional_scale};
use smithay::wayland::output::OutputHandler;
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::selection::data_device::{
    ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler, set_data_device_focus,
};
use smithay::wayland::selection::primary_selection::{
    PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
};
use smithay::wayland::shell::kde::decoration::{KdeDecorationHandler, KdeDecorationState};
use smithay::wayland::shell::wlr_layer::{
    KeyboardInteractivity, Layer, LayerSurface as WlrLayerSurface, LayerSurfaceData, WlrLayerShellHandler,
    WlrLayerShellState,
};
use smithay::wayland::shell::xdg::decoration::XdgDecorationHandler;
use smithay::wayland::shell::xdg::{
    PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState, XdgToplevelSurfaceData,
};
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::{
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_fractional_scale,
    delegate_kde_decoration, delegate_layer_shell, delegate_output, delegate_primary_selection,
    delegate_seat, delegate_shm, delegate_viewporter, delegate_xdg_decoration, delegate_xdg_shell,
};
use tracing::{debug, info};

use crate::deco::Edges;
use crate::input::{MoveGrab, ResizeGrab};
use crate::state::{ClientState, State};
use crate::view::{Resize, View};

// --- Surfaces -------------------------------------------------------------

fn surface_scale(state: &State, surface: &WlSurface, data: &SurfaceData) {
    let scale = state
        .outputs
        .first()
        .map(|o| o.current_scale().fractional_scale())
        .unwrap_or(1.0);
    with_fractional_scale(data, |fs| fs.set_preferred_scale(scale));
    send_surface_state(surface, data, scale.ceil() as i32, Transform::Normal);
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn new_surface(&mut self, surface: &WlSurface) {
        with_states(surface, |data| surface_scale(self, surface, data));
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            if let Some(i) = self.view_index(&root) {
                self.views[i].window.on_commit();
            }
        }
        self.commit_toplevel(surface);
        self.commit_popup(surface);
        self.commit_layer(surface);
    }
}

impl State {
    fn commit_toplevel(&mut self, surface: &WlSurface) {
        let Some(i) = self.view_index(surface) else { return };
        let toplevel = self.views[i].window.toplevel().unwrap().clone();
        let initial_sent = with_states(surface, |s| {
            s.data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if !initial_sent {
            toplevel.with_pending_state(|s| s.decoration_mode = Some(DecoMode::ServerSide));
            toplevel.send_configure();
            return;
        }
        let has_buffer = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
        let view = &mut self.views[i];
        if has_buffer && !view.mapped {
            view.mapped = true;
            if let Some(output) = self.outputs.first().cloned() {
                use smithay::desktop::space::SpaceElement;
                self.views[i]
                    .window
                    .output_enter(&output, self.output_geometry(&output));
            }
            self.place(i);
            self.focus_view(i);
            let i = self.view_index(surface).unwrap();
            info!("fenêtre: « {} »", self.views[i].title());
            return;
        }
        if !has_buffer && view.mapped {
            view.mapped = false;
            self.focus_top();
            return;
        }
        // Redimensionnement par la gauche ou le haut: le bord opposé reste fixe
        let view = &mut self.views[i];
        if let Some(r) = view.resize {
            let size = view.window.geometry().size;
            if r.edges.left {
                view.loc.x = r.initial_loc.x + (r.initial_size.w - size.w);
            }
            if r.edges.top {
                view.loc.y = r.initial_loc.y + (r.initial_size.h - size.h);
            }
            if r.ending {
                view.resize = None;
            }
        }
    }

    fn commit_popup(&mut self, surface: &WlSurface) {
        self.popups.commit(surface);
        if let Some(PopupKind::Xdg(xdg)) = self.popups.find_popup(surface)
            && !xdg.is_initial_configure_sent()
        {
            let _ = xdg.send_configure();
        }
    }

    fn commit_layer(&mut self, surface: &WlSurface) {
        let Some(output) = self
            .outputs
            .iter()
            .find(|o| {
                layer_map_for_output(o)
                    .layer_for_surface(surface, smithay::desktop::WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned()
        else {
            return;
        };
        let initial_sent = with_states(surface, |s| {
            s.data_map
                .get::<LayerSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        let mut map = layer_map_for_output(&output);
        map.arrange();
        let layer = map
            .layer_for_surface(surface, smithay::desktop::WindowSurfaceType::TOPLEVEL)
            .cloned();
        drop(map);
        let Some(layer) = layer else { return };
        if !initial_sent {
            layer.layer_surface().send_configure();
            return;
        }
        // Clavier exclusif (lanceurs comme rofi): il prend le focus
        let exclusive = layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive;
        let mapped = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
        if exclusive && mapped && self.focused_surface().as_ref() != Some(surface) {
            self.set_keyboard_focus(Some(surface.clone()));
        }
    }

    fn view_of(&self, surface: &ToplevelSurface) -> Option<usize> {
        self.view_index(surface.wl_surface())
    }

    /// Place une fenêtre surgissante dans l'écran (§ 9.1, xdg_positioner)
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let parent_loc = if let Some(i) = self.view_index(&root) {
            self.views[i].loc
        } else if let Some((output, geo)) = self.outputs.iter().find_map(|o| {
            let map = layer_map_for_output(o);
            let layer = map.layer_for_surface(&root, smithay::desktop::WindowSurfaceType::TOPLEVEL)?;
            Some((o.clone(), map.layer_geometry(layer)?))
        }) {
            geo.loc + output.current_location()
        } else {
            return;
        };
        let Some(output) = self.output_at(parent_loc.to_f64()) else {
            return;
        };
        let mut target = self.output_geometry(&output);
        target.loc -= get_popup_toplevel_coords(&kind);
        target.loc -= parent_loc;
        popup.with_pending_state(|s| s.geometry = s.positioner.get_unconstrained_geometry(target));
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for State {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf.as_mut().unwrap().0
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        let ok = self
            .backend
            .as_mut()
            .is_some_and(|b| b.renderer().import_dmabuf(&dmabuf, None).is_ok());
        if ok {
            let _ = notifier.successful::<State>();
        } else {
            notifier.failed();
        }
    }
}

impl FractionalScaleHandler for State {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        with_states(&surface, |data| surface_scale(self, &surface, data));
    }
}

delegate_compositor!(State);
delegate_shm!(State);
delegate_dmabuf!(State);
delegate_viewporter!(State);
delegate_fractional_scale!(State);

// --- Siège, presse-papiers, écrans ----------------------------------------

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| self.dh.get_client(s.id()).ok());
        set_data_device_focus(&self.dh, seat, client.clone());
        set_primary_focus(&self.dh, seat, client);
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

impl PrimarySelectionHandler for State {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl OutputHandler for State {}

delegate_seat!(State);
delegate_data_device!(State);
delegate_primary_selection!(State);
delegate_output!(State);

// --- xdg-shell -----------------------------------------------------------

fn check_grab(
    seat: &Seat<State>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<smithay::input::pointer::GrabStartData<State>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start = pointer.grab_start_data()?;
    let (focus, _) = start.focus.as_ref()?;
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }
    Some(start)
}

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.views.push(View::new(Window::new_wayland_window(surface)));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(i) = self.view_of(&surface) {
            self.views.remove(i);
            self.focus_top();
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|s| {
            s.geometry = positioner.get_geometry();
            s.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let Ok(mut grab) = self.popups.grab_popup(root, kind, &seat, serial) else {
            return;
        };
        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial) || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            return;
        };
        let Some(start) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(i) = self.view_of(&surface) else { return };
        self.start_move(i, start, serial);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        use xdg_toplevel::ResizeEdge as E;
        let Some(seat) = Seat::<State>::from_resource(&seat) else {
            return;
        };
        let Some(start) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(i) = self.view_of(&surface) else { return };
        let e = Edges {
            left: matches!(edges, E::Left | E::TopLeft | E::BottomLeft),
            right: matches!(edges, E::Right | E::TopRight | E::BottomRight),
            top: matches!(edges, E::Top | E::TopLeft | E::TopRight),
            bottom: matches!(edges, E::Bottom | E::BottomLeft | E::BottomRight),
        };
        self.start_resize(i, e, start, serial);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(i) = self.view_of(&surface)
            && self.views[i].maximized.is_none()
        {
            self.toggle_maximize(i);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(i) = self.view_of(&surface)
            && self.views[i].maximized.is_some()
        {
            self.toggle_maximize(i);
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<wl_output::WlOutput>) {
        // Plein écran: étape 0.2; on répond par la configuration courante
        surface.send_pending_configure();
    }

    fn minimize_request(&mut self, _surface: ToplevelSurface) {
        debug!("réduction demandée: prévue avec la barre des tâches (0.4)");
    }

    fn title_changed(&mut self, _surface: ToplevelSurface) {}
}

impl State {
    pub fn start_move(
        &mut self,
        i: usize,
        start: smithay::input::pointer::GrabStartData<State>,
        serial: Serial,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let view = &self.views[i];
        let grab = MoveGrab {
            start,
            window: view.window.clone(),
            initial_loc: view.loc,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }

    pub fn start_resize(
        &mut self,
        i: usize,
        edges: Edges,
        start: smithay::input::pointer::GrabStartData<State>,
        serial: Serial,
    ) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let view = &mut self.views[i];
        let size = view.window.geometry().size;
        view.resize = Some(Resize {
            edges,
            initial_loc: view.loc,
            initial_size: size,
            ending: false,
        });
        let toplevel = view.window.toplevel().unwrap().clone();
        toplevel.with_pending_state(|s| s.states.set(xdg_toplevel::State::Resizing));
        toplevel.send_pending_configure();
        let grab = ResizeGrab {
            start,
            window: view.window.clone(),
            edges,
            initial_size: size,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
    }
}

delegate_xdg_shell!(State);

// --- Décorations: toujours dessinées par le compositeur -------------------

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecoMode::ServerSide));
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecoMode) {
        toplevel.with_pending_state(|s| s.decoration_mode = Some(DecoMode::ServerSide));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        XdgDecorationHandler::request_mode(self, toplevel, DecoMode::ServerSide);
    }
}

impl KdeDecorationHandler for State {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.kde_decoration_state
    }
}

delegate_xdg_decoration!(State);
delegate_kde_decoration!(State);

// --- layer-shell ----------------------------------------------------------

impl WlrLayerShellHandler for State {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: WlrLayerSurface,
        output: Option<wl_output::WlOutput>,
        _layer: Layer,
        namespace: String,
    ) {
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.output_at(self.pointer));
        let Some(output) = output else {
            surface.send_close();
            return;
        };
        debug!("couche « {namespace} »");
        let mut map = layer_map_for_output(&output);
        let _ = map.map_layer(&LayerSurface::new(surface, namespace));
    }

    fn new_popup(&mut self, _parent: WlrLayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let was_focused = self.focused_surface().as_ref() == Some(surface.wl_surface());
        for output in &self.outputs {
            let mut map = layer_map_for_output(output);
            let layer = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(layer) = layer {
                map.unmap_layer(&layer);
            }
        }
        if was_focused {
            self.focus_top();
        }
    }
}

delegate_layer_shell!(State);
