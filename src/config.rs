// SPDX-License-Identifier: MIT OR Apache-2.0
//! Configuration (cahier des charges § 5): dossier de configuration et
//! lecture de `rc.xml`. Prototype 0.1: section `theme` seulement.

use std::path::{Path, PathBuf};

use tracing::warn;

/// Police d'une partie du bureau (`<font place="…">`)
#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub name: String,
    /// Taille en points
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            name: "sans-serif".into(),
            size: 10.0,
            bold: false,
            italic: false,
        }
    }
}

/// Bouton de la barre de titre
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Iconify,
    Maximize,
    Close,
    Shade,
    Desk,
}

impl Button {
    fn parse(s: &str) -> Option<Button> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "iconify" => Button::Iconify,
            "max" => Button::Maximize,
            "close" => Button::Close,
            "shade" => Button::Shade,
            "desk" => Button::Desk,
            _ => return None,
        })
    }

    /// Nom de l'image dans un thème Openbox (`close.xbm`…)
    pub fn image_name(self) -> &'static str {
        match self {
            Button::Iconify => "iconify",
            Button::Maximize => "max",
            Button::Close => "close",
            Button::Shade => "shade",
            Button::Desk => "desk",
        }
    }
}

/// Réglages de `rc.xml` utiles au prototype
#[derive(Debug, Clone)]
pub struct Config {
    pub dir: PathBuf,
    pub theme_name: String,
    pub buttons_left: Vec<Button>,
    pub buttons_right: Vec<Button>,
    pub show_title: bool,
    pub corner_radius: i32,
    pub drop_shadows: bool,
    pub drop_shadows_on_tiled: bool,
    pub font_active: Font,
    pub font_inactive: Font,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            dir: PathBuf::new(),
            theme_name: String::new(),
            buttons_left: vec![],
            buttons_right: vec![Button::Iconify, Button::Maximize, Button::Close],
            show_title: true,
            corner_radius: 8,
            drop_shadows: true,
            drop_shadows_on_tiled: false,
            font_active: Font {
                bold: true,
                ..Font::default()
            },
            font_inactive: Font::default(),
        }
    }
}

/// Dossier de configuration: `-C`, sinon `$XDG_CONFIG_HOME/packarch`, sinon
/// `/etc/xdg/packarch`
pub fn config_dir(cli: Option<&Path>) -> PathBuf {
    if let Some(dir) = cli {
        return dir.to_path_buf();
    }
    let home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(dir) = home.map(|h| h.join("packarch"))
        && dir.join("rc.xml").exists()
    {
        return dir;
    }
    PathBuf::from("/etc/xdg/packarch")
}

/// Booléen de la configuration: yes/no, true/false, on/off
pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Some(true),
        "no" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

/// `gauche:droite`, éléments séparés par des virgules (`:iconify,max,close`)
fn parse_layout(s: &str) -> (Vec<Button>, Vec<Button>) {
    let (left, right) = s.split_once(':').unwrap_or(("", s));
    let list = |part: &str| part.split(',').filter_map(Button::parse).collect();
    (list(left), list(right))
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|c| c.is_element() && eq(c.tag_name().name(), name))
}

fn text<'a>(node: roxmltree::Node<'a, '_>) -> &'a str {
    node.text().unwrap_or("").trim()
}

fn parse_font(node: roxmltree::Node, base: &Font) -> Font {
    let mut font = base.clone();
    for c in node.children().filter(|c| c.is_element()) {
        let v = text(c);
        match c.tag_name().name().to_ascii_lowercase().as_str() {
            "name" if !v.is_empty() => font.name = v.to_string(),
            "size" => match v.parse::<f32>() {
                Ok(s) if s > 0.0 => font.size = s,
                _ => warn!("rc.xml: taille de police invalide: {v}"),
            },
            "weight" => font.bold = eq(v, "bold"),
            "slant" => font.italic = eq(v, "italic") || eq(v, "oblique"),
            _ => {}
        }
    }
    font
}

impl Config {
    /// Lit `rc.xml` du dossier; une erreur garde les valeurs par défaut
    pub fn load(dir: &Path) -> Config {
        let mut config = Config {
            dir: dir.to_path_buf(),
            ..Config::default()
        };
        let path = dir.join("rc.xml");
        match std::fs::read_to_string(&path) {
            Ok(xml) => {
                if let Err(e) = config.parse(&xml) {
                    warn!("{}: {e}", path.display());
                }
            }
            Err(e) => warn!("{}: {e}", path.display()),
        }
        config
    }

    pub fn parse(&mut self, xml: &str) -> Result<(), String> {
        let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
        let root = doc.root_element();
        let name = root.tag_name().name();
        if !["packarch_config", "openbox_config", "labwc_config"]
            .iter()
            .any(|r| eq(name, r))
        {
            return Err(format!("racine <{name}> inconnue"));
        }
        if let Some(theme) = child(root, "theme") {
            self.parse_theme(theme);
        }
        Ok(())
    }

    fn parse_theme(&mut self, theme: roxmltree::Node) {
        for c in theme.children().filter(|c| c.is_element()) {
            let v = text(c);
            match c.tag_name().name().to_ascii_lowercase().as_str() {
                "name" => self.theme_name = v.to_string(),
                "cornerradius" => match v.parse::<i32>() {
                    Ok(r) if r >= 0 => self.corner_radius = r,
                    _ => warn!("rc.xml: cornerRadius invalide: {v}"),
                },
                "dropshadows" => self.drop_shadows = parse_bool(v).unwrap_or(self.drop_shadows),
                "dropshadowsontiled" => {
                    self.drop_shadows_on_tiled = parse_bool(v).unwrap_or(self.drop_shadows_on_tiled)
                }
                "titlebar" => {
                    if let Some(layout) = child(c, "layout") {
                        (self.buttons_left, self.buttons_right) = parse_layout(text(layout));
                    }
                    if let Some(show) = child(c, "showTitle") {
                        self.show_title = parse_bool(text(show)).unwrap_or(true);
                    }
                }
                "font" => {
                    let place = c.attribute("place").unwrap_or("");
                    if eq(place, "ActiveWindow") || place.is_empty() {
                        self.font_active = parse_font(c, &self.font_active);
                    }
                    if eq(place, "InactiveWindow") || place.is_empty() {
                        self.font_inactive = parse_font(c, &self.font_inactive);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        let (l, r) = parse_layout(":iconify,max,close");
        assert!(l.is_empty());
        assert_eq!(r, vec![Button::Iconify, Button::Maximize, Button::Close]);
        let (l, r) = parse_layout("close:shade");
        assert_eq!(l, vec![Button::Close]);
        assert_eq!(r, vec![Button::Shade]);
    }

    #[test]
    fn roots_and_theme() {
        let xml = r#"<?xml version="1.0"?>
            <labwc_config><theme><name>OB-Packarch-Packarch</name>
            <titlebar><layout>:iconify,max,close</layout><showTitle>no</showTitle></titlebar>
            <cornerRadius>6</cornerRadius><dropShadows>yes</dropShadows>
            <font place="ActiveWindow"><name>Noto Sans</name><size>10</size><weight>bold</weight></font>
            <font place="InactiveWindow"><name>Noto Sans</name><size>11</size><weight>normal</weight></font>
            </theme></labwc_config>"#;
        let mut c = Config::default();
        c.parse(xml).unwrap();
        assert_eq!(c.theme_name, "OB-Packarch-Packarch");
        assert_eq!(c.corner_radius, 6);
        assert!(!c.show_title);
        assert_eq!(
            c.font_active,
            Font {
                name: "Noto Sans".into(),
                size: 10.0,
                bold: true,
                italic: false
            }
        );
        assert_eq!(c.font_inactive.size, 11.0);
        assert!(!c.font_inactive.bold);
        for root in ["packarch_config", "openbox_config"] {
            let mut c = Config::default();
            c.parse(&format!("<{root}/>")).unwrap();
        }
        assert!(Config::default().parse("<sway/>").is_err());
    }

    #[test]
    fn packarch_rc() {
        // Le rc.xml installé par Packarch, s'il est présent sur la machine
        let path = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/labwc/rc.xml"));
        if let Some(xml) = path.and_then(|p| std::fs::read_to_string(p).ok()) {
            let mut c = Config::default();
            c.parse(&xml).unwrap();
            assert!(!c.theme_name.is_empty());
        }
    }
}
