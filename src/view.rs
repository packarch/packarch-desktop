// SPDX-License-Identifier: MIT OR Apache-2.0
//! Vue: une fenêtre de premier niveau et son état dans le bureau (§ 3, § 8)

use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::config::Button;
use crate::deco::{Edges, Frame};
use crate::render::{DecoCache, WindowTexture};

/// Redimensionnement en cours: la position suit les bords de gauche et du
/// haut quand la taille change
#[derive(Debug, Clone, Copy)]
pub struct Resize {
    pub edges: Edges,
    pub initial_loc: Point<i32, Logical>,
    pub initial_size: Size<i32, Logical>,
    /// Le geste est fini; on attend le dernier commit du client
    pub ending: bool,
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

/// Géométrie imposée par le compositeur
#[derive(Debug, Clone, PartialEq)]
pub enum Tiling {
    None,
    Maximized(Axis),
    Edge(Edge),
    Region(String),
    Fullscreen,
}

/// Couche d'empilement (§ 8.1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stack {
    Bottom,
    Normal,
    Top,
}

pub struct View {
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
}

impl View {
    pub fn new(window: Window, desktop: usize) -> View {
        View {
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
        }
    }

    pub fn surface(&self) -> &WlSurface {
        self.window.toplevel().unwrap().wl_surface()
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
        self.toplevel_data(|d| d.title.clone().unwrap_or_default())
    }

    pub fn app_id(&self) -> String {
        self.toplevel_data(|d| d.app_id.clone().unwrap_or_default())
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
