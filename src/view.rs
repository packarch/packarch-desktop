// SPDX-License-Identifier: MIT OR Apache-2.0
//! Vue: une fenêtre de premier niveau et son état dans le bureau (§ 3, § 8)

use smithay::desktop::Window;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

use crate::config::Button;
use crate::deco::Edges;
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

pub struct View {
    pub window: Window,
    /// Coin haut gauche de la géométrie du client, coordonnées globales
    pub loc: Point<i32, Logical>,
    pub mapped: bool,
    pub minimized: bool,
    /// Géométrie d'avant la maximisation
    pub maximized: Option<Rectangle<i32, Logical>>,
    pub resize: Option<Resize>,
    pub hovered: Option<Button>,
    pub pressed: Option<Button>,
    pub offscreen: Option<WindowTexture>,
    pub deco: Option<DecoCache>,
}

impl View {
    pub fn new(window: Window) -> View {
        View {
            window,
            loc: Point::from((0, 0)),
            mapped: false,
            minimized: false,
            maximized: None,
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

    pub fn title(&self) -> String {
        with_states(self.surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().unwrap().title.clone())
                .unwrap_or_default()
        })
    }
}
