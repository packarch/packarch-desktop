// SPDX-License-Identifier: MIT OR Apache-2.0
//! Loupe (cahier des charges § 7.2, priorité 3): une partie de l'écran
//! agrandie autour du pointeur, dans un rectangle qui le suit, ou sur tout
//! l'écran. `ZoomIn`, `ZoomOut` et `ToggleMagnify` la règlent; la section
//! `<magnifier>` de `rc.xml` donne sa taille et ses pas.
//!
//! Le point sous le pointeur reste sous le pointeur: on agit sur ce qu'on
//! voit agrandi.

use smithay::utils::{Logical, Point, Rectangle, Size};
use tracing::info;

use crate::state::State;

/// Facteur au-delà duquel la loupe ne grossit plus
pub const MAX_ZOOM: f64 = 16.0;

/// Réglages de `<magnifier>`
#[derive(Debug, Clone, PartialEq)]
pub struct MagnifierConfig {
    /// Taille du rectangle; 0 ou moins: tout l'écran
    pub width: i32,
    pub height: i32,
    /// Facteur à l'ouverture
    pub init_scale: f64,
    /// Pas de `ZoomIn`/`ZoomOut`
    pub increment: f64,
    /// Agrandissement lissé (sinon pixels nets)
    pub use_filter: bool,
}

impl Default for MagnifierConfig {
    fn default() -> Self {
        MagnifierConfig {
            width: 400,
            height: 400,
            init_scale: 2.0,
            increment: 0.5,
            use_filter: true,
        }
    }
}

/// Facteur de la loupe après une action; `None`: fermée
pub fn zoom_step(current: Option<f64>, cfg: &MagnifierConfig, action: ZoomAction) -> Option<f64> {
    let init = cfg.init_scale.clamp(1.1, MAX_ZOOM);
    let step = cfg.increment.max(0.05);
    match (action, current) {
        (ZoomAction::Toggle, Some(_)) => None,
        (ZoomAction::Toggle | ZoomAction::In, None) => Some(init),
        (ZoomAction::In, Some(z)) => Some((z + step).min(MAX_ZOOM)),
        (ZoomAction::Out, Some(z)) => Some(z - step).filter(|z| *z > 1.0 + 1e-9),
        (ZoomAction::Out, None) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomAction {
    In,
    Out,
    Toggle,
}

/// Où dessiner et quoi agrandir sur un écran de taille `output` (unités
/// logiques, coordonnées de l'écran), pour un pointeur en `p`: rectangle
/// de la loupe et rectangle source, `zoom` fois plus petit. Le point sous
/// le pointeur est le même dans les deux
pub fn magnifier_rects(
    output: Size<i32, Logical>,
    p: Point<f64, Logical>,
    cfg: &MagnifierConfig,
    zoom: f64,
) -> (Rectangle<f64, Logical>, Rectangle<f64, Logical>) {
    let full = cfg.width <= 0 || cfg.height <= 0;
    let dst = if full {
        Rectangle::from_size(output.to_f64())
    } else {
        let size = Size::from((cfg.width as f64, cfg.height as f64));
        Rectangle::new(p - size.to_point().downscale(2.0), size)
    };
    // Source: même position relative du pointeur, `zoom` fois plus petite
    let size = dst.size.downscale(zoom);
    let rel = p - dst.loc;
    let src = Rectangle::new(p - rel.downscale(zoom), size);
    (dst, src)
}

impl State {
    pub fn zoom(&mut self, action: ZoomAction) {
        let next = zoom_step(self.magnifier, &self.config.magnifier, action);
        if next != self.magnifier {
            match next {
                Some(z) => info!("loupe: ×{z}"),
                None => info!("loupe fermée"),
            }
        }
        self.magnifier = next;
        self.request_redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps() {
        let c = MagnifierConfig::default();
        assert_eq!(zoom_step(None, &c, ZoomAction::In), Some(2.0));
        assert_eq!(zoom_step(Some(2.0), &c, ZoomAction::In), Some(2.5));
        assert_eq!(zoom_step(Some(2.0), &c, ZoomAction::Out), Some(1.5));
        assert_eq!(zoom_step(Some(1.5), &c, ZoomAction::Out), None);
        assert_eq!(zoom_step(None, &c, ZoomAction::Out), None);
        assert_eq!(zoom_step(Some(3.0), &c, ZoomAction::Toggle), None);
        assert_eq!(zoom_step(None, &c, ZoomAction::Toggle), Some(2.0));
        assert_eq!(zoom_step(Some(15.8), &c, ZoomAction::In), Some(MAX_ZOOM));
    }

    #[test]
    fn rects() {
        let out = Size::from((1000, 800));
        let c = MagnifierConfig::default();
        // Rectangle de 400 centré sur le pointeur, source de 200 autour
        let (dst, src) = magnifier_rects(out, (500.0, 400.0).into(), &c, 2.0);
        assert_eq!(dst, Rectangle::new((300.0, 200.0).into(), (400.0, 400.0).into()));
        assert_eq!(src, Rectangle::new((400.0, 300.0).into(), (200.0, 200.0).into()));
        // Tout l'écran: le point sous le pointeur ne bouge pas
        let full = MagnifierConfig { width: 0, ..c };
        let (dst, src) = magnifier_rects(out, (1000.0, 0.0).into(), &full, 2.0);
        assert_eq!(dst, Rectangle::from_size((1000.0, 800.0).into()));
        assert_eq!(src, Rectangle::new((500.0, 0.0).into(), (500.0, 400.0).into()));
    }
}
