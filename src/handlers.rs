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
use smithay::utils::Transform;
use smithay::utils::{Point, Serial};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    CompositorClientState, CompositorHandler, CompositorState, SurfaceData, get_parent, is_sync_subsurface,
    send_surface_state, with_states,
};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier};
use smithay::wayland::fractional_scale::{FractionalScaleHandler, with_fractional_scale};
use smithay::wayland::output::OutputHandler;
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
use crate::input::MoveGrab;
use crate::state::{ClientState, State};
use crate::view::{Axis, Tiling, View};
use crate::wm::Origin;

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
        // Le client Xwayland porte les données de Smithay
        if let Some(x) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &x.compositor_state;
        }
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
        // Première configuration xdg (les fenêtres X11 n'en ont pas)
        if let Some(toplevel) = self.views[i].window.toplevel().cloned() {
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
        }
        let has_buffer = with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
        let view = &mut self.views[i];
        if has_buffer && !view.mapped {
            self.map_view(i);
            return;
        }
        if !has_buffer && view.mapped {
            self.ghost_view(i, crate::render::GhostKind::Close);
            self.views[i].mapped = false;
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
            // Fini quand le client a appliqué la dernière configuration
            if let Some(last) = r.ending {
                let acked = with_states(surface, |s| {
                    s.data_map
                        .get::<XdgToplevelSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap()
                        .current_serial
                });
                if last.is_none_or(|l| acked.is_some_and(|a| a >= l)) {
                    view.resize = None;
                }
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
        self.check_zone(&output);
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

    /// Première apparition d'une vue: placement, focus, animation, règles
    pub fn map_view(&mut self, i: usize) {
        self.views[i].mapped = true;
        if let Some(output) = self.outputs.first().cloned() {
            use smithay::desktop::space::SpaceElement;
            self.views[i]
                .window
                .output_enter(&output, self.output_geometry(&output));
        }
        // Une fenêtre X11 qui demande une position visible la garde (§ 8.2)
        let x11_pos = self.views[i].x11().map(|x| x.geometry().loc).filter(|p| {
            *p != Point::from((0, 0)) && self.outputs.iter().any(|o| self.output_geometry(o).contains(*p))
        });
        match (x11_pos, self.parent_view(i)) {
            (Some(p), _) => self.views[i].loc = p,
            (None, Some(parent)) => self.center_on(i, parent),
            (None, None) => self.place_new(i),
        }
        if self.views[i].x11().is_some() {
            self.configure(i, None);
        }
        let i = self.focus_view(i, true);
        self.appear(i, crate::render::Appear::Open);
        info!(
            "fenêtre: « {} » ({})",
            self.views[i].title(),
            self.views[i].app_id()
        );
        self.apply_rules(i);
    }

    /// Vue parente d'un dialogue (xdg parent, WM_TRANSIENT_FOR)
    pub fn parent_view(&self, i: usize) -> Option<usize> {
        if let Some(t) = self.views[i].window.toplevel() {
            return t.parent().and_then(|p| self.view_index(&p));
        }
        let parent = self.views[i].x11()?.is_transient_for()?;
        self.views
            .iter()
            .position(|v| v.x11().is_some_and(|x| x.window_id() == parent))
    }

    /// Centre une vue sur une autre, dans la zone utile (§ 8.1)
    fn center_on(&mut self, i: usize, parent: usize) {
        self.views[i].desktop = self.views[parent].desktop;
        let p = self.frame_rect(parent);
        let f = self.frame_rect(i);
        let zone = self.view_zone(parent);
        let x = (p.loc.x + (p.size.w - f.size.w) / 2)
            .clamp(zone.loc.x, zone.loc.x + (zone.size.w - f.size.w).max(0));
        let y = (p.loc.y + (p.size.h - f.size.h) / 2)
            .clamp(zone.loc.y, zone.loc.y + (zone.size.h - f.size.h).max(0));
        let layout = self.layout(&self.views[i]);
        self.views[i].loc = Point::from((x, y)) + layout.client.loc;
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
    type KeyboardFocus = crate::state::KbFocus;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<State> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor_status = image;
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&crate::state::KbFocus>) {
        let client = focused.and_then(|f| self.dh.get_client(f.surface.id()).ok());
        set_data_device_focus(&self.dh, seat, client.clone());
        set_primary_focus(&self.dh, seat, client);
    }
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

    /// Le cadre du compositeur n'est dessiné que si le client le demande
    /// (xdg-decoration, ou décoration KDE des applications GTK3) : sans
    /// l'un ni l'autre, il dessine le sien (GTK4, libadwaita…)
    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let mut view = View::new(
            Window::new_wayland_window(surface.clone()),
            surface.wl_surface().clone(),
            self.desktop,
        );
        self.kde_ssd.retain(|s| s.is_alive());
        view.decorated = self.kde_ssd.contains(surface.wl_surface());
        self.views.push(view);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(i) = self.view_of(&surface) {
            if self.views[i].mapped {
                self.ghost_view(i, crate::render::GhostKind::Close);
            }
            let view = self.views.remove(i);
            self.mru.retain(|w| *w != view.window);
            if let Some(c) = self.cycle.as_mut() {
                c.windows.retain(|w| *w != view.window);
                if c.windows.is_empty() {
                    self.cycle = None;
                } else {
                    c.index = c.index.min(c.windows.len() - 1);
                }
            }
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
        let Ok(mut grab) = self
            .popups
            .grab_popup(crate::state::KbFocus::from(root), kind, &seat, serial)
        else {
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
        let Some(pointer) = seat.get_pointer() else { return };
        let grab = MoveGrab {
            start,
            window: self.views[i].window.clone(),
            initial_loc: self.views[i].loc,
            keyboard: false,
            snap: None,
        };
        pointer.set_grab(self, grab, serial, Focus::Clear);
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
        let origin = Origin {
            button: Some(start.button),
            location: start.location,
        };
        self.start_resize_edges(i, e, origin);
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(i) = self.view_of(&surface) {
            self.maximize(i, Axis::Both);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(i) = self.view_of(&surface)
            && matches!(self.views[i].tiling, Tiling::Maximized(_))
        {
            self.restore(i);
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<wl_output::WlOutput>) {
        match self.view_of(&surface) {
            Some(i) if self.views[i].tiling != Tiling::Fullscreen => self.toggle_fullscreen(i),
            _ => {
                surface.send_pending_configure();
            }
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(i) = self.view_of(&surface)
            && self.views[i].tiling == Tiling::Fullscreen
        {
            self.toggle_fullscreen(i);
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(i) = self.view_of(&surface) {
            self.iconify(i);
        }
    }

    fn title_changed(&mut self, _surface: ToplevelSurface) {}
}

delegate_xdg_shell!(State);

// --- Décorations ----------------------------------------------------------

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        let mode = if self.config.server_decoration {
            DecoMode::ServerSide
        } else {
            DecoMode::ClientSide
        };
        if let Some(i) = self.view_of(&toplevel) {
            self.views[i].decorated = mode == DecoMode::ServerSide;
        }
        toplevel.with_pending_state(|s| s.decoration_mode = Some(mode));
    }

    /// `core/decoration` à `server`: le cadre du compositeur, toujours;
    /// à `client`: celui que le client demande
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecoMode) {
        let mode = if self.config.server_decoration {
            DecoMode::ServerSide
        } else {
            mode
        };
        if let Some(i) = self.view_of(&toplevel) {
            self.views[i].decorated = mode == DecoMode::ServerSide;
        }
        toplevel.with_pending_state(|s| s.decoration_mode = Some(mode));
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

    /// GTK3: décoration par le serveur (mode par défaut), souvent annoncée
    /// avant le rôle de fenêtre
    fn new_decoration(
        &mut self,
        surface: &WlSurface,
        _decoration: &smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration,
    ) {
        self.set_kde_ssd(surface, true);
    }

    fn request_mode(
        &mut self,
        surface: &WlSurface,
        decoration: &smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::OrgKdeKwinServerDecoration,
        mode: smithay::reexports::wayland_server::WEnum<
            smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::Mode,
        >,
    ) {
        use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::Mode;
        if let smithay::reexports::wayland_server::WEnum::Value(m) = mode {
            let server = m == Mode::Server || self.config.server_decoration;
            let m = if server { Mode::Server } else { m };
            self.set_kde_ssd(surface, server);
            decoration.mode(m);
        }
    }
}

impl State {
    fn set_kde_ssd(&mut self, surface: &WlSurface, server: bool) {
        self.kde_ssd.retain(|s| s.is_alive() && s != surface);
        if server {
            self.kde_ssd.push(surface.clone());
        }
        if let Some(i) = self.view_index(surface) {
            self.views[i].decorated = server;
        }
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
        for output in self.outputs.clone() {
            layer_map_for_output(&output).arrange();
            self.check_zone(&output);
        }
        if was_focused {
            self.focus_top();
        }
    }
}

delegate_layer_shell!(State);
