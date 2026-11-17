// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gestion des fenêtres (cahier des charges § 7, § 8): focus, empilement,
//! placement, maximisation, collage, bureaux, et exécution des actions.

use smithay::desktop::Window;
use smithay::input::pointer::{Focus, GrabStartData};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size};
use tracing::{debug, info, warn};

use crate::actions::{Action, DesktopTarget};
use crate::config::{Config, Placement};
use crate::deco::Edges;
use crate::input::{BTN_LEFT, BTN_RIGHT, MoveGrab, ResizeGrab};
use crate::state::{Cycle, State};
use crate::theme::Theme;
use crate::view::{Axis, Edge, Stack, Tiling};

/// Origine d'une action: la souris (bouton et position de l'appui) ou le
/// clavier
#[derive(Debug, Clone, Copy)]
pub struct Origin {
    pub button: Option<u32>,
    pub location: Point<f64, Logical>,
}

fn overlap(a: Rectangle<i32, Logical>, b: Rectangle<i32, Logical>) -> i64 {
    a.intersection(b)
        .map(|r| r.size.w as i64 * r.size.h as i64)
        .unwrap_or(0)
}

/// Rectangle d'une région en pourcentage d'une zone
fn percent_rect(zone: Rectangle<i32, Logical>, x: f64, y: f64, w: f64, h: f64) -> Rectangle<i32, Logical> {
    let px = |v: f64, total: i32| (v * total as f64 / 100.0).round() as i32;
    Rectangle::new(
        (zone.loc.x + px(x, zone.size.w), zone.loc.y + px(y, zone.size.h)).into(),
        (px(w, zone.size.w), px(h, zone.size.h)).into(),
    )
}

/// Rétrécit un rectangle de `gap` pixels de chaque côté
fn inset(r: Rectangle<i32, Logical>, gap: i32) -> Rectangle<i32, Logical> {
    Rectangle::new(
        (r.loc.x + gap, r.loc.y + gap).into(),
        ((r.size.w - 2 * gap).max(1), (r.size.h - 2 * gap).max(1)).into(),
    )
}

/// Moitié ou totalité d'une zone pour un collage à un bord
pub fn edge_rect(zone: Rectangle<i32, Logical>, edge: Edge, gap: i32) -> Rectangle<i32, Logical> {
    let (x, y, w, h) = (zone.loc.x, zone.loc.y, zone.size.w, zone.size.h);
    let r = match edge {
        Edge::Left => Rectangle::new((x, y).into(), (w / 2, h).into()),
        Edge::Right => Rectangle::new((x + w / 2, y).into(), (w - w / 2, h).into()),
        Edge::Down => Rectangle::new((x, y + h / 2).into(), (w, h - h / 2).into()),
        Edge::Up | Edge::Center => zone,
    };
    inset(r, gap)
}

/// Position la moins recouverte pour un cadre de taille `size` dans `zone`
/// (§ 8.2): positions candidates aux bords de la zone et des autres vues,
/// la première de haut en bas puis de gauche à droite à recouvrement minimal
pub fn least_overlap(
    zone: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
    others: &[Rectangle<i32, Logical>],
) -> Point<i32, Logical> {
    let max_x = zone.loc.x + (zone.size.w - size.w).max(0);
    let max_y = zone.loc.y + (zone.size.h - size.h).max(0);
    let mut xs = vec![zone.loc.x, max_x];
    let mut ys = vec![zone.loc.y, max_y];
    for o in others {
        xs.extend([o.loc.x + o.size.w, o.loc.x - size.w]);
        ys.extend([o.loc.y + o.size.h, o.loc.y - size.h]);
    }
    let clamp = |v: i32, lo: i32, hi: i32| v.max(lo).min(hi);
    let mut xs: Vec<i32> = xs.into_iter().map(|x| clamp(x, zone.loc.x, max_x)).collect();
    let mut ys: Vec<i32> = ys.into_iter().map(|y| clamp(y, zone.loc.y, max_y)).collect();
    xs.sort();
    xs.dedup();
    ys.sort();
    ys.dedup();
    // Sans autre vue: centré
    if others.is_empty() {
        return Point::from((
            zone.loc.x + (zone.size.w - size.w).max(0) / 2,
            zone.loc.y + (zone.size.h - size.h).max(0) / 2,
        ));
    }
    let mut best = (i64::MAX, Point::from((zone.loc.x, zone.loc.y)));
    for &y in &ys {
        for &x in &xs {
            let r = Rectangle::new((x, y).into(), size);
            let cost: i64 = others.iter().map(|o| overlap(r, *o)).sum();
            if cost < best.0 {
                best = (cost, r.loc);
            }
        }
    }
    best.1
}

/// Bord vers lequel coller une vue déplacée, selon la position du pointeur
pub fn snap_edge(
    zone: Rectangle<i32, Logical>,
    p: Point<f64, Logical>,
    range: i32,
    top_maximize: bool,
) -> Option<Edge> {
    if range <= 0 {
        return None;
    }
    let r = range as f64;
    let (x0, y0) = (zone.loc.x as f64, zone.loc.y as f64);
    let (x1, y1) = (x0 + zone.size.w as f64, y0 + zone.size.h as f64);
    if p.x < x0 + r {
        Some(Edge::Left)
    } else if p.x >= x1 - r {
        Some(Edge::Right)
    } else if p.y < y0 + r {
        top_maximize.then_some(Edge::Up)
    } else if p.y >= y1 - r {
        Some(Edge::Down)
    } else {
        None
    }
}

/// Résistance des bords pendant un déplacement (§ 8.6): un bord du cadre
/// qui passe de moins de `strength` pixels un bord d'écran ou d'une autre
/// vue s'y arrête
pub fn resist(
    frame: Rectangle<i32, Logical>,
    zone: Rectangle<i32, Logical>,
    screen: i32,
    others: &[Rectangle<i32, Logical>],
    window: i32,
) -> Point<i32, Logical> {
    let mut loc = frame.loc;
    let (w, h) = (frame.size.w, frame.size.h);
    // Bords de l'écran (zone utile)
    let snap = |v: i32, target: i32, strength: i32| (v - target).abs() < strength;
    if screen > 0 {
        if snap(loc.x, zone.loc.x, screen) {
            loc.x = zone.loc.x;
        } else if snap(loc.x + w, zone.loc.x + zone.size.w, screen) {
            loc.x = zone.loc.x + zone.size.w - w;
        }
        if snap(loc.y, zone.loc.y, screen) {
            loc.y = zone.loc.y;
        } else if snap(loc.y + h, zone.loc.y + zone.size.h, screen) {
            loc.y = zone.loc.y + zone.size.h - h;
        }
    }
    // Bords des autres vues, s'ils se font face
    if window > 0 {
        for o in others {
            let vert = loc.y < o.loc.y + o.size.h && loc.y + h > o.loc.y;
            let horiz = loc.x < o.loc.x + o.size.w && loc.x + w > o.loc.x;
            if vert {
                if snap(loc.x, o.loc.x + o.size.w, window) {
                    loc.x = o.loc.x + o.size.w;
                } else if snap(loc.x + w, o.loc.x, window) {
                    loc.x = o.loc.x - w;
                }
            }
            if horiz {
                if snap(loc.y, o.loc.y + o.size.h, window) {
                    loc.y = o.loc.y + o.size.h;
                } else if snap(loc.y + h, o.loc.y, window) {
                    loc.y = o.loc.y - h;
                }
            }
        }
    }
    loc
}

/// Bureau visé par GoToDesktop et SendToDesktop
pub fn target_desktop(
    target: &DesktopTarget,
    current: usize,
    last: usize,
    count: usize,
    wrap: bool,
) -> usize {
    match target {
        DesktopTarget::Index(i) => (*i).min(count - 1),
        DesktopTarget::Current => current,
        DesktopTarget::Last => last.min(count - 1),
        DesktopTarget::Left => {
            if current > 0 {
                current - 1
            } else if wrap {
                count - 1
            } else {
                current
            }
        }
        DesktopTarget::Right => {
            if current + 1 < count {
                current + 1
            } else if wrap {
                0
            } else {
                current
            }
        }
    }
}

impl State {
    pub fn window_index(&self, window: &Window) -> Option<usize> {
        self.views.iter().position(|v| v.window == *window)
    }

    /// Vue qui a le clavier
    pub fn focused_view(&self) -> Option<usize> {
        self.focused_root().and_then(|s| self.view_index(&s))
    }

    /// Cadre d'une vue, coordonnées globales
    pub fn frame_rect(&self, i: usize) -> Rectangle<i32, Logical> {
        let layout = self.layout(&self.views[i]);
        Rectangle::new(self.views[i].loc - layout.client.loc, layout.size)
    }

    /// Zone utile de l'écran d'une vue (sans panneaux ni marges)
    pub fn view_zone(&self, i: usize) -> Rectangle<i32, Logical> {
        let frame = self.frame_rect(i);
        let center = frame.loc.to_f64() + frame.size.to_f64().to_point().downscale(2.0);
        self.output_at(center)
            .map(|o| self.usable_area(&o))
            .unwrap_or_default()
    }

    /// Cadres des autres vues affichées
    fn other_frames(&self, i: usize) -> Vec<Rectangle<i32, Logical>> {
        self.stacking()
            .into_iter()
            .filter(|&j| j != i)
            .map(|j| self.frame_rect(j))
            .collect()
    }

    /// Met une vue au premier plan de sa couche
    pub fn raise(&mut self, i: usize) -> usize {
        let view = self.views.remove(i);
        self.views.push(view);
        self.views.len() - 1
    }

    pub fn lower(&mut self, i: usize) -> usize {
        let view = self.views.remove(i);
        self.views.insert(0, view);
        0
    }

    /// Donne le clavier à une vue (et la met au premier plan si `raise`);
    /// une vue réduite ou d'un autre bureau est d'abord rendue visible
    pub fn focus_view(&mut self, i: usize, raise: bool) -> usize {
        self.show_desktop = false;
        let mut i = i;
        if self.views[i].minimized {
            self.views[i].minimized = false;
            let to = self.minimize_point(i);
            self.appear(i, crate::render::Appear::Restore(to));
        }
        if !self.views[i].on_desktop(self.desktop) {
            let d = self.views[i].desktop;
            self.switch_desktop(d);
        }
        if raise {
            i = self.raise(i);
        }
        let surface = self.views[i].surface().clone();
        self.set_keyboard_focus(Some(surface));
        i
    }

    /// Donne le clavier à la vue affichée la plus en avant
    pub fn focus_top(&mut self) {
        let top = self.stacking().last().map(|&i| self.views[i].surface().clone());
        self.set_keyboard_focus(top);
    }

    /// Envoie au client la taille et les états voulus
    fn configure(&mut self, i: usize, size: Option<Size<i32, Logical>>) {
        let tiling = self.views[i].tiling.clone();
        let toplevel = self.views[i].window.toplevel().unwrap().clone();
        toplevel.with_pending_state(|s| {
            use xdg_toplevel::State as S;
            for st in [
                S::Maximized,
                S::Fullscreen,
                S::TiledLeft,
                S::TiledRight,
                S::TiledTop,
                S::TiledBottom,
            ] {
                s.states.unset(st);
            }
            match &tiling {
                Tiling::None => {}
                Tiling::Fullscreen => {
                    s.states.set(S::Fullscreen);
                }
                Tiling::Maximized(Axis::Both) | Tiling::Edge(Edge::Up | Edge::Center) => {
                    s.states.set(S::Maximized);
                }
                Tiling::Maximized(_) | Tiling::Region(_) => {
                    for st in [S::TiledLeft, S::TiledRight, S::TiledTop, S::TiledBottom] {
                        s.states.set(st);
                    }
                }
                Tiling::Edge(e) => {
                    let states: &[S] = match e {
                        Edge::Left => &[S::TiledLeft, S::TiledTop, S::TiledBottom],
                        Edge::Right => &[S::TiledRight, S::TiledTop, S::TiledBottom],
                        _ => &[S::TiledLeft, S::TiledRight, S::TiledBottom],
                    };
                    for st in states {
                        s.states.set(*st);
                    }
                }
            }
            if let Some(size) = size {
                s.size = Some(size);
            }
        });
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    /// Place le cadre d'une vue dans un rectangle (le client prend la place
    /// restante)
    fn fit_frame(&mut self, i: usize, frame: Rectangle<i32, Logical>) {
        let layout = self.layout(&self.views[i]);
        let deco = layout.size - self.views[i].client_size();
        self.views[i].loc = frame.loc + layout.client.loc;
        let size = Size::from(((frame.size.w - deco.w).max(1), (frame.size.h - deco.h).max(1)));
        self.configure(i, Some(size));
    }

    /// Passe une vue à une géométrie imposée, en gardant l'ancienne
    fn set_tiling(&mut self, i: usize, tiling: Tiling, frame: Rectangle<i32, Logical>) {
        let current = self.views[i].tiling.clone();
        self.views[i].prev_tiling = match (&current, &tiling) {
            (Tiling::Edge(_) | Tiling::Region(_), Tiling::Maximized(_)) => Some(current.clone()),
            (_, Tiling::Maximized(_)) => self.views[i].prev_tiling.take(),
            _ => None,
        };
        if self.views[i].tiling == Tiling::None {
            let geo = self.views[i].window.geometry();
            self.views[i].saved = Some(Rectangle::new(self.views[i].loc, geo.size));
        }
        self.views[i].shaded = false;
        self.views[i].tiling = tiling;
        self.fit_frame(i, frame);
    }

    /// Rend à une vue sa géométrie libre
    pub fn restore(&mut self, i: usize) {
        if self.views[i].tiling == Tiling::None {
            return;
        }
        // Maximisée depuis un collage: elle y retourne
        if matches!(self.views[i].tiling, Tiling::Maximized(_) | Tiling::Fullscreen) {
            match self.views[i].prev_tiling.take() {
                Some(Tiling::Edge(e)) => {
                    self.views[i].tiling = Tiling::None;
                    let saved = self.views[i].saved;
                    self.snap_to_edge(i, e);
                    self.views[i].saved = saved;
                    return;
                }
                Some(Tiling::Region(r)) => {
                    self.views[i].tiling = Tiling::None;
                    let saved = self.views[i].saved;
                    self.snap_to_region(i, &r);
                    self.views[i].saved = saved;
                    return;
                }
                _ => {}
            }
        }
        self.views[i].prev_tiling = None;
        self.views[i].tiling = Tiling::None;
        match self.views[i].saved.take() {
            Some(saved) => {
                self.views[i].loc = saved.loc;
                self.configure(i, Some(saved.size));
            }
            None => self.configure(i, None),
        }
    }

    pub fn maximize(&mut self, i: usize, axis: Axis) {
        let zone = inset(self.view_zone(i), self.config.gap);
        let current = self.frame_rect(i);
        let frame = match axis {
            Axis::Both => zone,
            Axis::Horizontal => Rectangle::new(
                (zone.loc.x, current.loc.y).into(),
                (zone.size.w, current.size.h).into(),
            ),
            Axis::Vertical => Rectangle::new(
                (current.loc.x, zone.loc.y).into(),
                (current.size.w, zone.size.h).into(),
            ),
        };
        self.set_tiling(i, Tiling::Maximized(axis), frame);
    }

    pub fn toggle_maximize(&mut self, i: usize, axis: Axis) {
        if matches!(self.views[i].tiling, Tiling::Maximized(a) if a == axis) {
            self.restore(i);
        } else {
            self.maximize(i, axis);
        }
    }

    pub fn toggle_fullscreen(&mut self, i: usize) {
        if self.views[i].tiling == Tiling::Fullscreen {
            self.restore(i);
            return;
        }
        let frame = self.frame_rect(i);
        let center = frame.loc.to_f64() + frame.size.to_f64().to_point().downscale(2.0);
        let Some(output) = self.output_at(center) else {
            return;
        };
        let rect = self.output_geometry(&output);
        if self.views[i].tiling == Tiling::None {
            let geo = self.views[i].window.geometry();
            self.views[i].saved = Some(Rectangle::new(self.views[i].loc, geo.size));
        }
        self.views[i].prev_tiling = match &self.views[i].tiling {
            t @ (Tiling::Edge(_) | Tiling::Region(_)) => Some(t.clone()),
            _ => None,
        };
        self.views[i].shaded = false;
        self.views[i].tiling = Tiling::Fullscreen;
        self.views[i].loc = rect.loc;
        self.configure(i, Some(rect.size));
        self.raise(i);
    }

    pub fn snap_to_edge(&mut self, i: usize, edge: Edge) {
        let zone = self.view_zone(i);
        let frame = edge_rect(zone, edge, self.config.gap);
        self.set_tiling(i, Tiling::Edge(edge), frame);
    }

    pub fn snap_to_region(&mut self, i: usize, name: &str) {
        let Some(r) = self.config.region(name).cloned() else {
            warn!("région inconnue: {name}");
            return;
        };
        let zone = self.view_zone(i);
        let frame = inset(percent_rect(zone, r.x, r.y, r.width, r.height), self.config.gap);
        self.set_tiling(i, Tiling::Region(name.to_string()), frame);
    }

    /// Place une vue selon une politique (§ 8.2)
    pub fn place(&mut self, i: usize, policy: Placement) {
        let zone = self.view_zone(i);
        let frame = self.frame_rect(i);
        let loc = match policy {
            Placement::Center => Point::from((
                zone.loc.x + (zone.size.w - frame.size.w).max(0) / 2,
                zone.loc.y + (zone.size.h - frame.size.h).max(0) / 2,
            )),
            Placement::Cursor => {
                let p = self.pointer.to_i32_round::<i32>();
                let x = (p.x - frame.size.w / 2)
                    .clamp(zone.loc.x, zone.loc.x + (zone.size.w - frame.size.w).max(0));
                let y = (p.y - frame.size.h / 2)
                    .clamp(zone.loc.y, zone.loc.y + (zone.size.h - frame.size.h).max(0));
                Point::from((x, y))
            }
            Placement::Automatic => least_overlap(zone, frame.size, &self.other_frames(i)),
        };
        let layout = self.layout(&self.views[i]);
        self.views[i].loc = loc + layout.client.loc;
    }

    /// Zone utile de l'écran sous le pointeur, pour une vue pas encore placée
    pub fn place_new(&mut self, i: usize) {
        let Some(output) = self.output_at(self.pointer) else {
            return;
        };
        // Position provisoire sur le bon écran avant le calcul
        self.views[i].loc = self.usable_area(&output).loc;
        let policy = self.config.placement;
        self.place(i, policy);
    }

    pub fn iconify(&mut self, i: usize) {
        if self.views[i].minimized {
            return;
        }
        let to = self.minimize_point(i);
        self.ghost_view(i, crate::render::GhostKind::Minimize(to));
        self.views[i].minimized = true;
        if self.focused_view() == Some(i) {
            self.focus_top();
        }
    }

    pub fn switch_desktop(&mut self, d: usize) {
        if d == self.desktop || d >= self.config.desktop_names.len() {
            return;
        }
        self.last_desktop = self.desktop;
        self.desktop = d;
        self.show_desktop = false;
        info!("bureau {}", self.config.desktop_names[d]);
        let keep = self.focused_view().filter(|&i| self.views[i].omnipresent);
        if keep.is_none() {
            // La vue la plus récemment active du nouveau bureau
            let next = self
                .mru
                .iter()
                .filter_map(|w| self.window_index(w))
                .find(|&i| self.visible(&self.views[i]));
            match next {
                Some(i) => {
                    let s = self.views[i].surface().clone();
                    self.set_keyboard_focus(Some(s));
                }
                None => self.focus_top(),
            }
        }
    }

    /// Alt+Tab (§ 8.3, sans liste à l'écran pour l'instant): vue suivante
    /// dans l'ordre d'activité, aperçue au premier plan; le choix est
    /// validé quand les modificateurs sont relâchés
    pub fn cycle_window(&mut self, forward: bool) {
        if self.cycle.is_none() {
            let mut windows: Vec<Window> = self
                .mru
                .iter()
                .filter(|w| {
                    self.window_index(w).is_some_and(|i| {
                        let v = &self.views[i];
                        v.mapped && v.on_desktop(self.desktop)
                    })
                })
                .cloned()
                .collect();
            for v in &self.views {
                if v.mapped && v.on_desktop(self.desktop) && !windows.contains(&v.window) {
                    windows.push(v.window.clone());
                }
            }
            if windows.is_empty() {
                return;
            }
            self.cycle = Some(Cycle { windows, index: 0 });
        }
        let cycle = self.cycle.as_mut().unwrap();
        let n = cycle.windows.len();
        cycle.index = if forward {
            (cycle.index + 1) % n
        } else {
            (cycle.index + n - 1) % n
        };
        let w = cycle.windows[cycle.index].clone();
        if let Some(i) = self.window_index(&w) {
            self.focus_view(i, true);
        }
    }

    /// Fin d'Alt+Tab: la vue choisie devient la plus récente
    pub fn end_cycle(&mut self) {
        if let Some(c) = self.cycle.take() {
            let w = c.windows[c.index].clone();
            self.mru.retain(|x| *x != w);
            self.mru.insert(0, w);
        }
    }

    /// Termine le processus d'un client: SIGTERM, puis SIGKILL après 2 s
    fn kill(&mut self, i: usize) {
        let surface = self.views[i].surface().clone();
        let Some(pid) = self
            .dh
            .get_client(surface.id())
            .ok()
            .and_then(|c| c.get_credentials(&self.dh).ok())
            .map(|c| c.pid)
        else {
            return;
        };
        if pid <= 1 || pid as u32 == std::process::id() {
            return;
        }
        info!("arrêt du processus {pid}");
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let timer =
            smithay::reexports::calloop::timer::Timer::from_duration(std::time::Duration::from_secs(2));
        let _ = self.handle.insert_source(timer, move |_, _, _| {
            // Sûr: simple appel système; sans effet si le processus est parti
            unsafe {
                if libc::kill(pid, 0) == 0 {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
            smithay::reexports::calloop::timer::TimeoutAction::Drop
        });
    }

    /// Exécute des actions sur une vue (celle sous le pointeur pour la
    /// souris, celle qui a le clavier sinon)
    pub fn run_actions(&mut self, actions: &[Action], target: Option<Window>, origin: Origin) {
        for action in actions {
            let i = target.as_ref().and_then(|w| self.window_index(w));
            self.run_action(action, i, origin);
        }
    }

    fn run_action(&mut self, action: &Action, i: Option<usize>, origin: Origin) {
        use Action::*;
        match action {
            Execute(cmd) => self.spawn(cmd),
            ShowMenu { menu, at_cursor } => self.show_menu(menu, *at_cursor, i),
            Reconfigure => self.reconfigure(),
            Exit => self.exit(),
            GoToDesktop { to, wrap } => {
                let n = self.config.desktop_names.len();
                let d = target_desktop(to, self.desktop, self.last_desktop, n, *wrap);
                self.switch_desktop(d);
            }
            NextWindow => self.cycle_window(true),
            PreviousWindow => self.cycle_window(false),
            ToggleShowDesktop => {
                self.show_desktop = !self.show_desktop;
                if self.show_desktop {
                    self.set_keyboard_focus(Option::None);
                } else {
                    self.focus_top();
                }
            }
            Unfocus => self.set_keyboard_focus(Option::None),
            None => {}
            _ => {
                let Some(i) = i else { return };
                self.run_view_action(action, i, origin);
            }
        }
    }

    fn run_view_action(&mut self, action: &Action, i: usize, origin: Origin) {
        use Action::*;
        match action {
            Focus => {
                if self.focused_view() != Some(i) {
                    let raise = self.config.raise_on_focus;
                    self.focus_view(i, raise);
                }
            }
            Raise => {
                self.raise(i);
            }
            Lower => {
                self.lower(i);
            }
            Close => self.views[i].window.toplevel().unwrap().send_close(),
            Kill => self.kill(i),
            Iconify => self.iconify(i),
            Maximize(axis) => self.maximize(i, *axis),
            UnMaximize(axis) => {
                if matches!(self.views[i].tiling, Tiling::Maximized(a) if a == *axis || *axis == Axis::Both) {
                    self.restore(i);
                }
            }
            ToggleMaximize(axis) => self.toggle_maximize(i, *axis),
            ToggleFullscreen => self.toggle_fullscreen(i),
            ToggleDecorations => {
                // Le cadre garde sa place: le client gagne ou perd la barre
                let frame = self.frame_rect(i);
                self.views[i].decorated = !self.views[i].decorated;
                self.fit_frame(i, frame);
            }
            Shade => self.views[i].shaded = self.views[i].tiling != Tiling::Fullscreen,
            Unshade => self.views[i].shaded = false,
            ToggleShade => {
                let s = !self.views[i].shaded;
                self.views[i].shaded = s && self.views[i].tiling != Tiling::Fullscreen;
            }
            ToggleAlwaysOnTop => {
                let v = &mut self.views[i];
                v.stack = if v.stack == Stack::Top {
                    Stack::Normal
                } else {
                    Stack::Top
                };
            }
            ToggleAlwaysOnBottom => {
                let v = &mut self.views[i];
                v.stack = if v.stack == Stack::Bottom {
                    Stack::Normal
                } else {
                    Stack::Bottom
                };
            }
            ToggleOmnipresent => {
                let d = self.desktop;
                let v = &mut self.views[i];
                v.omnipresent = !v.omnipresent;
                v.desktop = d;
            }
            Move => self.begin_move(i, origin),
            Resize => self.begin_resize(i, origin),
            MoveRelative { x, y } => {
                if self.views[i].tiling == Tiling::None {
                    self.views[i].loc += Point::from((*x, *y));
                }
            }
            ResizeRelative {
                left,
                right,
                top,
                bottom,
            } => {
                if self.views[i].tiling != Tiling::None {
                    return;
                }
                let size = self.views[i].window.geometry().size;
                let new = Size::from(((size.w + left + right).max(1), (size.h + top + bottom).max(1)));
                self.views[i].loc -= Point::from((*left, *top));
                self.configure(i, Some(new));
            }
            SnapToEdge(edge) => self.snap_to_edge(i, *edge),
            UnSnap => {
                if matches!(self.views[i].tiling, Tiling::Edge(_) | Tiling::Region(_)) {
                    self.restore(i);
                }
            }
            SnapToRegion(name) => self.snap_to_region(i, name),
            SendToDesktop { to, follow, wrap } => {
                let n = self.config.desktop_names.len();
                let d = target_desktop(to, self.views[i].desktop, self.last_desktop, n, *wrap);
                self.views[i].desktop = d;
                self.views[i].omnipresent = false;
                if *follow {
                    self.switch_desktop(d);
                    self.focus_view(i, true);
                } else if !self.views[i].on_desktop(self.desktop) {
                    self.focus_top();
                }
            }
            AutoPlace(policy) => {
                let policy = policy.unwrap_or(self.config.placement);
                if self.views[i].tiling == Tiling::None {
                    self.place(i, policy);
                }
            }
            _ => {}
        }
    }

    /// Déplacement interactif: depuis la souris (glisser) ou le clavier
    /// (jusqu'au prochain clic)
    fn begin_move(&mut self, i: usize, origin: Origin) {
        if self.views[i].tiling == Tiling::Fullscreen {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let start = GrabStartData {
            focus: None,
            button: origin.button.unwrap_or(BTN_LEFT),
            location: origin.location,
        };
        let grab = MoveGrab {
            start,
            window: self.views[i].window.clone(),
            initial_loc: self.views[i].loc,
            keyboard: origin.button.is_none(),
            snap: None,
        };
        pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }

    /// Redimensionnement interactif; les bords suivent le quart du cadre où
    /// se trouve le pointeur
    fn begin_resize(&mut self, i: usize, origin: Origin) {
        if self.views[i].tiling != Tiling::None {
            return;
        }
        let frame = self.frame_rect(i).to_f64();
        let p = origin.location;
        let edges = Edges {
            left: p.x < frame.loc.x + frame.size.w / 2.0,
            right: p.x >= frame.loc.x + frame.size.w / 2.0,
            top: p.y < frame.loc.y + frame.size.h / 2.0,
            bottom: p.y >= frame.loc.y + frame.size.h / 2.0,
        };
        self.start_resize_edges(i, edges, origin);
    }

    pub fn start_resize_edges(&mut self, i: usize, edges: Edges, origin: Origin) {
        if self.views[i].tiling != Tiling::None || self.views[i].shaded {
            return;
        }
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        let view = &mut self.views[i];
        let size = view.window.geometry().size;
        view.resize = Some(crate::view::Resize {
            edges,
            initial_loc: view.loc,
            initial_size: size,
            ending: false,
        });
        let toplevel = view.window.toplevel().unwrap().clone();
        toplevel.with_pending_state(|s| s.states.set(xdg_toplevel::State::Resizing));
        toplevel.send_pending_configure();
        let start = GrabStartData {
            focus: None,
            button: origin.button.unwrap_or(BTN_RIGHT),
            location: origin.location,
        };
        let grab = ResizeGrab {
            start,
            window: view.window.clone(),
            edges,
            initial_size: size,
            keyboard: origin.button.is_none(),
        };
        pointer.set_grab(self, grab, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }

    /// Règles de fenêtre à la première apparition d'une vue (§ 8.5)
    pub fn apply_rules(&mut self, i: usize) {
        let app_id = self.views[i].app_id();
        let title = self.views[i].title();
        let rules: Vec<_> = self
            .config
            .window_rules
            .iter()
            .filter(|r| r.matches(&app_id, &title))
            .cloned()
            .collect();
        let window = self.views[i].window.clone();
        for rule in rules {
            debug!("règle de fenêtre pour « {title} » ({app_id})");
            if let Some(d) = rule.server_decoration
                && let Some(i) = self.window_index(&window)
            {
                self.views[i].decorated = d;
            }
            let origin = Origin {
                button: None,
                location: self.pointer,
            };
            self.run_actions(&rule.actions, Some(window.clone()), origin);
        }
    }

    /// Recharge la configuration (§ 5.7); une erreur garde l'ancienne
    pub fn reconfigure(&mut self) {
        let dir = self.config.dir.clone();
        match Config::load(&dir) {
            Ok(config) => {
                info!("configuration rechargée");
                self.config = config;
            }
            Err(e) => {
                warn!("rechargement impossible, configuration gardée: {e}");
                return;
            }
        }
        if self.no_animations {
            self.config.animations.enabled = false;
        }
        self.theme = Theme::load(&self.config);
        self.close_menus();
        self.reload_menus();
        for v in &mut self.views {
            v.deco = None;
        }
        let n = self.config.desktop_names.len();
        if self.desktop >= n {
            self.switch_desktop(n - 1);
        }
        for v in &mut self.views {
            v.desktop = v.desktop.min(n - 1);
        }
        self.apply_session_env();
        self.apply_keyboard();
        // Les vues imposées suivent les nouvelles marges et le nouveau cadre
        for i in 0..self.views.len() {
            match self.views[i].tiling.clone() {
                Tiling::Maximized(a) => self.maximize(i, a),
                Tiling::Edge(e) => self.snap_to_edge(i, e),
                Tiling::Region(r) => self.snap_to_region(i, &r),
                _ => {}
            }
        }
    }

    /// Quitte la session: script `shutdown` (5 s au plus), puis arrêt
    pub fn exit(&mut self) {
        let script = self.config.dir.join("shutdown");
        if script.is_file() {
            info!("script shutdown");
            let mut cmd = std::process::Command::new("sh");
            cmd.arg(&script);
            for (k, v) in &self.child_env {
                cmd.env(k, v);
            }
            if let Ok(mut child) = cmd.spawn() {
                let start = std::time::Instant::now();
                while start.elapsed() < std::time::Duration::from_secs(5) {
                    if let Ok(Some(_)) = child.try_wait() {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
        self.signal.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn placement() {
        let zone = rect(0, 0, 1000, 800);
        // Seule: centrée
        assert_eq!(
            least_overlap(zone, (400, 300).into(), &[]),
            Point::from((300, 250))
        );
        // Une vue en haut à gauche: la nouvelle va à côté, sans recouvrement
        let p = least_overlap(zone, (400, 300).into(), &[rect(0, 0, 500, 400)]);
        assert_eq!(
            overlap(Rectangle::new(p, (400, 300).into()), rect(0, 0, 500, 400)),
            0
        );
        assert!(zone.contains_rect(Rectangle::new(p, (400, 300).into())));
    }

    #[test]
    fn edges() {
        let zone = rect(0, 38, 1280, 724);
        assert_eq!(edge_rect(zone, Edge::Left, 0), rect(0, 38, 640, 724));
        assert_eq!(edge_rect(zone, Edge::Right, 0), rect(640, 38, 640, 724));
        assert_eq!(edge_rect(zone, Edge::Up, 10), rect(10, 48, 1260, 704));
        assert_eq!(snap_edge(zone, (3.0, 300.0).into(), 10, true), Some(Edge::Left));
        assert_eq!(snap_edge(zone, (600.0, 40.0).into(), 10, true), Some(Edge::Up));
        assert_eq!(snap_edge(zone, (600.0, 40.0).into(), 10, false), None);
        assert_eq!(snap_edge(zone, (600.0, 400.0).into(), 10, true), None);
        assert_eq!(
            percent_rect(zone, 50.0, 0.0, 50.0, 100.0),
            rect(640, 38, 640, 724)
        );
    }

    #[test]
    fn resistance() {
        let zone = rect(0, 0, 1000, 800);
        // 12 px du bord gauche: collé au bord
        assert_eq!(
            resist(rect(12, 100, 300, 200), zone, 20, &[], 0),
            Point::from((0, 100))
        );
        // Au-delà de la force: libre
        assert_eq!(
            resist(rect(40, 100, 300, 200), zone, 20, &[], 0),
            Point::from((40, 100))
        );
        // Bord d'une autre vue en face
        let other = rect(500, 100, 200, 200);
        assert_eq!(
            resist(rect(190, 150, 300, 100), zone, 20, &[other], 20),
            Point::from((200, 150))
        );
    }

    #[test]
    fn desktops() {
        use DesktopTarget::*;
        assert_eq!(target_desktop(&Right, 3, 0, 4, true), 0);
        assert_eq!(target_desktop(&Right, 3, 0, 4, false), 3);
        assert_eq!(target_desktop(&Left, 0, 0, 4, true), 3);
        assert_eq!(target_desktop(&Index(9), 0, 0, 4, true), 3);
        assert_eq!(target_desktop(&Last, 2, 1, 4, true), 1);
    }
}
