// SPDX-License-Identifier: MIT OR Apache-2.0
//! Affichages à l'écran (cahier des charges § 8.3, § 8.4): liste Alt+Tab
//! (style `classic`, colonnes de `<fields>`) et nom du bureau pendant
//! `popupTime`, arrondis et ombrés (clés `osd.*`), avec un fondu (§ 10.5).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use resvg::tiny_skia::{FillRule, Pixmap, Transform};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::{Logical, Size};

use crate::anim::{Anim, Kind as AnimKind};
use crate::config::{Font, SwitcherField};
use crate::deco::line_height;
use crate::menu::draw::{blit, fill_rect, paint, rounded};
use crate::state::State;
use crate::theme::Justify;

/// Taille des icônes de la liste Alt+Tab (px logiques)
const ICON: i32 = 24;
const PAD: i32 = 8;

#[derive(Debug, Clone, PartialEq)]
pub enum OsdKind {
    Switcher,
    Desktop(String),
}

pub struct OsdImage {
    pub buffer: MemoryRenderBuffer,
    /// Taille en pixels physiques
    pub px: Size<i32, Logical>,
    /// Taille logique
    pub size: Size<i32, Logical>,
    key: u64,
}

pub struct Osd {
    pub kind: OsdKind,
    pub anim: Option<Anim>,
    /// Instant où il disparaît (nom du bureau)
    pub hide_at: Option<Duration>,
    pub image: Option<OsdImage>,
}

/// OSD qui disparaît
pub struct OsdGhost {
    pub image: OsdImage,
    pub anim: Anim,
}

/// Une ligne de la liste Alt+Tab
#[derive(Hash)]
struct Row {
    icon: Option<String>,
    app: String,
    title: String,
    app_id: String,
    desktop: String,
}

fn default_fields() -> Vec<SwitcherField> {
    vec![
        SwitcherField {
            content: "icon".into(),
            width: 5.0,
        },
        SwitcherField {
            content: "desktop_entry_name".into(),
            width: 30.0,
        },
        SwitcherField {
            content: "title".into(),
            width: 65.0,
        },
    ]
}

impl State {
    /// Nom du bureau, pendant `popupTime` ms
    pub fn show_desktop_osd(&mut self) {
        let ms = self.config.desktop_popup_ms;
        if ms == 0 || matches!(self.osd.as_ref().map(|o| &o.kind), Some(OsdKind::Switcher)) {
            return;
        }
        let name = self.config.desktop_names[self.desktop].clone();
        let now = self.now();
        let anim = match &self.osd {
            Some(o) => o.anim,
            None => self.config.animations.start(AnimKind::Osd, now),
        };
        let image = self.osd.take().and_then(|o| o.image);
        self.osd = Some(Osd {
            kind: OsdKind::Desktop(name),
            anim,
            hide_at: Some(now + Duration::from_millis(ms as u64)),
            image,
        });
    }

    /// Liste Alt+Tab
    pub fn show_switcher(&mut self) {
        if !self.config.switcher.show {
            return;
        }
        let now = self.now();
        self.osd = Some(Osd {
            kind: OsdKind::Switcher,
            anim: self.config.animations.start(AnimKind::Osd, now),
            hide_at: None,
            image: None,
        });
    }

    /// Cache l'OSD (fondu)
    pub fn hide_osd(&mut self) {
        let Some(osd) = self.osd.take() else { return };
        let now = self.now();
        if let (Some(image), Some(anim)) = (osd.image, self.config.animations.start(AnimKind::Osd, now)) {
            let shown = osd.anim.map(|a| a.value(now)).unwrap_or(1.0);
            self.osd_ghost = Some(OsdGhost {
                image,
                anim: anim.resumed_from(1.0 - shown.clamp(0.0, 1.0)),
            });
        }
    }

    fn switcher_rows(&mut self) -> (Vec<Row>, usize) {
        let Some(cycle) = self.cycle.as_ref() else {
            return (vec![], 0);
        };
        let windows = cycle.windows.clone();
        let selected = cycle.index;
        let mut rows = vec![];
        for w in &windows {
            let Some(i) = self.window_index(w) else { continue };
            let v = &self.views[i];
            let (title, app_id, desktop) = (v.title(), v.app_id(), v.desktop);
            let app = self.apps.get(&app_id);
            rows.push(Row {
                icon: app
                    .as_ref()
                    .and_then(|a| a.icon.clone())
                    .or_else(|| (!app_id.is_empty()).then(|| app_id.clone())),
                app: app.map(|a| a.name).unwrap_or_else(|| app_id.clone()),
                title,
                app_id,
                desktop: self
                    .config
                    .desktop_names
                    .get(desktop)
                    .cloned()
                    .unwrap_or_default(),
            });
        }
        (rows, selected)
    }

    /// Image de l'OSD pour une échelle et une taille d'écran, refaite si son
    /// contenu a changé
    pub fn osd_image(&mut self, scale: f64, screen: Size<i32, Logical>) {
        let Some(kind) = self.osd.as_ref().map(|o| o.kind.clone()) else {
            return;
        };
        let (rows, selected) = match kind {
            OsdKind::Switcher => self.switcher_rows(),
            OsdKind::Desktop(_) => (vec![], 0),
        };
        let mut h = DefaultHasher::new();
        kind.hash_key(&mut h);
        rows.hash(&mut h);
        selected.hash(&mut h);
        scale.to_bits().hash(&mut h);
        (screen.w, screen.h).hash(&mut h);
        let key = h.finish();
        if self
            .osd
            .as_ref()
            .and_then(|o| o.image.as_ref())
            .is_some_and(|i| i.key == key)
        {
            return;
        }
        let drawn = match &kind {
            OsdKind::Switcher => self.draw_switcher(&rows, selected, scale, screen),
            OsdKind::Desktop(name) => self.draw_desktop(name, scale),
        };
        let Some((pm, size)) = drawn else { return };
        let (w, hh) = (pm.width() as i32, pm.height() as i32);
        let buffer = MemoryRenderBuffer::from_slice(
            pm.data(),
            Fourcc::Abgr8888,
            (w, hh),
            1,
            smithay::utils::Transform::Normal,
            None,
        );
        if let Some(o) = self.osd.as_mut() {
            o.image = Some(OsdImage {
                buffer,
                px: Size::from((w, hh)),
                size,
                key,
            });
        }
    }

    /// Fond, bordure et coins d'un OSD
    fn osd_frame(&self, w: f32, h: f32, scale: f32) -> Option<Pixmap> {
        let t = &self.theme.osd;
        let mut pm = Pixmap::new(w.max(1.0) as u32, h.max(1.0) as u32)?;
        let b = (t.border_width as f32 * scale).round();
        fill_rect(&mut pm, 0.0, 0.0, w, h, t.border_color);
        fill_rect(&mut pm, b, b, w - 2.0 * b, h - 2.0 * b, t.bg);
        Some(pm)
    }

    fn osd_round(&self, pm: &mut Pixmap, scale: f32) {
        let r = (self.theme.osd.corner_radius as f32 * scale).round();
        if r > 0.0
            && let Some(path) = rounded(pm.width() as f32, pm.height() as f32, r)
            && let Some(mut mask) = resvg::tiny_skia::Mask::new(pm.width(), pm.height())
        {
            mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
            pm.apply_mask(&mask);
        }
    }

    fn draw_desktop(&mut self, name: &str, scale: f64) -> Option<(Pixmap, Size<i32, Logical>)> {
        let font = Font {
            size: self.config.font_osd.size * 1.6,
            ..self.config.font_osd.clone()
        };
        let t = self.theme.osd.clone();
        let tw = self.text.measure(name, &font, 1.0).ceil() as i32;
        let lh = line_height(&font);
        let size = Size::from((
            (tw + 6 * PAD + 2 * t.border_width).max(120),
            lh + 4 * PAD + 2 * t.border_width,
        ));
        let s = scale as f32;
        let (w, h) = ((size.w as f32 * s).round(), (size.h as f32 * s).round());
        let mut pm = self.osd_frame(w, h, s)?;
        let img = self.text.title(
            name,
            &font,
            t.text,
            Justify::Center,
            w as usize,
            h as usize,
            scale,
        );
        blit(&mut pm, &img, 0, 0);
        self.osd_round(&mut pm, s);
        Some((pm, size))
    }

    fn draw_switcher(
        &mut self,
        rows: &[Row],
        selected: usize,
        scale: f64,
        screen: Size<i32, Logical>,
    ) -> Option<(Pixmap, Size<i32, Logical>)> {
        if rows.is_empty() {
            return None;
        }
        let t = self.theme.osd.clone();
        let font = self.config.font_osd.clone();
        let fields = if self.config.switcher.fields.is_empty() {
            default_fields()
        } else {
            self.config.switcher.fields.clone()
        };
        let b = t.border_width;
        let row_h = ICON.max(line_height(&font)) + PAD;
        // Lignes visibles: autour de la ligne choisie si tout ne tient pas
        let max_rows = (((screen.h as f32 * 0.9) as i32 - 2 * (b + PAD)) / row_h).max(1) as usize;
        let n = rows.len().min(max_rows);
        let first = selected.saturating_sub(n - 1).min(rows.len() - n);
        let w = 600.min((screen.w as f32 * 0.9) as i32);
        let size = Size::from((w, n as i32 * row_h + 2 * (b + PAD)));
        let s = scale as f32;
        let px = |v: i32| (v as f32 * s).round();
        let mut pm = self.osd_frame(px(size.w), px(size.h), s)?;
        let inner_x = b + PAD;
        let inner_w = size.w - 2 * inner_x;
        let total: f64 = fields.iter().map(|f| f.width.max(0.0)).sum::<f64>().max(1.0);
        for (k, row) in rows[first..first + n].iter().enumerate() {
            let y = b + PAD + k as i32 * row_h;
            let is_sel = first + k == selected;
            if is_sel && let Some(path) = rounded(px(inner_w + PAD), px(row_h), px(4)) {
                pm.fill_path(
                    &path,
                    &paint(t.hilight_bg),
                    FillRule::Winding,
                    Transform::from_translate(px(inner_x - PAD / 2), px(y)),
                    None,
                );
            }
            let color = if is_sel { t.hilight_text } else { t.text };
            let mut x = inner_x as f64;
            for f in &fields {
                let fw = inner_w as f64 * f.width.max(0.0) / total;
                let content = f.content.to_ascii_lowercase();
                if content == "icon" {
                    let isz = px(ICON);
                    if let Some(name) = &row.icon
                        && let Some(icon) = self.icons.get(name, isz as u32)
                    {
                        pm.draw_pixmap(
                            px(x.round() as i32) as i32,
                            px(y + (row_h - ICON) / 2) as i32,
                            icon.as_ref().as_ref(),
                            &resvg::tiny_skia::PixmapPaint::default(),
                            Transform::identity(),
                            None,
                        );
                    }
                } else {
                    let text = match content.as_str() {
                        "desktop_entry_name" => &row.app,
                        "identifier" | "trimmed_identifier" => &row.app_id,
                        "workspace" | "desktop" => &row.desktop,
                        _ => &row.title,
                    };
                    let img = self.text.title(
                        text,
                        &font,
                        color,
                        Justify::Left,
                        px((fw as i32 - PAD).max(0)) as usize,
                        px(row_h) as usize,
                        scale,
                    );
                    blit(&mut pm, &img, px(x.round() as i32) as i32, px(y) as i32);
                }
                x += fw;
            }
        }
        self.osd_round(&mut pm, s);
        Some((pm, size))
    }
}

impl OsdKind {
    fn hash_key(&self, h: &mut DefaultHasher) {
        match self {
            OsdKind::Switcher => 0u8.hash(h),
            OsdKind::Desktop(n) => n.hash(h),
        }
    }
}
