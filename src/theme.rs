// SPDX-License-Identifier: MIT OR Apache-2.0
//! Thèmes (cahier des charges § 6): recherche, lecture de `themerc` et de
//! `themerc-override`, couleurs, images des boutons (xbm).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::config::{Button, Config};

/// Couleur RVBA, composantes de 0 à 1, non prémultipliée
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    /// `#rgb`, `#rrggbb`, `#rrggbbaa`, et `#rrggbb 50` (opacité en %)
    pub fn parse(s: &str) -> Option<Color> {
        let mut parts = s.split_whitespace();
        let hex = parts.next()?.strip_prefix('#')?;
        let byte = |i: usize| {
            u8::from_str_radix(hex.get(i..i + 2)?, 16)
                .ok()
                .map(|v| v as f32 / 255.0)
        };
        let mut c = match hex.len() {
            3 => {
                let n = |i: usize| {
                    u8::from_str_radix(hex.get(i..i + 1)?, 16)
                        .ok()
                        .map(|v| v as f32 / 15.0)
                };
                Color::rgba(n(0)?, n(1)?, n(2)?, 1.0)
            }
            6 => Color::rgba(byte(0)?, byte(2)?, byte(4)?, 1.0),
            8 => Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?),
            _ => return None,
        };
        if let Some(pct) = parts.next() {
            c.a = (pct.parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0);
        }
        Some(c)
    }

    /// Composantes prémultipliées, pour les shaders
    pub fn premultiplied(self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }
}

/// Clés et valeurs d'un `themerc`, avec jokers `*`
#[derive(Debug, Default, Clone)]
pub struct ThemeRc {
    exact: HashMap<String, String>,
    patterns: Vec<(Vec<String>, String)>,
}

impl ThemeRc {
    pub fn parse(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if key.contains('*') {
                let parts = key.split('.').map(str::to_string).collect();
                self.patterns.retain(|(p, _)| p.join(".") != key);
                self.patterns.push((parts, value));
            } else {
                self.exact.insert(key, value);
            }
        }
    }

    /// Valeur d'une clé: exacte, sinon le motif le plus précis (le moins de `*`)
    pub fn get(&self, key: &str) -> Option<&str> {
        let key = key.to_ascii_lowercase();
        if let Some(v) = self.exact.get(&key) {
            return Some(v);
        }
        let parts: Vec<&str> = key.split('.').collect();
        self.patterns
            .iter()
            .filter(|(p, _)| p.len() == parts.len() && p.iter().zip(&parts).all(|(a, b)| a == "*" || a == b))
            .min_by_key(|(p, _)| p.iter().filter(|s| *s == "*").count())
            .map(|(_, v)| v.as_str())
    }

    pub fn color(&self, key: &str) -> Option<Color> {
        let v = self.get(key)?;
        let c = Color::parse(v);
        if c.is_none() {
            warn!("thème: couleur invalide pour {key}: {v}");
        }
        c
    }

    pub fn int(&self, key: &str) -> Option<i32> {
        self.get(key)
            .and_then(|v| v.split_whitespace().next()?.parse().ok())
    }
}

/// Image monochrome d'un bouton (1 = pixel allumé)
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub width: usize,
    pub height: usize,
    pub bits: Vec<bool>,
}

impl Mask {
    /// Fichier xbm (`#define x_width 6` … `static unsigned char x_bits[] = {…}`)
    pub fn parse_xbm(text: &str) -> Option<Mask> {
        let mut width = None;
        let mut height = None;
        for line in text.lines() {
            let mut w = line.split_whitespace();
            if w.next() == Some("#define") {
                let (name, value) = (w.next()?, w.next()?);
                if name.ends_with("_width") {
                    width = value.parse().ok();
                } else if name.ends_with("_height") {
                    height = value.parse().ok();
                }
            }
        }
        let (width, height): (usize, usize) = (width?, height?);
        let body = &text[text.find('{')? + 1..text.rfind('}')?];
        let bytes: Vec<u8> = body
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                let s = s.trim_start_matches("0x").trim_start_matches("0X");
                u8::from_str_radix(s, 16)
            })
            .collect::<Result<_, _>>()
            .ok()?;
        let stride = width.div_ceil(8);
        if bytes.len() < stride * height {
            return None;
        }
        let mut bits = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                bits.push(bytes[y * stride + x / 8] >> (x % 8) & 1 == 1);
            }
        }
        Some(Mask { width, height, bits })
    }

    /// Images intégrées, si le thème n'en fournit pas
    fn builtin(button: Button, toggled: bool) -> Mask {
        let rows: &[&str] = match (button, toggled) {
            (Button::Close, _) => &["#....#", ".#..#.", "..##..", "..##..", ".#..#.", "#....#"],
            (Button::Maximize, false) => &["######", "######", "#....#", "#....#", "#....#", "######"],
            (Button::Maximize, true) => &["..####", "..#..#", "####.#", "#..###", "#..#..", "####.."],
            (Button::Iconify, _) => &["......", "......", "......", "......", "######", "######"],
            (Button::Shade, _) => &["######", "######", "......", "......", "......", "......"],
            (Button::Desk, _) => &["##..##", "##..##", "......", "......", "##..##", "##..##"],
        };
        Mask {
            width: 6,
            height: rows.len(),
            bits: rows.iter().flat_map(|r| r.chars().map(|c| c == '#')).collect(),
        }
    }
}

/// Couleurs d'une fenêtre active ou inactive
#[derive(Debug, Clone)]
pub struct WindowColors {
    pub title_bg: Color,
    pub label_text: Color,
    pub border: Color,
    pub button_bg: Color,
    pub button_hover_bg: Color,
    pub button_pressed_bg: Color,
    /// Couleur de l'image de chaque bouton: (normal, survolé, appuyé)
    pub button_image: HashMap<Button, [Color; 3]>,
    pub shadow_size: i32,
    pub shadow_color: Color,
}

/// Alignement du titre
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Justify {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone)]
pub struct Theme {
    #[allow(dead_code)]
    pub name: String,
    pub border_width: i32,
    pub padding_width: i32,
    pub padding_height: i32,
    pub justify: Justify,
    pub active: WindowColors,
    pub inactive: WindowColors,
    /// Images des boutons: (normal, basculé)
    pub masks: HashMap<Button, (Mask, Mask)>,
}

/// Dossiers où chercher un thème, par ordre de priorité
fn theme_dirs(name: &str) -> Vec<PathBuf> {
    let mut dirs = vec![];
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    if let Some(d) = data_home {
        dirs.push(d.join("themes").join(name));
    }
    if let Some(h) = &home {
        dirs.push(h.join(".themes").join(name));
    }
    let data_dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data_dirs.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(d).join("themes").join(name));
    }
    dirs
}

/// Dossier du thème (`…/NOM/packarch` ou `…/NOM/openbox-3`) et son themerc
fn find_theme(name: &str) -> Option<PathBuf> {
    for dir in theme_dirs(name) {
        for sub in ["packarch", "openbox-3"] {
            let d = dir.join(sub);
            if d.join("themerc").is_file() {
                return Some(d);
            }
        }
    }
    None
}

fn colors(rc: &ThemeRc, state: &str, fallback_shadow: Color) -> WindowColors {
    let k = |s: &str| format!("window.{state}.{s}");
    let title_bg = rc
        .color(&k("title.bg.color"))
        .unwrap_or(Color::rgba(0.2, 0.2, 0.2, 1.0));
    let label_text = rc
        .color(&k("label.text.color"))
        .unwrap_or(Color::rgba(1.0, 1.0, 1.0, 1.0));
    let button_bg = rc.color(&k("button.unpressed.bg.color")).unwrap_or(title_bg);
    let button_hover_bg = rc.color(&k("button.hover.bg.color")).unwrap_or(button_bg);
    let button_pressed_bg = rc.color(&k("button.pressed.bg.color")).unwrap_or(button_hover_bg);
    let normal = rc.color(&k("button.unpressed.image.color")).unwrap_or(label_text);
    let hover = rc.color(&k("button.hover.image.color")).unwrap_or(normal);
    let pressed = rc.color(&k("button.pressed.image.color")).unwrap_or(hover);
    let mut button_image = HashMap::new();
    for b in [
        Button::Iconify,
        Button::Maximize,
        Button::Close,
        Button::Shade,
        Button::Desk,
    ] {
        let n = b.image_name();
        button_image.insert(
            b,
            [
                rc.color(&k(&format!("button.{n}.unpressed.image.color")))
                    .unwrap_or(normal),
                rc.color(&k(&format!("button.{n}.hover.image.color")))
                    .unwrap_or(hover),
                rc.color(&k(&format!("button.{n}.pressed.image.color")))
                    .unwrap_or(pressed),
            ],
        );
    }
    WindowColors {
        title_bg,
        label_text,
        border: rc
            .color(&k("border.color"))
            .or_else(|| rc.color("border.color"))
            .unwrap_or(title_bg),
        button_bg,
        button_hover_bg,
        button_pressed_bg,
        button_image,
        shadow_size: rc
            .int(&k("shadow.size"))
            .unwrap_or(if state == "active" { 40 } else { 24 })
            .max(0),
        shadow_color: rc.color(&k("shadow.color")).unwrap_or(fallback_shadow),
    }
}

impl Theme {
    /// Thème de la configuration, `themerc-override` compris
    pub fn load(config: &Config) -> Theme {
        let mut rc = ThemeRc::default();
        let dir = find_theme(&config.theme_name);
        match &dir {
            Some(d) => {
                info!("thème: {}", d.display());
                rc.parse(&std::fs::read_to_string(d.join("themerc")).unwrap_or_default());
            }
            None if !config.theme_name.is_empty() => warn!("thème introuvable: {}", config.theme_name),
            None => {}
        }
        if let Ok(text) = std::fs::read_to_string(config.dir.join("themerc-override")) {
            rc.parse(&text);
        }
        Theme::from_rc(&config.theme_name, &rc, dir.as_deref())
    }

    pub fn from_rc(name: &str, rc: &ThemeRc, dir: Option<&Path>) -> Theme {
        let justify = match rc
            .get("window.label.text.justify")
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("left") => Justify::Left,
            Some("right") => Justify::Right,
            _ => Justify::Center,
        };
        let mut masks = HashMap::new();
        for b in [
            Button::Iconify,
            Button::Maximize,
            Button::Close,
            Button::Shade,
            Button::Desk,
        ] {
            let load = |file: String, toggled: bool| {
                dir.and_then(|d| std::fs::read_to_string(d.join(file)).ok())
                    .and_then(|t| Mask::parse_xbm(&t))
                    .unwrap_or_else(|| Mask::builtin(b, toggled))
            };
            let n = b.image_name();
            masks.insert(
                b,
                (
                    load(format!("{n}.xbm"), false),
                    load(format!("{n}_toggled.xbm"), true),
                ),
            );
        }
        Theme {
            name: name.to_string(),
            border_width: rc.int("border.width").unwrap_or(1).max(0),
            padding_width: rc.int("padding.width").unwrap_or(4).max(0),
            padding_height: rc.int("padding.height").unwrap_or(4).max(0),
            justify,
            active: colors(rc, "active", Color::rgba(0.0, 0.0, 0.0, 0.6)),
            inactive: colors(rc, "inactive", Color::rgba(0.0, 0.0, 0.0, 0.4)),
            masks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(Color::parse("#ffffff"), Some(Color::rgba(1.0, 1.0, 1.0, 1.0)));
        assert_eq!(Color::parse("#000000ff").unwrap().a, 1.0);
        assert!((Color::parse("#00000099").unwrap().a - 0.6).abs() < 1e-6);
        assert_eq!(Color::parse("#f00"), Some(Color::rgba(1.0, 0.0, 0.0, 1.0)));
        assert!((Color::parse("#20262C 50").unwrap().a - 0.5).abs() < 1e-6);
        assert_eq!(Color::parse("rouge"), None);
    }

    #[test]
    fn wildcards() {
        let mut rc = ThemeRc::default();
        rc.parse("window.*.border.color: #111111\nwindow.active.border.color: #222222\n# commentaire\n");
        assert_eq!(rc.get("window.active.border.color"), Some("#222222"));
        assert_eq!(rc.get("window.inactive.border.color"), Some("#111111"));
        assert_eq!(rc.get("window.inactive.title.bg.color"), None);
    }

    #[test]
    fn xbm() {
        let text = "#define close_width 10\n#define close_height 2\nstatic unsigned char close_bits[] = {\n 0xff, 0x03, 0x01, 0x02 };";
        let m = Mask::parse_xbm(text).unwrap();
        assert_eq!((m.width, m.height), (10, 2));
        assert!(m.bits[..10].iter().all(|b| *b));
        assert!(m.bits[10] && !m.bits[11] && m.bits[19]);
    }

    #[test]
    fn packarch_themes() {
        // Tous les thèmes OB-Packarch-* installés doivent se lire
        let Ok(entries) = std::fs::read_dir("/usr/share/themes") else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("OB-Packarch-") {
                continue;
            }
            let dir = e.path().join("openbox-3");
            let mut rc = ThemeRc::default();
            rc.parse(&std::fs::read_to_string(dir.join("themerc")).unwrap());
            let t = Theme::from_rc(&name, &rc, Some(&dir));
            assert!(t.border_width >= 0, "{name}");
            assert!(rc.color("window.active.title.bg.color").is_some(), "{name}");
            for b in [Button::Close, Button::Maximize, Button::Iconify] {
                let (m, _) = &t.masks[&b];
                assert!(m.width > 0 && m.bits.iter().any(|x| *x), "{name} {b:?}");
            }
        }
    }
}
