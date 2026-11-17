// SPDX-License-Identifier: MIT OR Apache-2.0
//! État global du compositeur

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Instant;

use smithay::desktop::{PopupManager, Window, WindowSurfaceType, layer_map_for_output};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, LoopSignal, Mode, PostAction};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER};
use smithay::wayland::compositor::{CompositorClientState, CompositorState};
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::shell::kde::decoration::KdeDecorationState;
use smithay::wayland::shell::wlr_layer::{Layer, WlrLayerShellState};
use smithay::wayland::shell::xdg::XdgShellState;
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shm::ShmState;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::viewporter::ViewporterState;
use tracing::{info, warn};

use crate::config::Config;
use crate::deco::{Layout, Part, Text};
use crate::render::Shaders;
use crate::theme::Theme;
use crate::view::View;

/// Ce qui se trouve sous le pointeur
pub enum Under {
    /// Surface d'un client, et position de son origine (globale)
    Surface(WlSurface, Point<f64, Logical>, Option<usize>),
    /// Partie du cadre d'une vue
    Frame(usize, Part),
    /// Fond d'écran sans surface
    Root,
}

/// Les états des protocoles doivent vivre aussi longtemps que leurs
/// globales, même quand le code ne les relit pas
#[allow(dead_code)]
pub struct State {
    pub dh: DisplayHandle,
    pub handle: LoopHandle<'static, State>,
    pub signal: LoopSignal,
    pub socket_name: OsString,
    pub start: Instant,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub kde_decoration_state: KdeDecorationState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub dmabuf: Option<(DmabufState, DmabufGlobal)>,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub viewporter_state: ViewporterState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub popups: PopupManager,
    pub seat: Seat<State>,

    /// Vues, de l'arrière vers l'avant
    pub views: Vec<View>,
    pub outputs: Vec<Output>,
    pub config: Config,
    pub theme: Theme,
    pub text: Text,
    pub shaders: Option<Shaders>,
    /// Backend (sorti de l'état pendant le rendu)
    pub backend: Option<crate::backend::Backend>,
    pub pointer: Point<f64, Logical>,
    /// Dernier clic sur une barre de titre (vue, instant), pour le double clic
    pub last_title_click: Option<(Window, Instant)>,
    /// Commandes de la session: variables d'environnement à leur passer
    pub child_env: Vec<(String, String)>,
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl State {
    pub fn new(
        display: Display<State>,
        handle: LoopHandle<'static, State>,
        signal: LoopSignal,
        config: Config,
    ) -> State {
        let dh = display.handle();
        let compositor_state = CompositorState::new::<State>(&dh);
        let xdg_shell_state = XdgShellState::new::<State>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<State>(&dh);
        let kde_decoration_state = KdeDecorationState::new::<State>(
            &dh,
            smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode::Server,
        );
        let layer_shell_state = WlrLayerShellState::new::<State>(&dh);
        let shm_state = ShmState::new::<State>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<State>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<State>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<State>(&dh);
        let viewporter_state = ViewporterState::new::<State>(&dh);
        let fractional_scale_state = FractionalScaleManagerState::new::<State>(&dh);

        let mut seat: Seat<State> = seat_state.new_wl_seat(&dh, "seat0");
        if let Err(e) = seat.add_keyboard(Default::default(), 600, 25) {
            warn!("clavier: {e:?}");
        }
        seat.add_pointer();

        let listening = ListeningSocketSource::new_auto().expect("socket Wayland");
        let socket_name = listening.socket_name().to_os_string();
        handle
            .insert_source(listening, |stream, _, state| {
                if let Err(e) = state.dh.insert_client(stream, Arc::new(ClientState::default())) {
                    warn!("nouveau client: {e}");
                }
            })
            .expect("source du socket");
        handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Sûr: l'affichage n'est jamais libéré pendant la boucle
                    unsafe {
                        display.get_mut().dispatch_clients(state).unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .expect("source de l'affichage");
        info!("socket Wayland: {}", socket_name.to_string_lossy());

        let theme = Theme::load(&config);
        let child_env = vec![
            (
                "WAYLAND_DISPLAY".into(),
                socket_name.to_string_lossy().to_string(),
            ),
            ("PACKARCH_PID".into(), std::process::id().to_string()),
        ];
        State {
            dh,
            handle,
            signal,
            socket_name,
            start: Instant::now(),
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            kde_decoration_state,
            layer_shell_state,
            shm_state,
            dmabuf: None,
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            viewporter_state,
            fractional_scale_state,
            popups: PopupManager::default(),
            seat,
            views: vec![],
            outputs: vec![],
            config,
            theme,
            text: Text::new(),
            shaders: None,
            backend: None,
            pointer: Point::from((0.0, 0.0)),
            last_title_click: None,
            child_env,
        }
    }

    /// Lance `sh -c commande`, détachée, avec l'environnement de la session
    pub fn spawn(&self, command: &str) {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(command).stdin(std::process::Stdio::null());
        cmd.env_remove("WAYLAND_SOCKET");
        for (k, v) in &self.child_env {
            cmd.env(k, v);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(e) => warn!("lancement de « {command} »: {e}"),
        }
    }

    pub fn output_location(&self, output: &Output) -> Point<i32, Logical> {
        output.current_location()
    }

    pub fn output_geometry(&self, output: &Output) -> Rectangle<i32, Logical> {
        let size = output
            .current_mode()
            .map(|m| {
                m.size
                    .to_f64()
                    .to_logical(output.current_scale().fractional_scale())
                    .to_i32_round()
            })
            .unwrap_or_default();
        Rectangle::new(output.current_location(), size)
    }

    /// Écran sous un point, sinon le premier
    pub fn output_at(&self, p: Point<f64, Logical>) -> Option<Output> {
        self.outputs
            .iter()
            .find(|o| self.output_geometry(o).to_f64().contains(p))
            .or(self.outputs.first())
            .cloned()
    }

    /// Zone utile d'un écran: sans les panneaux (zones exclusives)
    pub fn usable_area(&self, output: &Output) -> Rectangle<i32, Logical> {
        let mut zone = layer_map_for_output(output).non_exclusive_zone();
        zone.loc += output.current_location();
        zone
    }

    pub fn focused_surface(&self) -> Option<WlSurface> {
        self.seat.get_keyboard().and_then(|k| k.current_focus())
    }

    /// Surface de premier niveau qui a le clavier: celle d'une fenêtre
    /// surgissante (menu) est rapportée à sa fenêtre
    pub fn focused_root(&self) -> Option<WlSurface> {
        let surface = self.focused_surface()?;
        match self.popups.find_popup(&surface) {
            Some(popup) => smithay::desktop::find_popup_root_surface(&popup).ok(),
            None => Some(surface),
        }
    }

    pub fn view_index(&self, surface: &WlSurface) -> Option<usize> {
        self.views.iter().position(|v| v.surface() == surface)
    }

    pub fn layout(&self, view: &View) -> Layout {
        Layout::new(&self.config, &self.theme, view.window.geometry().size)
    }

    /// Ce qui se trouve au point `p` (coordonnées globales)
    pub fn under(&self, p: Point<f64, Logical>) -> Under {
        let output = self.output_at(p);
        let layer_hit = |layers: &[Layer]| -> Option<Under> {
            let output = output.as_ref()?;
            let map = layer_map_for_output(output);
            let local = p - output.current_location().to_f64();
            for &l in layers {
                if let Some(layer) = map.layer_under(l, local) {
                    let geo = map.layer_geometry(layer)?;
                    if let Some((s, loc)) =
                        layer.surface_under(local - geo.loc.to_f64(), WindowSurfaceType::ALL)
                    {
                        let origin = (loc + geo.loc + output.current_location()).to_f64();
                        return Some(Under::Surface(s, origin, None));
                    }
                }
            }
            None
        };
        if let Some(u) = layer_hit(&[Layer::Overlay, Layer::Top]) {
            return u;
        }
        for (i, view) in self.views.iter().enumerate().rev() {
            if !view.mapped || view.minimized {
                continue;
            }
            let render_loc = view.loc - view.window.geometry().loc;
            if let Some((s, loc)) = view
                .window
                .surface_under(p - render_loc.to_f64(), WindowSurfaceType::ALL)
            {
                return Under::Surface(s, (loc + render_loc).to_f64(), Some(i));
            }
            let layout = self.layout(view);
            let frame_loc = view.loc - layout.client.loc;
            if let Some(part) = layout.part_at(p - frame_loc.to_f64()) {
                return Under::Frame(i, part);
            }
        }
        if let Some(u) = layer_hit(&[Layer::Bottom, Layer::Background]) {
            return u;
        }
        Under::Root
    }

    /// Met une vue au premier plan et lui donne le clavier
    pub fn focus_view(&mut self, i: usize) {
        let view = self.views.remove(i);
        let surface = view.surface().clone();
        self.views.push(view);
        self.set_keyboard_focus(Some(surface));
    }

    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        let serial = SERIAL_COUNTER.next_serial();
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, surface.clone(), serial);
        }
        for view in &self.views {
            let active = surface.as_ref() == Some(view.surface());
            if view.window.set_activated(active) {
                view.window.toplevel().unwrap().send_pending_configure();
            }
        }
    }

    /// Donne le clavier à la vue la plus en avant
    pub fn focus_top(&mut self) {
        let top = self
            .views
            .iter()
            .rev()
            .find(|v| v.mapped && !v.minimized)
            .map(|v| v.surface().clone());
        self.set_keyboard_focus(top);
    }

    /// Place une nouvelle vue au centre de la zone utile, décalée en
    /// cascade si une autre vue occupe déjà cette place (§ 8.2, simplifié)
    pub fn place(&mut self, i: usize) {
        let Some(output) = self.output_at(self.pointer) else {
            return;
        };
        let zone = self.usable_area(&output);
        let layout = self.layout(&self.views[i]);
        let mut frame = Point::from((
            zone.loc.x + (zone.size.w - layout.size.w).max(0) / 2,
            zone.loc.y + (zone.size.h - layout.size.h).max(0) / 2,
        ));
        let step = Layout::title_height(&self.config, &self.theme);
        while self
            .views
            .iter()
            .enumerate()
            .any(|(j, v)| j != i && v.mapped && v.loc - self.layout(v).client.loc == frame)
        {
            frame += Point::from((step, step));
            if frame.x + layout.size.w > zone.loc.x + zone.size.w
                || frame.y + layout.size.h > zone.loc.y + zone.size.h
            {
                frame = zone.loc;
                break;
            }
        }
        self.views[i].loc = frame + layout.client.loc;
    }

    pub fn toggle_maximize(&mut self, i: usize) {
        let Some(output) = self.output_at(self.pointer) else {
            return;
        };
        let toplevel = self.views[i].window.toplevel().unwrap().clone();
        if let Some(prev) = self.views[i].maximized.take() {
            self.views[i].loc = prev.loc;
            toplevel.with_pending_state(|s| {
                s.states.unset(xdg_toplevel::State::Maximized);
                s.size = Some(prev.size);
            });
        } else {
            let zone = self.usable_area(&output);
            let geo = self.views[i].window.geometry();
            self.views[i].maximized = Some(Rectangle::new(self.views[i].loc, geo.size));
            let layout = self.layout(&self.views[i]);
            let deco = layout.size - geo.size;
            self.views[i].loc = zone.loc + layout.client.loc;
            toplevel.with_pending_state(|s| {
                s.states.set(xdg_toplevel::State::Maximized);
                s.size = Some(zone.size - deco);
            });
        }
        toplevel.send_pending_configure();
    }
}
