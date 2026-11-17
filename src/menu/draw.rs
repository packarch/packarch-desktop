// SPDX-License-Identifier: MIT OR Apache-2.0
//! Dessin d'un menu (§ 10.4): disposition des lignes, puis image en mémoire
//! (fond, bordure, titres, séparateurs, icônes, flèches des sous-menus),
//! coins arrondis anticrénelés. L'ombre est dessinée par le shader des
//! fenêtres.

use resvg::tiny_skia::{self, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};
use smithay::utils::{Logical, Size};

use super::{Entry, Menus};
use crate::config::Config;
use crate::deco::{Text, line_height};
use crate::icons::Icons;
use crate::theme::{Color, Justify, MenuTheme, Theme};

/// Taille des icônes, en pixels logiques
pub const ICON: i32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Item,
    Separator,
    Title,
}

/// Ligne d'un menu, relative au haut du menu
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub y: i32,
    pub h: i32,
    pub kind: RowKind,
}

/// Disposition d'un menu, en pixels logiques
#[derive(Debug, Clone, PartialEq)]
pub struct MenuLayout {
    pub size: Size<i32, Logical>,
    pub rows: Vec<Row>,
    /// Colonne des icônes (0 sans icône)
    pub icon_col: i32,
    /// Colonne des flèches des sous-menus (0 sans sous-menu)
    pub arrow_col: i32,
}

impl MenuLayout {
    /// Ligne sous un point relatif au menu
    pub fn row_at(&self, x: f64, y: f64) -> Option<usize> {
        if x < 0.0 || x >= self.size.w as f64 {
            return None;
        }
        self.rows
            .iter()
            .position(|r| y >= r.y as f64 && y < (r.y + r.h) as f64)
    }
}

/// Libellé affiché d'une entrée
pub fn entry_label<'a>(entry: &'a Entry, menus: &'a Menus) -> &'a str {
    match entry {
        Entry::Item { label, .. } => &label.text,
        Entry::Separator(l) => l.as_deref().unwrap_or(""),
        Entry::Menu(id) => menus.get(id).map(|m| m.label.text.as_str()).unwrap_or(id),
    }
}

fn entry_icon<'a>(entry: &'a Entry, menus: &'a Menus) -> Option<&'a str> {
    match entry {
        Entry::Item { icon, .. } => icon.as_deref(),
        Entry::Menu(id) => menus.get(id).and_then(|m| m.icon.as_deref()),
        Entry::Separator(_) => None,
    }
}

pub fn layout(
    entries: &[Entry],
    menus: &Menus,
    config: &Config,
    theme: &Theme,
    text: &mut Text,
) -> MenuLayout {
    let t = &theme.menu;
    let b = t.border_width;
    let item_h = line_height(&config.font_menu_item) + 2 * t.padding_y;
    let title_h = line_height(&config.font_menu_header) + 2 * t.padding_y;
    let sep_h = t.separator_width + 2 * t.separator_padding_height;
    let icons = config.menu_show_icons
        && entries
            .iter()
            .any(|e| entry_icon(e, menus).is_some() || matches!(e, Entry::Item { checked: true, .. }));
    let has_check = entries
        .iter()
        .any(|e| matches!(e, Entry::Item { checked: true, .. }));
    let icon_col = if icons || has_check { ICON + t.padding_x } else { 0 };
    let arrow_col = if entries.iter().any(|e| matches!(e, Entry::Menu(_))) {
        item_h / 3 + t.padding_x
    } else {
        0
    };
    let mut rows = vec![];
    let mut y = b;
    let mut widest = 0.0f32;
    for e in entries {
        let (kind, h) = match e {
            Entry::Separator(Some(_)) => (RowKind::Title, title_h),
            Entry::Separator(None) => (RowKind::Separator, sep_h),
            _ => (RowKind::Item, item_h),
        };
        let label = entry_label(e, menus);
        if !label.is_empty() {
            let font = if kind == RowKind::Title {
                &config.font_menu_header
            } else {
                &config.font_menu_item
            };
            let extra = if kind == RowKind::Title {
                0.0
            } else {
                (icon_col + arrow_col) as f32
            };
            widest = widest.max(text.measure(label, font, 1.0) + extra);
        }
        rows.push(Row { y, h, kind });
        y += h;
    }
    if rows.is_empty() {
        y += item_h;
    }
    let w = (widest.ceil() as i32 + 2 * t.padding_x + 2 * b).clamp(t.width_min.max(2 * b + 1), t.width_max);
    MenuLayout {
        size: Size::from((w, y + b)),
        rows,
        icon_col,
        arrow_col,
    }
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(
        (c.r * 255.0).round() as u8,
        (c.g * 255.0).round() as u8,
        (c.b * 255.0).round() as u8,
        (c.a * 255.0).round() as u8,
    );
    p.anti_alias = true;
    p
}

fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Color) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pm.fill_rect(r, &paint(c), Transform::identity(), None);
    }
}

/// Rectangle aux coins arrondis
fn rounded(w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    // Approximation d'un quart de cercle par une cubique
    let k = r * 0.552_284_8;
    pb.move_to(r, 0.0);
    pb.line_to(w - r, 0.0);
    pb.cubic_to(w - r + k, 0.0, w, r - k, w, r);
    pb.line_to(w, h - r);
    pb.cubic_to(w, h - r + k, w - r + k, h, w - r, h);
    pb.line_to(r, h);
    pb.cubic_to(r - k, h, 0.0, h - r + k, 0.0, h - r);
    pb.line_to(0.0, r);
    pb.cubic_to(0.0, r - k, r - k, 0.0, r, 0.0);
    pb.close();
    pb.finish()
}

/// Pose une image prémultipliée (`crate::deco::Image`) sur la pixmap
fn blit(pm: &mut Pixmap, img: &crate::deco::Image, x0: i32, y0: i32) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    for y in 0..img.height as i32 {
        let py = y0 + y;
        if py < 0 || py >= ph {
            continue;
        }
        for x in 0..img.width as i32 {
            let px = x0 + x;
            if px < 0 || px >= pw {
                continue;
            }
            let s = ((y as usize) * img.width + x as usize) * 4;
            let a = img.data[s + 3] as u32;
            if a == 0 {
                continue;
            }
            let d = ((py * pw + px) * 4) as usize;
            for c in 0..4 {
                let v = img.data[s + c] as u32 + data[d + c] as u32 * (255 - a) / 255;
                data[d + c] = v.min(255) as u8;
            }
        }
    }
}

/// Coche (menu de fenêtre), dans un carré de `s` pixels
fn check(pm: &mut Pixmap, x: f32, y: f32, s: f32, c: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(x + s * 0.15, y + s * 0.55);
    pb.line_to(x + s * 0.4, y + s * 0.8);
    pb.line_to(x + s * 0.85, y + s * 0.25);
    if let Some(path) = pb.finish() {
        let stroke = tiny_skia::Stroke {
            width: (s * 0.14).max(1.0),
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        pm.stroke_path(&path, &paint(c), &stroke, Transform::identity(), None);
    }
}

/// Flèche d'un sous-menu: triangle pointant à droite
fn arrow(pm: &mut Pixmap, x: f32, cy: f32, s: f32, c: Color) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, cy - s / 2.0);
    pb.line_to(x + s * 0.6, cy);
    pb.line_to(x, cy + s / 2.0);
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &paint(c), FillRule::Winding, Transform::identity(), None);
    }
}

/// Ce qu'il faut pour dessiner un menu
pub struct Draw<'a> {
    pub entries: &'a [Entry],
    pub menus: &'a Menus,
    pub layout: &'a MenuLayout,
    pub selected: Option<usize>,
    pub scale: f64,
}

/// Image du menu, en pixels physiques (RVBA prémultiplié)
pub fn draw(d: &Draw, config: &Config, theme: &Theme, text: &mut Text, icons: &mut Icons) -> Option<Pixmap> {
    let t: &MenuTheme = &theme.menu;
    let s = d.scale as f32;
    let px = |v: i32| (v as f32 * s).round();
    let (w, h) = (px(d.layout.size.w), px(d.layout.size.h));
    let mut pm = Pixmap::new(w.max(1.0) as u32, h.max(1.0) as u32)?;
    let b = px(t.border_width);
    fill_rect(&mut pm, 0.0, 0.0, w, h, t.border_color);
    fill_rect(&mut pm, b, b, w - 2.0 * b, h - 2.0 * b, t.bg);
    let inner_w = w - 2.0 * b;
    for (i, (e, row)) in d.entries.iter().zip(&d.layout.rows).enumerate() {
        let (y, rh) = (px(row.y), px(row.y + row.h) - px(row.y));
        match row.kind {
            RowKind::Separator => {
                let sw = px(t.separator_width).max(if t.separator_width > 0 { 1.0 } else { 0.0 });
                let pw = px(t.separator_padding_width);
                let sy = (y + (rh - sw) / 2.0).round();
                fill_rect(&mut pm, b + pw, sy, inner_w - 2.0 * pw, sw, t.separator_color);
            }
            RowKind::Title => {
                fill_rect(&mut pm, b, y, inner_w, rh, t.title_bg);
                let label = entry_label(e, d.menus);
                let pad = px(t.padding_x);
                let tw = (inner_w - 2.0 * pad).max(0.0) as usize;
                let img = text.title(
                    label,
                    &config.font_menu_header,
                    t.title_text,
                    t.title_justify,
                    tw,
                    rh as usize,
                    d.scale,
                );
                blit(&mut pm, &img, (b + pad) as i32, y as i32);
            }
            RowKind::Item => {
                let selected = d.selected == Some(i);
                let enabled = e.is_selectable(d.menus);
                if selected {
                    fill_rect(&mut pm, b, y, inner_w, rh, t.active_bg);
                }
                let color = match (selected, enabled) {
                    (true, true) => t.active_text,
                    (false, true) => t.text,
                    (true, false) => t.active_disabled_text,
                    (false, false) => t.disabled_text,
                };
                let pad = px(t.padding_x);
                let mut x = b + pad;
                if d.layout.icon_col > 0 {
                    let isz = px(ICON);
                    let iy = (y + (rh - isz) / 2.0).round();
                    let icon = config
                        .menu_show_icons
                        .then(|| entry_icon(e, d.menus))
                        .flatten()
                        .and_then(|name| icons.get(name, isz as u32));
                    if let Some(icon) = icon {
                        pm.draw_pixmap(
                            x as i32,
                            iy as i32,
                            icon.as_ref().as_ref(),
                            &tiny_skia::PixmapPaint::default(),
                            Transform::identity(),
                            None,
                        );
                    } else if matches!(e, Entry::Item { checked: true, .. }) {
                        check(&mut pm, x, iy, isz, color);
                    }
                    x += px(d.layout.icon_col);
                }
                let arrow_w = px(d.layout.arrow_col);
                let tw = (w - b - pad - arrow_w - x).max(0.0) as usize;
                let img = text.title(
                    entry_label(e, d.menus),
                    &config.font_menu_item,
                    color,
                    Justify::Left,
                    tw,
                    rh as usize,
                    d.scale,
                );
                blit(&mut pm, &img, x as i32, y as i32);
                if matches!(e, Entry::Menu(_)) {
                    let size = (rh / 3.0).round().max(3.0);
                    arrow(&mut pm, w - b - pad - size * 0.6, y + rh / 2.0, size, color);
                }
            }
        }
    }
    // Coins arrondis: masque anticrénelé
    let r = px(t.corner_radius);
    if r > 0.0
        && let Some(path) = rounded(w, h, r)
        && let Some(mut mask) = tiny_skia::Mask::new(pm.width(), pm.height())
    {
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        pm.apply_mask(&mask);
    }
    Some(pm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::Action;
    use crate::menu::Label;
    use crate::theme::ThemeRc;

    #[test]
    fn rows() {
        let mut rc = ThemeRc::default();
        rc.parse("menu.border.width: 8\nmenu.items.padding.y: 4\nmenu.separator.width: 1\nmenu.separator.padding.height: 2\nmenu.width.max: 500\nmenu.corner.radius: 6\n");
        let theme = Theme::from_rc("t", &rc, None);
        let config = Config::default();
        let mut text = Text::new();
        let menus = Menus::default();
        let entries = vec![
            Entry::item("Terminal", vec![Action::Execute("kitty".into())]),
            Entry::Separator(None),
            Entry::Separator(Some("Titre".into())),
            Entry::Item {
                label: Label::plain("Coché"),
                icon: None,
                actions: vec![],
                target: None,
                checked: true,
                enabled: true,
            },
        ];
        let l = layout(&entries, &menus, &config, &theme, &mut text);
        assert_eq!(l.rows[0].y, 8);
        assert_eq!(l.rows[1].h, 5);
        assert_eq!(l.rows[1].y, l.rows[0].y + l.rows[0].h);
        assert_eq!(l.size.h, l.rows[3].y + l.rows[3].h + 8);
        assert!(l.icon_col > 0 && l.arrow_col == 0);
        assert_eq!(l.row_at(20.0, 9.0), Some(0));
        assert_eq!(l.row_at(-1.0, 9.0), None);
        let pm = draw(
            &Draw {
                entries: &entries,
                menus: &menus,
                layout: &l,
                selected: Some(0),
                scale: 1.5,
            },
            &config,
            &theme,
            &mut text,
            &mut Icons::default(),
        )
        .unwrap();
        assert_eq!(pm.width() as i32, (l.size.w as f64 * 1.5).round() as i32);
        // Coin transparent, centre opaque
        assert_eq!(pm.pixel(0, 0).unwrap().alpha(), 0);
        assert_eq!(pm.pixel(pm.width() / 2, 4).unwrap().alpha(), 255);
    }
}
