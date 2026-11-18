// SPDX-License-Identifier: MIT OR Apache-2.0
//! Vue: une fenêtre de premier niveau et son état dans le bureau (§ 3, § 8)

use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::anim::Anim;
use crate::config::Button;
use crate::deco::{Edges, Frame};
use crate::render::{Appear, DecoCache, Snapshot, WindowTexture};

/// Redimensionnement en cours: la position suit les bords de gauche et du
/// haut quand la taille change
#[derive(Debug, Clone, Copy)]
pub struct Resize {
    pub edges: Edges,
    pub initial_loc: Point<i32, Logical>,
    pub initial_size: Size<i32, Logical>,
    /// Dernière configuration envoyée pendant le geste
    pub last_serial: Option<smithay::utils::Serial>,
    /// Le geste est fini; on attend que le client ait appliqué la
    /// configuration de ce numéro (`None`: pas fini)
    pub ending: Option<Option<smithay::utils::Serial>>,
}

/// Direction d'une maximisation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Both,
    Horizontal,
    Vertical,
}

/// Bord d'écran d'un collage
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Up,
    Down,
    Center,
}

/// Quart de la zone utile (collage combiné de deux bords)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Corner {
    pub up: bool,
    pub left: bool,
}

/// Collage: moitié (ou zone entière) le long d'un bord, ou quart
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    Edge(Edge),
    Corner(Corner),
}

impl Snap {
    /// Collage combiné (`SnapToEdge combine="yes"`, à la labwc): la
    /// nouvelle direction remplace celle du même axe et garde l'autre;
    /// gauche puis haut donne le quart en haut à gauche
    pub fn combine(current: &Tiling, dir: Edge) -> Snap {
        // Composantes (verticale, horizontale) du collage en cours
        let (v, h) = match current {
            Tiling::Edge(Edge::Up) => (Some(true), None),
            Tiling::Edge(Edge::Down) => (Some(false), None),
            Tiling::Edge(Edge::Left) => (None, Some(true)),
            Tiling::Edge(Edge::Right) => (None, Some(false)),
            Tiling::Corner(c) => (Some(c.up), Some(c.left)),
            _ => (None, None),
        };
        let corner = match dir {
            Edge::Up | Edge::Down => h.map(|left| Corner {
                up: dir == Edge::Up,
                left,
            }),
            Edge::Left | Edge::Right => v.map(|up| Corner {
                up,
                left: dir == Edge::Left,
            }),
            Edge::Center => None,
        };
        corner.map_or(Snap::Edge(dir), Snap::Corner)
    }

    pub fn tiling(self) -> Tiling {
        match self {
            Snap::Edge(e) => Tiling::Edge(e),
            Snap::Corner(c) => Tiling::Corner(c),
        }
    }
}

/// Géométrie imposée par le compositeur
#[derive(Debug, Clone, PartialEq)]
pub enum Tiling {
    None,
    Maximized(Axis),
    Edge(Edge),
    Corner(Corner),
    Region(String),
    Fullscreen,
}

impl Tiling {
    /// Toute la zone utile: maximisée dans les deux sens, ou collée en haut
    /// (glissée en haut de l'écran, topMaximize) ou au centre, qui ont la
    /// même géométrie et que le client voit maximisées
    pub fn fills_zone(&self) -> bool {
        matches!(
            self,
            Tiling::Maximized(Axis::Both) | Tiling::Edge(Edge::Up | Edge::Center)
        )
    }

    /// Maximisée, dans un sens au moins
    pub fn is_maximized(&self) -> bool {
        matches!(self, Tiling::Maximized(_)) || self.fills_zone()
    }

    /// Maximisée dans ce sens (`Both`: dans les deux)
    pub fn is_maximized_in(&self, axis: Axis) -> bool {
        match axis {
            Axis::Both => self.fills_zone(),
            a => *self == Tiling::Maximized(a),
        }
    }
}

/// Couche d'empilement (§ 8.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stack {
    Bottom,
    Normal,
    Top,
}

/// Prochain identifiant de vue
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub struct View {
    /// Identifiant stable (protocoles de barre des tâches)
    pub id: u64,
    pub window: Window,
    /// Coin haut gauche de la géométrie du client, coordonnées globales
    pub loc: Point<i32, Logical>,
    pub mapped: bool,
    pub minimized: bool,
    pub tiling: Tiling,
    /// Géométrie d'avant la maximisation ou le collage
    pub saved: Option<Rectangle<i32, Logical>>,
    /// Collage d'avant une maximisation ou un plein écran, rétabli ensuite
    pub prev_tiling: Option<Tiling>,
    pub shaded: bool,
    /// Cadre affiché (ToggleDecorations, règle serverDecoration)
    pub decorated: bool,
    pub stack: Stack,
    /// Bureau (à partir de 0)
    pub desktop: usize,
    pub omnipresent: bool,
    pub resize: Option<Resize>,
    pub hovered: Option<Button>,
    pub pressed: Option<Button>,
    pub offscreen: Option<WindowTexture>,
    pub deco: Option<DecoCache>,
    /// Dernière image affichée (animations de fermeture et de réduction)
    pub last: Option<Snapshot>,
    /// Animation d'apparition en cours
    pub appear: Option<(Anim, Appear)>,
    /// Activation demandée sans jeton valide (§ 8.1)
    pub urgent: bool,
    /// Surface Wayland de la vue (celle d'une fenêtre X11 est connue après
    /// son association par xwayland-shell)
    surface: WlSurface,
    /// Dernière taille demandée au client
    pub pending_size: Option<Size<i32, Logical>>,
    /// Animation du rectangle (maximisation, collage): de, vers
    pub geom_anim: Option<(Anim, Rectangle<i32, Logical>, Rectangle<i32, Logical>)>,
    /// Enroulement en cours: animation, sens (vrai: s'enroule), dernière
    /// image déroulée
    pub shade_anim: Option<(Anim, bool, Option<Snapshot>)>,
    /// Règles de fenêtre (§ 8.5)
    pub skip_taskbar: bool,
    pub skip_switcher: bool,
    pub fixed_position: bool,
}

impl View {
    pub fn new(window: Window, surface: WlSurface, desktop: usize) -> View {
        View {
            id: NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            surface,
            pending_size: None,
            shade_anim: None,
            geom_anim: None,
            skip_taskbar: false,
            skip_switcher: false,
            fixed_position: false,
            window,
            loc: Point::from((0, 0)),
            mapped: false,
            minimized: false,
            tiling: Tiling::None,
            saved: None,
            prev_tiling: None,
            shaded: false,
            decorated: true,
            stack: Stack::Normal,
            desktop,
            omnipresent: false,
            resize: None,
            hovered: None,
            pressed: None,
            offscreen: None,
            deco: None,
            last: None,
            appear: None,
            urgent: false,
        }
    }

    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }

    /// Fenêtre X11 (XWayland)
    pub fn x11(&self) -> Option<&smithay::xwayland::X11Surface> {
        self.window.x11_surface()
    }

    /// Demande la fermeture (xdg_toplevel.close, WM_DELETE_WINDOW)
    pub fn close(&self) {
        if let Some(t) = self.window.toplevel() {
            t.send_close();
        } else if let Some(x) = self.x11() {
            let _ = x.close();
        }
    }

    fn toplevel_data<T>(
        &self,
        f: impl FnOnce(&smithay::wayland::shell::xdg::XdgToplevelSurfaceRoleAttributes) -> T,
    ) -> T {
        with_states(self.surface(), |states| {
            let data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            f(&data)
        })
    }

    pub fn title(&self) -> String {
        match self.x11() {
            Some(x) => x.title(),
            None => self.toplevel_data(|d| d.title.clone().unwrap_or_default()),
        }
    }

    /// Identifiant: app_id Wayland, ou classe X11 (§ 8.5)
    pub fn app_id(&self) -> String {
        match self.x11() {
            Some(x) => x.class(),
            None => self.toplevel_data(|d| d.app_id.clone().unwrap_or_default()),
        }
    }

    /// Taille du client (zéro en hauteur quand la vue est enroulée)
    pub fn client_size(&self) -> Size<i32, Logical> {
        let s = self.window.geometry().size;
        if self.shaded { Size::from((s.w, 0)) } else { s }
    }

    /// Parties du cadre affichées
    pub fn frame(&self, keep_border: bool) -> Frame {
        if self.tiling == Tiling::Fullscreen {
            Frame {
                title: false,
                border: false,
            }
        } else if self.decorated {
            Frame {
                title: true,
                border: true,
            }
        } else {
            Frame {
                title: false,
                border: keep_border,
            }
        }
    }

    /// Coins arrondis et ombre seulement pour une vue libre
    pub fn is_free(&self) -> bool {
        self.tiling == Tiling::None
    }

    pub fn on_desktop(&self, desktop: usize) -> bool {
        self.omnipresent || self.desktop == desktop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiling_maximized() {
        // Glissée en haut de l'écran: maximisée pour les boutons et le client
        for t in [
            Tiling::Maximized(Axis::Both),
            Tiling::Edge(Edge::Up),
            Tiling::Edge(Edge::Center),
        ] {
            assert!(
                t.fills_zone() && t.is_maximized() && t.is_maximized_in(Axis::Both),
                "{t:?}"
            );
        }
        let h = Tiling::Maximized(Axis::Horizontal);
        assert!(!h.fills_zone() && h.is_maximized());
        assert!(h.is_maximized_in(Axis::Horizontal) && !h.is_maximized_in(Axis::Vertical));
        for t in [Tiling::None, Tiling::Edge(Edge::Left), Tiling::Fullscreen] {
            assert!(!t.is_maximized(), "{t:?}");
        }
    }
}
