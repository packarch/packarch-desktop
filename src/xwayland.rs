// SPDX-License-Identifier: MIT OR Apache-2.0
//! XWayland (cahier des charges § 9.4): serveur X lancé au démarrage,
//! gestionnaire de fenêtres X11 de Smithay. Les fenêtres X11 sont des vues
//! comme les autres (cadre, focus, empilement, `_NET_WM_STATE`); les
//! fenêtres « override-redirect » (menus, bulles X11) sont dessinées sans
//! cadre devant les vues. Presse-papiers partagé avec Wayland.
//!
//! Échelle: par défaut les clients X11 sont dessinés à l'échelle 1 puis
//! agrandis (flous). Avec `<core><xwaylandSharp>`, XWayland reçoit une
//! échelle client égale à la plus grande échelle des écrans: les clients X
//! voient des écrans en pixels réels et sont rendus nets; le DPI leur est
//! donné par XSettings (GTK) et `Xft.dpi` (Qt, xrdb).

use std::os::fd::OwnedFd;
use std::process::Stdio;

use smithay::desktop::Window;
use smithay::desktop::utils::under_from_surface_tree;
use smithay::input::Seat;
use smithay::reexports::wayland_server::Client;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::protocol::wl_data_device_manager::DndAction;
use smithay::reexports::wayland_server::protocol::wl_data_source::WlDataSource;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::selection::data_device::{
    ForeignDnd, ForeignDndHandler, clear_data_device_selection, current_data_device_selection_userdata,
    request_data_device_client_selection, set_data_device_selection, with_source_metadata,
};
use smithay::wayland::selection::primary_selection::{
    clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection,
    set_primary_selection,
};
use smithay::wayland::selection::{SelectionHandler, SelectionSource, SelectionTarget};
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::settings::Value;
use smithay::xwayland::xwm::{Reorder, ResizeEdge, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};
use smithay::{delegate_xwayland_shell, desktop::WindowSurfaceType};
use tracing::{info, warn};

use crate::deco::Edges;
use crate::input::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use crate::state::State;
use crate::view::{Axis, Tiling, View};
use crate::wm::Origin;

/// Données du serveur X
pub struct Xwayland {
    pub wm: Option<X11Wm>,
    /// Client Wayland de XWayland, qui porte l'échelle client
    pub client: Option<Client>,
    /// Échelle client appliquée (pixels X par unité logique)
    pub scale: f64,
    pub display: Option<u32>,
    /// Fenêtres override-redirect affichées, de l'arrière vers l'avant
    pub unmanaged: Vec<X11Surface>,
    pub shell: XWaylandShellState,
}

impl State {
    /// Lance Xwayland; `DISPLAY` est exporté quand il est prêt
    pub fn start_xwayland(&mut self) {
        let env: Vec<(String, String)> = self.child_env.clone();
        let (xwayland, client) =
            match XWayland::spawn(&self.dh, None, env, true, Stdio::null(), Stdio::inherit(), |_| ()) {
                Ok(x) => x,
                Err(e) => {
                    warn!("XWayland: {e}");
                    self.run_autostart();
                    return;
                }
            };
        let result = self
            .handle
            .insert_source(xwayland, move |event, _, state| match event {
                XWaylandEvent::Ready {
                    x11_socket,
                    display_number,
                } => {
                    match X11Wm::start_wm(state.handle.clone(), x11_socket, client.clone()) {
                        Ok(wm) => state.xwayland.wm = Some(wm),
                        Err(e) => {
                            warn!("gestionnaire X11: {e}");
                            return;
                        }
                    }
                    state.xwayland.client = Some(client.clone());
                    state.xwayland.display = Some(display_number);
                    // Échelle de départ des clients: 1
                    state.xwayland.scale = 1.0;
                    state.update_xwayland_scale();
                    let var = format!(":{display_number}");
                    info!("XWayland prêt: DISPLAY={var}");
                    state.child_env.retain(|(k, _)| k != "DISPLAY");
                    state.child_env.push(("DISPLAY".into(), var));
                    state.export_session_env();
                    state.load_xresources();
                    state.run_autostart();
                }
                XWaylandEvent::Error => {
                    warn!("XWayland s'est arrêté au démarrage");
                    state.run_autostart();
                }
            });
        if let Err(e) = result {
            warn!("XWayland: {e}");
        }
    }

    /// Échelle voulue pour les clients X11: 1 (agrandis par le compositeur)
    /// ou la plus grande échelle des écrans (`xwaylandSharp`)
    fn wanted_xwayland_scale(&self) -> f64 {
        if !self.config.xwayland_sharp {
            return 1.0;
        }
        self.outputs
            .iter()
            .map(|o| o.current_scale().fractional_scale())
            .fold(1.0, f64::max)
    }

    /// Applique l'échelle client de XWayland si elle a changé: écrans
    /// annoncés de nouveau, DPI, fenêtres X11 reconfigurées
    pub fn update_xwayland_scale(&mut self) {
        let scale = self.wanted_xwayland_scale();
        if self.xwayland.wm.is_none() || scale == self.xwayland.scale {
            return;
        }
        let Some(data) = self
            .xwayland
            .client
            .as_ref()
            .and_then(|c| c.get_data::<smithay::xwayland::XWaylandClientData>())
        else {
            return;
        };
        data.compositor_state.set_client_scale(scale);
        self.xwayland.scale = scale;
        info!("XWayland: échelle des clients X11 {scale}");
        // xdg-output renvoie position et taille en pixels X
        for o in &self.outputs {
            o.change_current_state(None, None, None, None);
        }
        // GTK: facteur entier, le reste par le DPI; curseurs à la même échelle
        let dpi = (96.0 * scale * 1024.0).round() as i32;
        let gdk = scale.floor().max(1.0);
        let (_, cursor) = self.cursor_settings();
        let settings = [
            ("Xft/DPI", Value::Integer(dpi)),
            ("Gdk/WindowScalingFactor", Value::Integer(gdk as i32)),
            (
                "Gdk/UnscaledDPI",
                Value::Integer((96.0 * scale / gdk * 1024.0).round() as i32),
            ),
            (
                "Xcursor/size",
                Value::Integer((cursor as f64 * scale).round() as i32),
            ),
        ];
        if let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(e) = wm.set_xsettings(settings.into_iter().map(|(k, v)| (k.to_string(), v)))
        {
            warn!("XSettings: {e}");
        }
        self.set_xft_dpi((96.0 * scale).round() as u32);
        // Les fenêtres reçoivent leur taille en pixels X
        for v in &self.views {
            if let Some(x) = v.x11() {
                let _ = x.configure(None);
            }
        }
    }

    /// Ressources X du système et de l'utilisateur (`/etc/X11/Xresources`,
    /// `~/.Xresources`) chargées dans XWayland par xrdb s'il est installé:
    /// le script de session de LightDM les charge avant le compositeur, quand
    /// aucun serveur X n'existe encore (« xrdb: Can't open display '' »)
    fn load_xresources(&self) {
        let Some(n) = self.xwayland.display else { return };
        let home = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".Xresources"));
        let files: Vec<_> = [Some(std::path::PathBuf::from("/etc/X11/Xresources")), home]
            .into_iter()
            .flatten()
            .filter(|f| f.is_file())
            .collect();
        if files.is_empty() {
            return;
        }
        // Dans un fil: le compositeur ne bloque pas, l'ordre des fichiers est gardé
        std::thread::spawn(move || {
            for f in files {
                let status = std::process::Command::new("xrdb")
                    .arg("-merge")
                    .arg(&f)
                    .env("DISPLAY", format!(":{n}"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .status();
                match status {
                    Ok(s) if s.success() => info!("ressources X: {}", f.display()),
                    Ok(s) => warn!("xrdb {}: {s}", f.display()),
                    Err(e) => {
                        tracing::debug!("xrdb: {e}");
                        return;
                    }
                }
            }
        });
    }

    /// `Xft.dpi` dans les ressources X (Qt, Xft), par xrdb s'il est installé
    fn set_xft_dpi(&self, dpi: u32) {
        use std::io::Write;
        let Some(n) = self.xwayland.display else { return };
        let child = std::process::Command::new("xrdb")
            .args(["-nocpp", "-merge"])
            .env("DISPLAY", format!(":{n}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match child {
            Ok(mut child) => {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = writeln!(stdin, "Xft.dpi: {dpi}");
                }
                // Attente dans un fil: le compositeur ne bloque pas
                std::thread::spawn(move || child.wait());
            }
            Err(e) => tracing::debug!("xrdb: {e}"),
        }
    }

    // --- Glisser-déposer entre Wayland et X11 (XDND) ------------------

    /// Surface d'un client X11 (servie par XWayland)
    pub fn is_xwayland_surface(&self, surface: &WlSurface) -> bool {
        self.xwayland
            .client
            .as_ref()
            .is_some_and(|c| surface.client().as_ref() == Some(c))
    }

    /// Fenêtre X11 d'une surface
    fn x11_of_surface(&self, surface: &WlSurface) -> Option<X11Surface> {
        self.views
            .iter()
            .filter_map(|v| v.x11())
            .chain(self.xwayland.unmanaged.iter())
            .find(|x| x.wl_surface().as_ref() == Some(surface))
            .cloned()
    }

    /// Un glisser-déposer Wayland commence: ses types sont annoncés aux
    /// clients X11 (XdndSelection)
    pub fn start_dnd_source(&mut self, source: Option<WlDataSource>) {
        self.dnd_source = source;
        let mime_types = self
            .dnd_source
            .as_ref()
            .and_then(|s| with_source_metadata(s, |m| m.mime_types.clone()).ok());
        if let Some(mime_types) = mime_types
            && let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(e) = wm.dnd_source(Some(mime_types))
        {
            warn!("glisser-déposer vers X11: {e}");
        }
    }

    /// Fin du glisser-déposer Wayland
    pub fn end_dnd_source(&mut self) {
        self.dnd_source = None;
        if let Some(wm) = self.xwayland.wm.as_mut() {
            let _ = wm.dnd_source(None);
        }
    }

    /// Le glisser-déposer Wayland passe sur une fenêtre X11 (`None`: il en
    /// sort)
    pub fn dnd_over_x11(&mut self, source: &WlDataSource, surface: Option<&WlSurface>) {
        let actions = with_source_metadata(source, |m| m.dnd_action).unwrap_or(DndAction::Copy);
        let window = surface.and_then(|s| self.x11_of_surface(s));
        let location = self.pointer;
        if let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(e) = wm.dnd_motion(window.as_ref(), location, actions)
        {
            warn!("glisser-déposer vers X11: {e}");
        }
    }

    /// Dépôt sur une fenêtre X11: vrai si elle l'accepte
    pub fn dnd_drop_on_x11(&mut self) -> bool {
        self.xwayland
            .wm
            .as_mut()
            .is_some_and(|wm| wm.dnd_drop().unwrap_or(false))
    }

    /// Fenêtre mandataire XDND: un client X11 glisse (bouton enfoncé sur
    /// une fenêtre X11) et le pointeur est au-dessus d'une surface Wayland.
    /// Placée devant les fenêtres X11, elle reçoit les messages XDND
    pub fn update_dnd_proxy(&mut self) {
        let Some(wm) = self.xwayland.wm.as_ref() else {
            return;
        };
        let pointer = self.seat.get_pointer().unwrap();
        let dragging = wm.x11_drag_active()
            && self.press.is_some()
            && pointer
                .current_focus()
                .is_some_and(|s| self.is_xwayland_surface(&s));
        let shown = dragging
            && match self.under_with(self.pointer, false) {
                crate::state::Under::View(_, s, _) | crate::state::Under::Layer(s, _, _) => {
                    !self.is_xwayland_surface(&s)
                }
                _ => false,
            };
        if let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(e) = wm.set_dnd_proxy(shown)
        {
            warn!("glisser-déposer depuis X11: {e}");
        }
    }

    /// Surface Wayland (pas X11) au point `p` et position locale
    fn wayland_surface_at(&self, p: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        match self.under_with(p, false) {
            crate::state::Under::View(_, s, loc) | crate::state::Under::Layer(s, loc, _)
                if !self.is_xwayland_surface(&s) =>
            {
                Some((s, p - loc))
            }
            _ => None,
        }
    }

    fn x11_view(&self, window: &X11Surface) -> Option<usize> {
        self.views.iter().position(|v| v.x11() == Some(window))
    }

    /// Fait d'une fenêtre X11 affichée et associée à sa surface une vue
    fn manage_x11(&mut self, window: &X11Surface) {
        if window.is_override_redirect() || !window.is_mapped() || self.x11_view(window).is_some() {
            return;
        }
        let Some(surface) = window.wl_surface() else {
            return;
        };
        let mut view = View::new(
            Window::new_x11_window(window.clone()),
            surface.clone(),
            self.desktop,
        );
        // Indications Motif « sans décoration » (Steam…): le client dessine
        // son cadre
        view.decorated = !window.is_decorated();
        view.loc = window.geometry().loc;
        self.views.push(view);
        let i = self.views.len() - 1;
        let has_buffer = smithay::backend::renderer::utils::with_renderer_surface_state(&surface, |s| {
            s.buffer().is_some()
        })
        .unwrap_or(false);
        if has_buffer {
            self.map_view(i);
        }
    }

    /// Fenêtre X11 retirée (cachée ou détruite)
    fn forget_x11(&mut self, window: &X11Surface) {
        self.xwayland.unmanaged.retain(|w| w != window);
        if let Some(i) = self.x11_view(window) {
            if self.views[i].mapped {
                self.ghost_view(i, crate::render::GhostKind::Close);
            }
            let view = self.views.remove(i);
            self.mru.retain(|w| *w != view.window);
            if let Some(c) = self.cycle.as_mut() {
                c.windows.retain(|w| *w != view.window);
                if c.windows.is_empty() {
                    self.cycle = None;
                }
            }
            self.focus_top();
        }
    }

    /// Surface X11 sans cadre sous un point (menus, bulles)
    pub fn unmanaged_under(&self, p: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        for w in self.xwayland.unmanaged.iter().rev() {
            let Some(s) = w.wl_surface() else { continue };
            let geo = w.geometry();
            if let Some((s, loc)) = under_from_surface_tree(&s, p, geo.loc, WindowSurfaceType::ALL) {
                return Some((s, loc.to_f64()));
            }
        }
        None
    }

    /// Fenêtres X11 sans cadre: surfaces et positions
    pub fn unmanaged_surfaces(&self) -> Vec<(WlSurface, Rectangle<i32, Logical>)> {
        self.xwayland
            .unmanaged
            .iter()
            .filter_map(|w| Some((w.wl_surface()?, w.geometry())))
            .collect()
    }

    /// Met une fenêtre X11 au premier plan côté X (ordre d'empilement)
    pub fn raise_x11(&mut self, i: usize) {
        if let Some(x) = self.views[i].x11().cloned()
            && let Some(wm) = self.xwayland.wm.as_mut()
        {
            let _ = wm.raise_window(&x);
        }
    }

    /// Position de la vue transmise aux fenêtres X11 qui ont bougé (les
    /// clients X connaissent leur position, pour placer leurs menus)
    pub fn sync_x11_positions(&mut self) {
        for v in &self.views {
            if let Some(x) = v.x11()
                && v.mapped
                && v.resize.is_none()
            {
                let geo = x.geometry();
                if geo.loc != v.loc {
                    let _ = x.configure(Rectangle::new(v.loc, geo.size));
                }
            }
        }
    }
}

fn button_code(b: u32) -> u32 {
    match b {
        2 => BTN_MIDDLE,
        3 => BTN_RIGHT,
        _ => BTN_LEFT,
    }
}

impl XwmHandler for State {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwayland.wm.as_mut().expect("gestionnaire X11")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Err(e) = window.set_mapped(true) {
            warn!("X11: {e}");
            return;
        }
        self.manage_x11(&window);
    }

    fn map_window_notify(&mut self, _xwm: XwmId, window: X11Surface) {
        self.manage_x11(&window);
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.xwayland.unmanaged.retain(|w| *w != window);
        self.xwayland.unmanaged.push(window);
    }

    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.forget_x11(&window);
        if !window.is_override_redirect() {
            let _ = window.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.forget_x11(&window);
    }

    /// Une vue gérée garde sa position; sa taille suit la demande si elle
    /// est libre. Une fenêtre pas encore affichée est configurée comme
    /// demandé
    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        let mut geo = window.geometry();
        match self.x11_view(&window) {
            Some(i) => {
                if self.views[i].tiling == Tiling::None {
                    if let Some(w) = w {
                        geo.size.w = w as i32;
                    }
                    if let Some(h) = h {
                        geo.size.h = h as i32;
                    }
                }
                geo.loc = self.views[i].loc;
            }
            None => {
                if let Some(x) = x {
                    geo.loc.x = x;
                }
                if let Some(y) = y {
                    geo.loc.y = y;
                }
                if let Some(w) = w {
                    geo.size.w = w as i32;
                }
                if let Some(h) = h {
                    geo.size.h = h as i32;
                }
            }
        }
        let _ = window.configure(geo);
    }

    fn configure_notify(
        &mut self,
        _xwm: XwmId,
        _window: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, button: u32, edge: ResizeEdge) {
        let Some(i) = self.x11_view(&window) else { return };
        let edges = match edge {
            ResizeEdge::Top => Edges {
                top: true,
                ..Default::default()
            },
            ResizeEdge::Bottom => Edges {
                bottom: true,
                ..Default::default()
            },
            ResizeEdge::Left => Edges {
                left: true,
                ..Default::default()
            },
            ResizeEdge::Right => Edges {
                right: true,
                ..Default::default()
            },
            ResizeEdge::TopLeft => Edges {
                top: true,
                left: true,
                ..Default::default()
            },
            ResizeEdge::TopRight => Edges {
                top: true,
                right: true,
                ..Default::default()
            },
            ResizeEdge::BottomLeft => Edges {
                bottom: true,
                left: true,
                ..Default::default()
            },
            ResizeEdge::BottomRight => Edges {
                bottom: true,
                right: true,
                ..Default::default()
            },
        };
        let origin = Origin {
            button: Some(button_code(button)),
            location: self.pointer,
        };
        self.start_resize_edges(i, edges, origin);
    }

    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, button: u32) {
        let Some(i) = self.x11_view(&window) else { return };
        let origin = Origin {
            button: Some(button_code(button)),
            location: self.pointer,
        };
        self.run_actions(
            &[crate::actions::Action::Move],
            Some(self.views[i].window.clone()),
            origin,
        );
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(i) = self.x11_view(&window) {
            self.maximize(i, Axis::Both);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(i) = self.x11_view(&window)
            && self.views[i].tiling.is_maximized()
        {
            self.restore(i);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(i) = self.x11_view(&window)
            && self.views[i].tiling != Tiling::Fullscreen
        {
            self.toggle_fullscreen(i);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(i) = self.x11_view(&window)
            && self.views[i].tiling == Tiling::Fullscreen
        {
            self.toggle_fullscreen(i);
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(i) = self.x11_view(&window) {
            self.iconify(i);
        }
    }

    fn unminimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(i) = self.x11_view(&window) {
            self.focus_view(i, true);
        }
    }

    // --- Presse-papiers: X11 → Wayland --------------------------------

    fn allow_selection_access(&mut self, _xwm: XwmId, _selection: SelectionTarget) -> bool {
        // Tout client X11 peut lire le presse-papiers, comme sous X11, sway
        // ou KWin (décision 34) : aussi ceux qui n'ont pas de fenêtre au
        // premier plan, comme spice-vdagent (copier depuis une machine
        // virtuelle) ou un gestionnaire de presse-papiers X11. Smithay ne
        // dit pas quel client demande : réserver la lecture à l'application
        // au premier plan les bloquait tous.
        true
    }

    fn send_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        let result = match selection {
            SelectionTarget::Clipboard => {
                request_data_device_client_selection(&self.seat, mime_type, fd).map_err(|e| format!("{e:?}"))
            }
            SelectionTarget::Primary => {
                request_primary_client_selection(&self.seat, mime_type, fd).map_err(|e| format!("{e:?}"))
            }
        };
        if let Err(e) = result {
            warn!("presse-papiers vers X11: {e}");
        }
    }

    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => set_data_device_selection(&self.dh, &self.seat, mime_types, XSel),
            SelectionTarget::Primary => set_primary_selection(&self.dh, &self.seat, mime_types, XSel),
        }
    }

    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.dh, &self.seat)
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.dh, &self.seat)
                }
            }
        }
    }

    // --- Glisser-déposer: Wayland → X11 --------------------------------

    fn send_dnd_data(&mut self, _xwm: XwmId, mime_type: String, fd: OwnedFd) {
        use std::os::fd::AsFd;
        if let Some(source) = self.dnd_source.as_ref().filter(|s| s.is_alive()) {
            source.send(mime_type, fd.as_fd());
        }
    }

    fn dnd_target_status(&mut self, _xwm: XwmId, accepted: bool, action: DndAction) {
        let Some(source) = self.dnd_source.as_ref().filter(|s| s.is_alive()) else {
            return;
        };
        // Retour au client source (curseur « interdit » ou non)
        let mime = with_source_metadata(source, |m| m.mime_types.first().cloned())
            .ok()
            .flatten();
        source.target(if accepted { mime } else { None });
        if accepted && source.version() >= 3 {
            source.action(action);
        }
    }

    fn dnd_target_finished(&mut self, _xwm: XwmId, success: bool, _action: DndAction) {
        if let Some(source) = self.dnd_source.take().filter(|s| s.is_alive()) {
            if !success {
                source.cancelled();
            } else if source.version() >= 3 {
                source.dnd_finished();
            }
        }
        self.end_dnd_source();
    }

    // --- Glisser-déposer: X11 → Wayland --------------------------------

    fn x11_drag_enter(&mut self, _xwm: XwmId, mime_types: Vec<String>, actions: DndAction) {
        if let Some(mut old) = self.x11_drag.take() {
            old.leave();
        }
        self.x11_drag = Some(ForeignDnd::new(&self.dh, &self.seat, mime_types, actions));
    }

    fn x11_drag_motion(&mut self, _xwm: XwmId, location: Point<f64, Logical>, _action: DndAction) {
        let target = self.wayland_surface_at(location);
        let time = self.start.elapsed().as_millis() as u32;
        if let Some(drag) = self.x11_drag.as_mut() {
            drag.motion(target.as_ref().map(|(s, l)| (s, *l)), time);
        }
    }

    fn x11_drag_leave(&mut self, _xwm: XwmId) {
        if let Some(mut drag) = self.x11_drag.take() {
            drag.leave();
        }
    }

    fn x11_drag_drop(&mut self, _xwm: XwmId) {
        let accepted = self.x11_drag.take().is_some_and(|mut d| d.drop());
        if let Some(wm) = self.xwayland.wm.as_mut() {
            if !accepted {
                let _ = wm.x11_drag_finished(false, DndAction::empty());
            }
            let _ = wm.set_dnd_proxy(false);
        }
    }

    fn disconnected(&mut self, _xwm: XwmId) {
        warn!("XWayland déconnecté");
        self.dnd_source = None;
        self.x11_drag = None;
        self.xwayland.wm = None;
        self.xwayland.client = None;
        self.xwayland.unmanaged.clear();
    }
}

/// Sélection posée par une application X11
#[derive(Debug, Clone, Copy)]
pub struct XSel;

// --- Presse-papiers: Wayland → X11 --------------------------------------

impl SelectionHandler for State {
    type SelectionUserData = XSel;

    fn new_selection(&mut self, ty: SelectionTarget, source: Option<SelectionSource>, _seat: Seat<Self>) {
        if let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(e) = wm.new_selection(ty, source.map(|s| s.mime_types()))
        {
            warn!("presse-papiers vers X11: {e}");
        }
    }

    fn send_selection(
        &mut self,
        ty: SelectionTarget,
        mime_type: String,
        fd: OwnedFd,
        _seat: Seat<Self>,
        _user_data: &XSel,
    ) {
        let handle = self.handle.clone();
        if let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(e) = wm.send_selection(ty, mime_type, fd, handle)
        {
            warn!("presse-papiers depuis X11: {e}");
        }
    }
}

// --- Glisser-déposer: X11 → Wayland (réponses du client Wayland) --------

impl ForeignDndHandler for State {
    fn foreign_dnd_status(&mut self, accepted: bool, action: DndAction) {
        if let Some(wm) = self.xwayland.wm.as_mut() {
            let _ = wm.x11_drag_status(accepted, action);
        }
    }

    fn foreign_dnd_receive(&mut self, mime_type: String, fd: OwnedFd) {
        let handle = self.handle.clone();
        if let Some(wm) = self.xwayland.wm.as_mut()
            && let Err(e) = wm.x11_drag_receive(mime_type, fd, handle)
        {
            warn!("glisser-déposer depuis X11: {e}");
        }
    }

    fn foreign_dnd_finished(&mut self, success: bool, action: DndAction) {
        if let Some(wm) = self.xwayland.wm.as_mut() {
            let _ = wm.x11_drag_finished(success, action);
        }
    }
}

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland.shell
    }

    fn surface_associated(&mut self, _xwm: XwmId, _surface: WlSurface, window: X11Surface) {
        self.manage_x11(&window);
    }
}

delegate_xwayland_shell!(State);
