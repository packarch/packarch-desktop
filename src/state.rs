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
    /// Surface d'une vue (fenêtres surgissantes comprises), et position de
    /// son origine (globale)
    View(usize, WlSurface, Point<f64, Logical>),
    /// Partie du cadre d'une vue
    Frame(usize, Part),
    /// Surface d'une couche; `background`: fond ou dessous des fenêtres
    /// (comme le bureau pour les liaisons de la souris)
    Layer(WlSurface, Point<f64, Logical>, bool),
    /// Bureau sans surface
    Root,
}

/// Appui d'un bouton de la souris en cours (§ 5.4)
#[derive(Debug, Clone)]
pub struct Press {
    pub button: crate::bindings::MouseButton,
    pub mods: crate::bindings::Mods,
    pub contexts: Vec<crate::bindings::Context>,
    pub window: Option<Window>,
    pub location: Point<f64, Logical>,
    pub dragged: bool,
    /// Un raccourci a pris l'appui: le client ne le reçoit pas
    pub intercepted: bool,
}

/// Parcours Alt+Tab en cours (sans liste à l'écran avant la 0.4)
#[derive(Debug, Clone)]
pub struct Cycle {
    pub windows: Vec<Window>,
    pub index: usize,
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
    /// Tampons dmabuf des captures (ext-image-copy-capture)
    pub capture_dmabuf: Option<crate::protocols::image_capture::CaptureDmabuf>,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<State>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub viewporter_state: ViewporterState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub popups: PopupManager,
    pub seat: Seat<State>,

    /// Vues, de l'arrière vers l'avant (à couche d'empilement égale)
    pub views: Vec<View>,
    pub outputs: Vec<Output>,
    pub config: Config,
    pub theme: Theme,
    pub text: Text,
    pub shaders: Option<Shaders>,
    /// Backend (sorti de l'état pendant le rendu)
    pub backend: Option<crate::backend::Backend>,
    pub pointer: Point<f64, Logical>,
    /// Bureau affiché, et le précédent (GoToDesktop « last »)
    pub desktop: usize,
    pub last_desktop: usize,
    /// ToggleShowDesktop: toutes les vues cachées
    pub show_desktop: bool,
    /// Vues de la plus récemment active à la plus ancienne
    pub mru: Vec<Window>,
    pub cycle: Option<Cycle>,
    pub press: Option<Press>,
    /// Dernier clic (bouton, premier contexte, vue, instant), pour le double clic
    pub last_click: Option<(
        crate::bindings::MouseButton,
        crate::bindings::Context,
        Option<Window>,
        Instant,
    )>,
    /// Touches dont l'appui a déclenché un raccourci: leur relâche n'est pas
    /// transmise
    pub suppressed_keys: Vec<smithay::input::keyboard::Keycode>,
    /// Disposition « us », pour retrouver la touche physique (§ 5.3)
    pub us_keymap: Option<smithay::input::keyboard::xkb::Keymap>,
    /// Aperçu du collage pendant un déplacement (§ 8.6)
    pub snap_preview: Option<Rectangle<i32, Logical>>,
    /// Facteur de la loupe, si elle est ouverte
    pub magnifier: Option<f64>,
    /// Texture où l'écran est rendu pour la loupe, par écran
    pub magnifier_textures: Vec<(smithay::output::Output, crate::render::MagnifierTexture)>,
    /// Contour d'un redimensionnement sans dessin du contenu (§ 8.7)
    pub resize_outline: Option<Rectangle<i32, Logical>>,
    /// Aperçu affiché et son animation
    pub snap_shown: Option<(Rectangle<i32, Logical>, Option<crate::anim::Anim>)>,
    /// Défilement accumulé (pavé tactile) pour les liaisons de la molette
    pub scroll_acc: (f64, f64),
    /// Commandes de la session: variables d'environnement à leur passer
    pub child_env: Vec<(String, String)>,
    /// Variables du fichier `environment`
    pub env_vars: Vec<(String, String)>,

    /// Menus de `menu.xml` (§ 5.5), menus ouverts, menus qui disparaissent
    pub menus: crate::menu::Menus,
    pub menu: Option<crate::menu::ui::MenuState>,
    pub menu_ghosts: Vec<crate::menu::ui::MenuGhost>,
    pub icons: crate::icons::Icons,
    /// Résultats des pipemenus (jeton, sortie)
    pub pipe_tx: smithay::reexports::calloop::channel::Sender<(u64, Option<String>)>,
    pub pipe_counter: u64,
    /// Horloge des animations (§ 10.5)
    pub clock: crate::anim::Clock,
    /// `--no-animations`
    pub no_animations: bool,
    /// Vues fermées ou réduites, encore animées
    pub ghosts: Vec<crate::render::Ghost>,

    /// Protocoles du § 9.2
    pub protocols: crate::protocols::Protocols,
    /// Verrouillage (ext-session-lock) et surfaces de verrouillage par écran
    pub lock: crate::protocols::Lock,
    pub lock_surfaces: Vec<(Output, smithay::wayland::session_lock::LockSurface)>,
    /// Fenêtres dessinées par leur texture hors écran au dernier rendu
    /// (surface, identifiant de l'élément du contenu) : leurs surfaces
    /// n'apparaissent pas dans les états du rendu, `after_render` les y
    /// ajoute
    pub offscreen_shown: Vec<(WlSurface, smithay::backend::renderer::element::Id)>,
    /// Surfaces qui empêchent la mise en veille (vidéos)
    pub idle_inhibitors: Vec<WlSurface>,
    /// Curseur demandé par le client sous le pointeur
    pub cursor_status: smithay::input::pointer::CursorImageStatus,
    /// Point chaud du dernier curseur rendu (pixels, depuis son coin haut
    /// gauche): plan curseur des pilotes virtuels (HOTSPOT_X/HOTSPOT_Y)
    pub cursor_hotspot: Option<smithay::utils::Point<i32, smithay::utils::Physical>>,
    /// Icône d'un glisser-déposer en cours (wl_data_device.start_drag),
    /// dessinée sous le pointeur, et son décalage cumulé (wl_surface.offset)
    pub dnd_icon: Option<(WlSurface, smithay::utils::Point<i32, smithay::utils::Logical>)>,
    /// Source d'un glisser-déposer Wayland, lue par les clients X11 (XDND)
    pub dnd_source: Option<smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource>,
    /// Glisser-déposer d'un client X11 au-dessus des surfaces Wayland
    pub x11_drag: Option<smithay::wayland::selection::data_device::ForeignDnd<State>>,
    /// OSD affiché (Alt+Tab, nom du bureau) et celui qui disparaît
    pub osd: Option<crate::osd::Osd>,
    /// Logo du fond (`<core><logo>`)
    pub logo: crate::logo::Logo,
    pub osd_ghost: Option<crate::osd::OsdGhost>,
    /// Applications installées (fichiers .desktop)
    pub apps: crate::apps::Apps,
    /// Glissement du changement de bureau: animation, ancien bureau
    pub desk_anim: Option<(crate::anim::Anim, usize)>,
    /// XWayland (§ 9.4)
    pub xwayland: crate::xwayland::Xwayland,
    /// Le script autostart a été lancé
    pub autostarted: bool,
    /// Commandes de `--startup`, lancées après le script autostart (§ 12)
    pub startup: Vec<String>,
    /// L'environnement a été exporté vers systemd et D-Bus
    pub env_exported: bool,
    /// `packarch-session.target` a été démarrée
    pub session_target: bool,
    /// Bus D-Bus propre à la session (adresse, processus), quand une autre
    /// session est active
    pub private_bus: Option<(String, i32)>,
    /// Synchronisation explicite (linux-drm-syncobj-v1), si le GPU la permet
    pub syncobj_state: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
    /// Surfaces qui ont demandé le cadre du compositeur par la décoration KDE
    pub kde_ssd: Vec<WlSurface>,
    /// Dernière zone utile connue de chaque écran (barres)
    pub zones: std::collections::HashMap<String, Rectangle<i32, Logical>>,
    /// Raccourcis désactivés (ToggleKeybinds), sauf celui qui les rétablit
    pub keybinds_off: bool,
    /// Curseurs du thème (session réelle)
    pub cursors: Option<crate::cursor::Cursors>,
    /// Horloge monotone (presentation-time)
    pub monotonic: smithay::utils::Clock<smithay::utils::Monotonic>,
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    /// Client lancé dans un bac à sable (security-context)
    pub security_context: Option<smithay::wayland::security_context::SecurityContext>,
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
        // Mode par défaut annoncé aux clients GTK3: celui de `core/decoration`
        let kde_decoration_state = {
            use smithay::reexports::wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode;
            let mode = if config.server_decoration {
                Mode::Server
            } else {
                Mode::Client
            };
            KdeDecorationState::new::<State>(&dh, mode)
        };
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
                    // Une requête d'un client peut changer l'image
                    state.request_redraw();
                    Ok(PostAction::Continue)
                },
            )
            .expect("source de l'affichage");
        info!("socket Wayland: {}", socket_name.to_string_lossy());

        let (pipe_tx, pipe_rx) = smithay::reexports::calloop::channel::channel();
        handle
            .insert_source(pipe_rx, |event, _, state: &mut State| {
                if let smithay::reexports::calloop::channel::Event::Msg((token, out)) = event {
                    state.pipe_done(token, out);
                }
            })
            .expect("source des pipemenus");
        let theme = Theme::load(&config);
        let menus = crate::menu::Menus::load(&config.dir).unwrap_or_else(|e| {
            warn!("{e}");
            crate::menu::Menus::fallback()
        });
        let protocols = crate::protocols::Protocols::new(&dh, &handle, &primary_selection_state);
        let xwayland_shell = smithay::wayland::xwayland_shell::XWaylandShellState::new::<State>(&dh);
        let mut icons = crate::icons::Icons::default();
        icons.set_theme(&config.icon_theme);
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
            capture_dmabuf: None,
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
            desktop: 0,
            last_desktop: 0,
            show_desktop: false,
            mru: vec![],
            cycle: None,
            press: None,
            last_click: None,
            suppressed_keys: vec![],
            us_keymap: us_keymap(),
            snap_preview: None,
            magnifier: None,
            magnifier_textures: vec![],
            snap_shown: None,
            resize_outline: None,
            scroll_acc: (0.0, 0.0),
            child_env,
            env_vars: vec![],
            menus,
            menu: None,
            menu_ghosts: vec![],
            icons,
            pipe_tx,
            pipe_counter: 0,
            clock: crate::anim::Clock::default(),
            no_animations: false,
            ghosts: vec![],
            protocols,
            lock: crate::protocols::Lock::Unlocked,
            lock_surfaces: vec![],
            offscreen_shown: vec![],
            idle_inhibitors: vec![],
            cursor_status: smithay::input::pointer::CursorImageStatus::default_named(),
            cursor_hotspot: None,
            dnd_icon: None,
            dnd_source: None,
            x11_drag: None,
            monotonic: smithay::utils::Clock::new(),
            osd: None,
            osd_ghost: None,
            apps: crate::apps::Apps::default(),
            logo: crate::logo::Logo::default(),
            desk_anim: None,
            cursors: None,
            xwayland: crate::xwayland::Xwayland {
                wm: None,
                client: None,
                scale: 1.0,
                display: None,
                unmanaged: vec![],
                shell: xwayland_shell,
            },
            autostarted: false,
            startup: vec![],
            env_exported: false,
            session_target: false,
            keybinds_off: false,
            private_bus: None,
            syncobj_state: None,
            zones: std::collections::HashMap::new(),
            kde_ssd: vec![],
        }
    }

    /// Instant présent de l'horloge des animations
    pub fn now(&self) -> std::time::Duration {
        self.clock.now(self.start)
    }

    /// Lance `sh -c commande`, détachée, avec l'environnement de la session
    pub fn spawn(&self, command: &str) {
        let mut cmd = self.command(command);
        match cmd.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(e) => warn!("lancement de « {command} »: {e}"),
        }
    }

    /// Commande `sh -c …` avec l'environnement de la session
    pub fn command(&self, command: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(command).stdin(std::process::Stdio::null());
        cmd.env_remove("WAYLAND_SOCKET");
        // Les signaux bloqués pour la boucle d'événements ne le restent pas
        // dans les programmes lancés
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(|| {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
                Ok(())
            });
        }
        for (k, v) in &self.child_env {
            cmd.env(k, v);
        }
        cmd
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
        let m = self.config.margin;
        Rectangle::new(
            (zone.loc.x + m.left, zone.loc.y + m.top).into(),
            (
                (zone.size.w - m.left - m.right).max(1),
                (zone.size.h - m.top - m.bottom).max(1),
            )
                .into(),
        )
    }

    pub fn focused_surface(&self) -> Option<WlSurface> {
        self.seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .map(|f| f.surface)
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
        Layout::new(
            &self.config,
            &self.theme,
            view.client_size(),
            view.frame(self.config.keep_border),
        )
    }

    /// Une vue est affichée: sur le bureau courant, ni réduite ni cachée
    pub fn visible(&self, view: &View) -> bool {
        view.mapped && !view.minimized && !self.show_desktop && view.on_desktop(self.desktop)
    }

    /// Indices des vues affichées, de l'arrière vers l'avant
    pub fn stacking(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.views.len())
            .filter(|&i| self.visible(&self.views[i]))
            .collect();
        order.sort_by_key(|&i| self.views[i].stack);
        order
    }

    /// Vue en plein écran au premier plan: dessinée devant les panneaux
    pub fn fullscreen_top(&self) -> Option<usize> {
        let top = *self.stacking().last()?;
        (self.views[top].tiling == crate::view::Tiling::Fullscreen).then_some(top)
    }

    fn view_under(&self, i: usize, p: Point<f64, Logical>) -> Option<Under> {
        let view = &self.views[i];
        let render_loc = view.loc - view.window.geometry().loc;
        if !view.shaded
            && let Some((s, loc)) = view
                .window
                .surface_under(p - render_loc.to_f64(), WindowSurfaceType::ALL)
        {
            return Some(Under::View(i, s, (loc + render_loc).to_f64()));
        }
        let layout = self.layout(view);
        let frame_loc = view.loc - layout.client.loc;
        layout
            .part_at(p - frame_loc.to_f64())
            .map(|part| Under::Frame(i, part))
    }

    /// Ce qui se trouve au point `p` (coordonnées globales)
    pub fn under(&self, p: Point<f64, Logical>) -> Under {
        self.under_with(p, true)
    }

    /// Comme `under`; `unmanaged` faux: sans les fenêtres X11 non gérées
    /// (l'icône d'un glisser-déposer X11 suit le pointeur)
    pub fn under_with(&self, p: Point<f64, Logical>, unmanaged: bool) -> Under {
        let output = self.output_at(p);
        if unmanaged
            && !self.is_locked()
            && let Some((s, loc)) = self.unmanaged_under(p)
        {
            return Under::Layer(s, loc, false);
        }
        if self.is_locked() {
            return output
                .as_ref()
                .and_then(|o| {
                    let s = self.lock_surface(o)?;
                    Some(Under::Layer(
                        s.wl_surface().clone(),
                        o.current_location().to_f64(),
                        false,
                    ))
                })
                .unwrap_or(Under::Root);
        }
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
                        let background = matches!(l, Layer::Bottom | Layer::Background);
                        return Some(Under::Layer(s, origin, background));
                    }
                }
            }
            None
        };
        if let Some(u) = layer_hit(&[Layer::Overlay]) {
            return u;
        }
        let full = self.fullscreen_top();
        if let Some(u) = full.and_then(|i| self.view_under(i, p)) {
            return u;
        }
        if let Some(u) = layer_hit(&[Layer::Top]) {
            return u;
        }
        for i in self.stacking().into_iter().rev() {
            if Some(i) == full {
                continue;
            }
            if let Some(u) = self.view_under(i, p) {
                return u;
            }
        }
        if let Some(u) = layer_hit(&[Layer::Bottom, Layer::Background]) {
            return u;
        }
        Under::Root
    }

    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        // Verrouillé: seule une surface de verrouillage reçoit le clavier
        if self.is_locked()
            && surface
                .as_ref()
                .is_some_and(|s| !self.lock_surfaces.iter().any(|(_, l)| l.wl_surface() == s))
        {
            return;
        }
        let serial = SERIAL_COUNTER.next_serial();
        let focus = surface.clone().map(|s| {
            let x11 = self.view_index(&s).and_then(|i| self.views[i].x11().cloned());
            KbFocus { surface: s, x11 }
        });
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, focus, serial);
        }
        for view in &self.views {
            let active = surface.as_ref() == Some(view.surface());
            if view.window.set_activated(active)
                && let Some(t) = view.window.toplevel()
            {
                t.send_pending_configure();
            }
        }
        if let Some(i) = surface.as_ref().and_then(|s| self.view_index(s)) {
            let w = self.views[i].window.clone();
            if self.cycle.is_none() {
                self.mru.retain(|x| *x != w);
                self.mru.insert(0, w);
            }
        }
    }
}

/// Disposition « us » (touches physiques des raccourcis)
fn us_keymap() -> Option<smithay::input::keyboard::xkb::Keymap> {
    use smithay::input::keyboard::xkb;
    let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_names(&ctx, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
}

/// Cible du clavier: une surface, et sa fenêtre X11 si c'en est une (le
/// focus X lui est alors donné aussi: SetInputFocus, WM_TAKE_FOCUS)
#[derive(Debug, Clone, PartialEq)]
pub struct KbFocus {
    pub surface: WlSurface,
    pub x11: Option<smithay::xwayland::X11Surface>,
}

impl From<WlSurface> for KbFocus {
    fn from(surface: WlSurface) -> KbFocus {
        KbFocus { surface, x11: None }
    }
}

impl From<smithay::desktop::PopupKind> for KbFocus {
    fn from(popup: smithay::desktop::PopupKind) -> KbFocus {
        KbFocus::from(popup.wl_surface().clone())
    }
}

impl smithay::utils::IsAlive for KbFocus {
    fn alive(&self) -> bool {
        self.surface.alive()
    }
}

impl smithay::wayland::seat::WaylandFocus for KbFocus {
    fn wl_surface(&self) -> Option<std::borrow::Cow<'_, WlSurface>> {
        Some(std::borrow::Cow::Borrowed(&self.surface))
    }
}

impl smithay::input::keyboard::KeyboardTarget<State> for KbFocus {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<smithay::input::keyboard::KeysymHandle<'_>>,
        serial: smithay::utils::Serial,
    ) {
        use smithay::input::keyboard::KeyboardTarget;
        match &self.x11 {
            Some(x) => KeyboardTarget::enter(x, seat, data, keys, serial),
            None => KeyboardTarget::enter(&self.surface, seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: smithay::utils::Serial) {
        use smithay::input::keyboard::KeyboardTarget;
        match &self.x11 {
            Some(x) => KeyboardTarget::leave(x, seat, data, serial),
            None => KeyboardTarget::leave(&self.surface, seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: smithay::input::keyboard::KeysymHandle<'_>,
        state: smithay::backend::input::KeyState,
        serial: smithay::utils::Serial,
        time: u32,
    ) {
        smithay::input::keyboard::KeyboardTarget::key(&self.surface, seat, data, key, state, serial, time)
    }

    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: smithay::input::keyboard::ModifiersState,
        serial: smithay::utils::Serial,
    ) {
        smithay::input::keyboard::KeyboardTarget::modifiers(&self.surface, seat, data, modifiers, serial)
    }
}

impl From<KbFocus> for WlSurface {
    fn from(f: KbFocus) -> WlSurface {
        f.surface
    }
}
