// SPDX-License-Identifier: MIT OR Apache-2.0
//! Logo pacman au centre du fond (option `<core><logo>`): dessiné
//! sous les couches de fond, donc caché dès qu'un programme pose un fond
//! d'écran (swaybg…). Le SVG est intégré au programme et rendu à la taille
//! exacte de chaque écran.

use std::collections::HashMap;

use resvg::tiny_skia::{self, Pixmap};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size, Transform};

/// Pacman et trois pastilles, d'une teinte à peine plus sombre que le fond
const SVG: &[u8] = include_bytes!("../data/logo.svg");

/// Hauteur du logo, en fraction de la hauteur de l'écran
const HEIGHT: f64 = 0.2;

/// Images du logo déjà rendues, par hauteur en pixels physiques
#[derive(Default)]
pub struct Logo {
    images: HashMap<u32, Option<(MemoryRenderBuffer, Size<i32, Logical>)>>,
}

/// Rend le SVG à `height` pixels de haut (couleurs prémultipliées, RGBA)
pub fn render(height: u32) -> Option<Pixmap> {
    let tree = resvg::usvg::Tree::from_data(SVG, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let k = height as f32 / size.height();
    let width = (size.width() * k).round().max(1.0) as u32;
    let mut pm = Pixmap::new(width, height.max(1))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(k, k), &mut pm.as_mut());
    Some(pm)
}

impl Logo {
    /// Image et rectangle du logo, centré sur un écran de taille `screen`
    /// (coordonnées de l'écran) à l'échelle `scale`
    pub fn get(
        &mut self,
        screen: Size<i32, Logical>,
        scale: f64,
    ) -> Option<(&MemoryRenderBuffer, Size<i32, Logical>, Rectangle<i32, Logical>)> {
        let px = (screen.h as f64 * HEIGHT * scale).round().max(1.0) as u32;
        let entry = self.images.entry(px).or_insert_with(|| {
            let pm = render(px)?;
            let (w, h) = (pm.width() as i32, pm.height() as i32);
            let buffer = MemoryRenderBuffer::from_slice(
                pm.data(),
                Fourcc::Abgr8888,
                (w, h),
                1,
                Transform::Normal,
                None,
            );
            Some((buffer, Size::from((w, h))))
        });
        let (buffer, px_size) = entry.as_ref()?;
        let size: Size<i32, Logical> = Size::from((
            (px_size.w as f64 / scale).round() as i32,
            (px_size.h as f64 / scale).round() as i32,
        ));
        let loc = Point::from(((screen.w - size.w) / 2, (screen.h - size.h) / 2));
        Some((buffer, *px_size, Rectangle::new(loc, size)))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn logo() {
        let pm = super::render(216).unwrap();
        assert_eq!(pm.height(), 216);
        assert_eq!(pm.width(), 378);
        // Corps du pacman plein, bouche et coins transparents
        assert_eq!(pm.pixel(80, 108).unwrap().alpha(), 255);
        assert_eq!(pm.pixel(160, 108).unwrap().alpha(), 0);
        assert_eq!(pm.pixel(0, 0).unwrap().alpha(), 0);
    }
}
