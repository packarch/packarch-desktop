// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fond par défaut (option `<core><logo>`) : le fond au labyrinthe du thème
//! packarch, en plein écran, dessiné sous les couches de fond, donc caché
//! dès qu'un programme pose un fond d'écran (swaybg…). L'image (un SVG qui
//! contient le JPEG) est intégrée au programme et rendue à la taille exacte
//! de chaque écran, recadrée sans déformation.

use std::collections::HashMap;

use resvg::tiny_skia::{self, Pixmap};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Point, Rectangle, Size, Transform};

/// Fond au labyrinthe (Packarch/branding/fonds/packarch-labyrinthe.jpg)
const SVG: &[u8] = include_bytes!("../data/fond.svg");

/// Images du fond déjà rendues, par taille en pixels physiques
#[derive(Default)]
pub struct Logo {
    images: HashMap<(u32, u32), Option<MemoryRenderBuffer>>,
}

/// Rend le fond sur `width` x `height` pixels : il couvre toute la surface,
/// centré, le surplus coupé (couleurs prémultipliées, RGBA)
pub fn render(width: u32, height: u32) -> Option<Pixmap> {
    let tree = resvg::usvg::Tree::from_data(SVG, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let k = (w / size.width()).max(h / size.height());
    let tx = (w - size.width() * k) / 2.0;
    let ty = (h - size.height() * k) / 2.0;
    let mut pm = Pixmap::new(width.max(1), height.max(1))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_row(k, 0.0, 0.0, k, tx, ty),
        &mut pm.as_mut(),
    );
    Some(pm)
}

impl Logo {
    /// Image et rectangle du fond pour un écran de taille `screen`
    /// (coordonnées de l'écran) à l'échelle `scale`
    pub fn get(
        &mut self,
        screen: Size<i32, Logical>,
        scale: f64,
    ) -> Option<(&MemoryRenderBuffer, Size<i32, Logical>, Rectangle<i32, Logical>)> {
        let w = (screen.w as f64 * scale).round().max(1.0) as u32;
        let h = (screen.h as f64 * scale).round().max(1.0) as u32;
        let entry = self.images.entry((w, h)).or_insert_with(|| {
            let pm = render(w, h)?;
            Some(MemoryRenderBuffer::from_slice(
                pm.data(),
                Fourcc::Abgr8888,
                (w as i32, h as i32),
                1,
                Transform::Normal,
                None,
            ))
        });
        let buffer = entry.as_ref()?;
        Some((
            buffer,
            Size::from((w as i32, h as i32)),
            Rectangle::new(Point::from((0, 0)), screen),
        ))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fond() {
        // Écran 16:10 : l'image 16:9 couvre tout, recadrée sur les côtés
        let pm = super::render(320, 200).unwrap();
        assert_eq!((pm.width(), pm.height()), (320, 200));
        // Opaque partout, coins compris
        for (x, y) in [(0, 0), (319, 0), (0, 199), (319, 199), (160, 100)] {
            assert_eq!(pm.pixel(x, y).unwrap().alpha(), 255, "{x},{y}");
        }
        // Sombre : fond #0e0e11 et murs bleu nuit
        let p = pm.pixel(5, 5).unwrap();
        assert!(p.red() < 80 && p.green() < 80 && p.blue() < 140, "{p:?}");
    }
}
