// SPDX-License-Identifier: MIT OR Apache-2.0
//! Curseur de la souris (session réelle, § 5.6, § 9.1): thème xcursor
//! (`XCURSOR_THEME`, `XCURSOR_SIZE`), formes demandées par les clients
//! (cursor-shape) ou surface de curseur d'un client.

use std::collections::HashMap;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::surface::render_elements_from_surface_tree;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::output::Output;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Physical, Point, Rectangle, Size, Transform};
use smithay::wayland::compositor::with_states;
use tracing::warn;

use crate::render::Elem;
use crate::state::State;

/// Image d'un curseur à une taille donnée
struct Image {
    buffer: MemoryRenderBuffer,
    size: Size<i32, Physical>,
    hotspot: Point<i32, Physical>,
}

pub struct Cursors {
    theme_name: String,
    theme: xcursor::CursorTheme,
    /// Taille logique
    size: u32,
    cache: HashMap<(String, u32), Option<Image>>,
}

impl Cursors {
    pub fn new(theme: &str, size: u32) -> Cursors {
        Cursors {
            theme_name: theme.to_string(),
            theme: xcursor::CursorTheme::load(theme),
            size: size.max(8),
            cache: HashMap::new(),
        }
    }

    /// Change de thème ou de taille (fichier `environment` relu)
    pub fn set(&mut self, theme: &str, size: u32) {
        if theme != self.theme_name || size != self.size {
            *self = Cursors::new(theme, size);
        }
    }

    /// Image d'un curseur nommé à `px` pixels physiques: celle du thème la
    /// plus proche, sinon une flèche dessinée
    fn image(&mut self, name: &str, px: u32) -> &Option<Image> {
        let key = (name.to_string(), px);
        if !self.cache.contains_key(&key) {
            let loaded = self
                .theme
                .load_icon(name)
                .or_else(|| self.theme.load_icon("default"))
                .or_else(|| self.theme.load_icon("left_ptr"))
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|data| xcursor::parser::parse_xcursor(&data))
                .and_then(|images| {
                    images
                        .into_iter()
                        .min_by_key(|i| (i.size as i64 - px as i64).abs())
                })
                .map(|i| Image {
                    buffer: MemoryRenderBuffer::from_slice(
                        &premultiply(&i.pixels_rgba),
                        Fourcc::Argb8888,
                        (i.width as i32, i.height as i32),
                        1,
                        Transform::Normal,
                        None,
                    ),
                    size: Size::from((i.width as i32, i.height as i32)),
                    hotspot: Point::from((i.xhot as i32, i.yhot as i32)),
                })
                .or_else(|| Some(fallback(px)));
            self.cache.insert(key.clone(), loaded);
        }
        &self.cache[&key]
    }
}

/// RVBA en ARGB8888 prémultiplié (octets B, V, R, A en mémoire): le format
/// du plan curseur, que le compositeur DRM copie alors tel quel sans le
/// dessiner dans l'image (un seul curseur sous QEMU/SPICE)
fn premultiply(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let a = p[3] as u32;
            let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
            [m(p[2]), m(p[1]), m(p[0]), p[3]]
        })
        .collect()
}

/// Flèche si le thème n'a aucun curseur
fn fallback(px: u32) -> Image {
    let s = px.max(8) as i32;
    let mut data = vec![0u8; (s * s * 4) as usize];
    for y in 0..s {
        for x in 0..=y.min(s - 1) {
            if x <= y / 2 + y / 4 {
                let edge = x == 0 || x == y / 2 + y / 4 || y == s - 1;
                let v = if edge { 0 } else { 255 };
                let i = ((y * s + x) * 4) as usize;
                data[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
    }
    Image {
        buffer: MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (s, s), 1, Transform::Normal, None),
        size: Size::from((s, s)),
        hotspot: Point::from((0, 0)),
    }
}

impl State {
    /// Thème et taille du curseur d'après l'environnement de la session
    pub fn cursor_settings(&self) -> (String, u32) {
        let get = |k: &str| {
            self.child_env
                .iter()
                .rev()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
                .or_else(|| std::env::var(k).ok())
        };
        let theme = get("XCURSOR_THEME").unwrap_or_else(|| "default".into());
        let size = get("XCURSOR_SIZE").and_then(|s| s.parse().ok()).unwrap_or(24);
        (theme, size)
    }

    /// Icône du glisser-déposer en cours, sous le pointeur (devant la scène,
    /// derrière le curseur). Elle fait partie de l'image capturée, même
    /// sans le curseur.
    pub fn dnd_icon_elements(&self, renderer: &mut GlesRenderer, output: &Output) -> Vec<Elem> {
        let Some((surface, offset)) = &self.dnd_icon else {
            return vec![];
        };
        if !surface.is_alive() {
            return vec![];
        }
        let scale = output.current_scale().fractional_scale();
        let geo = self.output_geometry(output);
        let loc = self.pointer - geo.loc.to_f64() + offset.to_f64();
        let loc = loc.to_physical(scale).to_i32_round();
        render_elements_from_surface_tree(renderer, surface, loc, scale, 1.0, Kind::Unspecified)
    }

    /// Éléments du curseur sur un écran (devant tout le reste)
    pub fn cursor_elements(&mut self, renderer: &mut GlesRenderer, output: &Output) -> Vec<Elem> {
        let scale = output.current_scale().fractional_scale();
        let geo = self.output_geometry(output);
        let pos = self.pointer - geo.loc.to_f64();
        self.cursor_hotspot = None;
        // Hors de cet écran (avec une marge pour les grands curseurs)
        if !Rectangle::new(Point::from((-64, -64)), geo.size + Size::from((128, 128)))
            .to_f64()
            .contains(pos)
        {
            return vec![];
        }
        match self.cursor_status.clone() {
            CursorImageStatus::Hidden => vec![],
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(&surface, |s| {
                    s.data_map
                        .get::<CursorImageSurfaceData>()
                        .map(|d| d.lock().unwrap().hotspot)
                        .unwrap_or_default()
                });
                let loc: Point<f64, Logical> = pos - hotspot.to_f64();
                let loc = loc.to_physical(scale).to_i32_round();
                self.cursor_hotspot = Some(pos.to_physical(scale).to_i32_round() - loc);
                render_elements_from_surface_tree(renderer, &surface, loc, scale, 1.0, Kind::Cursor)
            }
            CursorImageStatus::Named(icon) => {
                let (theme, size) = self.cursor_settings();
                let cursors = self.cursors.get_or_insert_with(|| Cursors::new(&theme, size));
                cursors.set(&theme, size);
                let px = (size as f64 * scale).round() as u32;
                let Some(img) = cursors.image(icon.name(), px) else {
                    return vec![];
                };
                let loc = pos.to_physical(scale) - img.hotspot.to_f64();
                let hotspot = img.hotspot;
                let logical = img.size.to_f64().to_logical(scale).to_i32_round();
                match MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    loc,
                    &img.buffer,
                    None,
                    Some(Rectangle::from_size(img.size.to_f64().to_logical(1.0))),
                    Some(logical),
                    Kind::Cursor,
                ) {
                    Ok(e) => {
                        self.cursor_hotspot = Some(hotspot);
                        vec![Elem::Memory(e)]
                    }
                    Err(e) => {
                        warn!("curseur: {e:?}");
                        vec![]
                    }
                }
            }
        }
    }
}
