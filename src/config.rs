// SPDX-License-Identifier: MIT OR Apache-2.0
//! Configuration (cahier des charges § 5): dossier de configuration et
//! lecture de `rc.xml`. Les éléments inconnus ou invalides sont ignorés avec
//! un avertissement (`Config::warnings`).

use std::path::{Path, PathBuf};

use tracing::warn;

use crate::actions::{self, Action, line};
use crate::bindings::{Context, Keybind, Mousebind, parse_combo, parse_keysym};

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

/// Politique de placement des nouvelles vues (§ 8.2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Automatic,
    Cursor,
    Center,
}

impl Placement {
    pub fn parse(s: &str) -> Option<Placement> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "automatic" | "smart" => Placement::Automatic,
            "cursor" | "undermouse" => Placement::Cursor,
            "center" | "centre" => Placement::Center,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Margin {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

/// Région nommée, en pourcentage de la zone utile (§ 5.2)
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Affichage de la taille pendant un redimensionnement
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupShow {
    Never,
    Always,
    Nonpixel,
}

/// Colonne de la liste Alt+Tab
#[derive(Debug, Clone, PartialEq)]
pub struct SwitcherField {
    pub content: String,
    pub width: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Switcher {
    pub preview: bool,
    pub outlines: bool,
    pub unshade: bool,
    pub show: bool,
    pub style: String,
    pub fields: Vec<SwitcherField>,
}

/// Réglages d'un type de périphérique d'entrée (appliqués avec udev, 0.4)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InputDevice {
    pub category: String,
    pub tap: Option<bool>,
    pub natural_scroll: Option<bool>,
    pub middle_emulation: Option<bool>,
    pub disable_while_typing: Option<bool>,
    pub left_handed: Option<bool>,
    pub pointer_speed: Option<f64>,
    pub accel_profile: Option<String>,
    pub scroll_method: Option<String>,
}

/// Règle de fenêtre (§ 8.5)
#[derive(Debug, Clone)]
pub struct WindowRule {
    pub identifier: Option<String>,
    pub title: Option<String>,
    pub actions: Vec<Action>,
    pub server_decoration: Option<bool>,
    pub skip_taskbar: Option<bool>,
    pub skip_window_switcher: Option<bool>,
    pub fixed_position: Option<bool>,
}

/// Correspondance avec jokers `*` et `?`, sans casse
pub fn wildcard(pattern: &str, text: &str) -> bool {
    fn rec(p: &[char], t: &[char]) -> bool {
        match (p.first(), t.first()) {
            (None, None) => true,
            (Some('*'), _) => rec(&p[1..], t) || (!t.is_empty() && rec(p, &t[1..])),
            (Some('?'), Some(_)) => rec(&p[1..], &t[1..]),
            (Some(a), Some(b)) => a == b && rec(&p[1..], &t[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    rec(&p, &t)
}

impl WindowRule {
    pub fn matches(&self, app_id: &str, title: &str) -> bool {
        if self.identifier.is_none() && self.title.is_none() {
            return false;
        }
        self.identifier.as_deref().is_none_or(|p| wildcard(p, app_id))
            && self.title.as_deref().is_none_or(|p| wildcard(p, title))
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub dir: PathBuf,
    // core
    pub server_decoration: bool,
    pub gap: i32,
    pub xwayland_persistence: bool,
    /// Clients X11 rendus à l'échelle de l'écran (nets) au lieu d'être agrandis
    pub xwayland_sharp: bool,
    pub placement: Placement,
    // theme
    pub theme_name: String,
    pub icon_theme: String,
    pub buttons_left: Vec<Button>,
    pub buttons_right: Vec<Button>,
    pub show_title: bool,
    pub corner_radius: i32,
    pub keep_border: bool,
    pub drop_shadows: bool,
    pub drop_shadows_on_tiled: bool,
    pub font_active: Font,
    pub font_inactive: Font,
    pub font_menu_header: Font,
    pub font_menu_item: Font,
    pub font_osd: Font,
    pub margin: Margin,
    pub switcher: Switcher,
    pub screen_edge_strength: i32,
    pub window_edge_strength: i32,
    pub resize_popup: PopupShow,
    pub resize_draw_contents: bool,
    pub follow_mouse: bool,
    pub raise_on_focus: bool,
    pub snap_inner: i32,
    pub snap_outer: i32,
    pub top_maximize: bool,
    pub desktop_names: Vec<String>,
    pub desktop_popup_ms: u32,
    pub regions: Vec<Region>,
    pub numlock: Option<bool>,
    pub layout_per_window: bool,
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    pub keybinds: Vec<Keybind>,
    pub double_click_ms: u32,
    pub mousebinds: Vec<Mousebind>,
    pub input_devices: Vec<InputDevice>,
    pub window_rules: Vec<WindowRule>,
    pub menu_show_icons: bool,
    pub animations: crate::anim::Animations,
    /// Éléments ignorés ou invalides, avec leur ligne
    pub warnings: Vec<String>,
}

/// Souris par défaut, si `rc.xml` n'a pas de section `<mouse>`
const DEFAULT_MOUSE: &str = r#"<mouse>
  <context name="Frame">
    <mousebind button="W-Left" action="Press"><action name="Focus"/><action name="Raise"/></mousebind>
    <mousebind button="W-Left" action="Drag"><action name="Move"/></mousebind>
    <mousebind button="W-Right" action="Press"><action name="Focus"/><action name="Raise"/></mousebind>
    <mousebind button="W-Right" action="Drag"><action name="Resize"/></mousebind>
  </context>
  <context name="Border">
    <mousebind button="Left" action="Press"><action name="Focus"/><action name="Raise"/></mousebind>
    <mousebind button="Left" action="Drag"><action name="Resize"/></mousebind>
  </context>
  <context name="TitleBar">
    <mousebind button="Left" action="Press"><action name="Focus"/><action name="Raise"/></mousebind>
  </context>
  <context name="Title">
    <mousebind button="Left" action="Drag"><action name="Move"/></mousebind>
    <mousebind button="Left" action="DoubleClick"><action name="ToggleMaximize"/></mousebind>
  </context>
  <context name="Maximize"><mousebind button="Left" action="Click"><action name="ToggleMaximize"/></mousebind></context>
  <context name="Iconify"><mousebind button="Left" action="Click"><action name="Iconify"/></mousebind></context>
  <context name="Close"><mousebind button="Left" action="Click"><action name="Close"/></mousebind></context>
  <context name="Shade"><mousebind button="Left" action="Click"><action name="ToggleShade"/></mousebind></context>
  <context name="AllDesktops"><mousebind button="Left" action="Click"><action name="ToggleOmnipresent"/></mousebind></context>
  <context name="Client">
    <mousebind button="Left" action="Press"><action name="Focus"/><action name="Raise"/></mousebind>
    <mousebind button="Middle" action="Press"><action name="Focus"/><action name="Raise"/></mousebind>
    <mousebind button="Right" action="Press"><action name="Focus"/><action name="Raise"/></mousebind>
  </context>
</mouse>"#;

/// Raccourcis par défaut, si `rc.xml` n'en a aucun
const DEFAULT_KEYBOARD: &str = r#"<keyboard>
  <keybind key="A-Tab"><action name="NextWindow"/></keybind>
  <keybind key="A-S-Tab"><action name="PreviousWindow"/></keybind>
  <keybind key="A-F4"><action name="Close"/></keybind>
  <keybind key="W-Return"><action name="Execute" command="kitty"/></keybind>
</keyboard>"#;

impl Default for Config {
    fn default() -> Self {
        let mut c = Config {
            dir: PathBuf::new(),
            server_decoration: true,
            gap: 0,
            xwayland_persistence: false,
            xwayland_sharp: false,
            placement: Placement::Automatic,
            theme_name: String::new(),
            icon_theme: String::new(),
            buttons_left: vec![],
            buttons_right: vec![Button::Iconify, Button::Maximize, Button::Close],
            show_title: true,
            corner_radius: 8,
            keep_border: true,
            drop_shadows: true,
            drop_shadows_on_tiled: false,
            font_active: Font {
                bold: true,
                ..Font::default()
            },
            font_inactive: Font::default(),
            font_menu_header: Font {
                bold: true,
                ..Font::default()
            },
            font_menu_item: Font::default(),
            font_osd: Font::default(),
            margin: Margin::default(),
            switcher: Switcher {
                preview: true,
                outlines: true,
                unshade: true,
                show: true,
                style: "classic".into(),
                fields: vec![],
            },
            screen_edge_strength: 20,
            window_edge_strength: 20,
            resize_popup: PopupShow::Never,
            resize_draw_contents: true,
            follow_mouse: false,
            raise_on_focus: false,
            snap_inner: 10,
            snap_outer: 10,
            top_maximize: true,
            desktop_names: vec!["1".into(), "2".into(), "3".into(), "4".into()],
            desktop_popup_ms: 500,
            regions: vec![],
            numlock: None,
            layout_per_window: false,
            repeat_rate: 25,
            repeat_delay: 600,
            keybinds: vec![],
            double_click_ms: 500,
            mousebinds: vec![],
            input_devices: vec![],
            window_rules: vec![],
            menu_show_icons: true,
            animations: crate::anim::Animations::default(),
            warnings: vec![],
        };
        c.parse_defaults();
        c
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

fn elements<'a, 'i>(node: roxmltree::Node<'a, 'i>) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    node.children().filter(|c| c.is_element())
}

fn text<'a>(node: roxmltree::Node<'a, '_>) -> &'a str {
    node.text().unwrap_or("").trim()
}

fn name(node: roxmltree::Node) -> String {
    node.tag_name().name().to_ascii_lowercase()
}

/// Lecteur d'une section: chaque erreur devient un avertissement
struct Reader<'w> {
    warnings: &'w mut Vec<String>,
}

impl Reader<'_> {
    fn warn(&mut self, node: roxmltree::Node, msg: impl std::fmt::Display) {
        self.warnings.push(format!("ligne {}: {msg}", line(node)));
    }

    fn unknown(&mut self, node: roxmltree::Node, section: &str) {
        let n = node.tag_name().name();
        self.warn(node, format!("<{n}> inconnu dans <{section}>, ignoré"));
    }

    fn bool(&mut self, node: roxmltree::Node, target: &mut bool) {
        match parse_bool(text(node)) {
            Some(b) => *target = b,
            None => self.warn(node, format!("<{}>: booléen attendu", node.tag_name().name())),
        }
    }

    fn opt_bool(&mut self, node: roxmltree::Node) -> Option<bool> {
        let v = parse_bool(text(node));
        if v.is_none() {
            self.warn(node, format!("<{}>: booléen attendu", node.tag_name().name()));
        }
        v
    }

    fn attr_bool(&mut self, node: roxmltree::Node, attr: &str, target: &mut bool) {
        if let Some(v) = node.attribute(attr) {
            match parse_bool(v) {
                Some(b) => *target = b,
                None => self.warn(node, format!("{attr}: booléen attendu")),
            }
        }
    }

    fn int(&mut self, node: roxmltree::Node, target: &mut i32) {
        match text(node).parse() {
            Ok(v) => *target = v,
            Err(_) => self.warn(
                node,
                format!("<{}>: nombre entier attendu", node.tag_name().name()),
            ),
        }
    }

    fn percent(&mut self, node: roxmltree::Node, attr: &str) -> f64 {
        let v = node.attribute(attr).unwrap_or("0").trim();
        match v.trim_end_matches('%').parse::<f64>() {
            Ok(p) => p.clamp(0.0, 100.0),
            Err(_) => {
                self.warn(node, format!("{attr}: pourcentage attendu, pas « {v} »"));
                0.0
            }
        }
    }
}

fn parse_font(r: &mut Reader, node: roxmltree::Node, base: &Font) -> Font {
    let mut font = base.clone();
    for c in elements(node) {
        let v = text(c);
        match name(c).as_str() {
            "name" if !v.is_empty() => font.name = v.to_string(),
            "size" => match v.parse::<f32>() {
                Ok(s) if s > 0.0 => font.size = s,
                _ => r.warn(c, format!("taille de police invalide: {v}")),
            },
            "weight" => font.bold = eq(v, "bold"),
            "slant" => font.italic = eq(v, "italic") || eq(v, "oblique"),
            _ => r.unknown(c, "font"),
        }
    }
    font
}

impl Config {
    /// Lit `rc.xml` du dossier. `Err` si le fichier est illisible ou n'est
    /// pas du XML valide (le rechargement garde alors l'ancienne
    /// configuration)
    pub fn load(dir: &Path) -> Result<Config, String> {
        let mut config = Config {
            dir: dir.to_path_buf(),
            ..Config::default()
        };
        let path = dir.join("rc.xml");
        let xml = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        config
            .parse(&xml)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        for w in &config.warnings {
            warn!("rc.xml, {w}");
        }
        Ok(config)
    }

    /// Configuration par défaut d'un dossier, si `rc.xml` est absent
    pub fn fallback(dir: &Path) -> Config {
        Config {
            dir: dir.to_path_buf(),
            ..Config::default()
        }
    }

    fn parse_defaults(&mut self) {
        for xml in [DEFAULT_MOUSE, DEFAULT_KEYBOARD] {
            let doc = roxmltree::Document::parse(xml).unwrap();
            let mut warnings = vec![];
            let mut r = Reader {
                warnings: &mut warnings,
            };
            let root = doc.root_element();
            if eq(root.tag_name().name(), "mouse") {
                self.parse_mouse(&mut r, root);
            } else {
                self.parse_keyboard(&mut r, root);
            }
            debug_assert!(warnings.is_empty(), "{warnings:?}");
        }
    }

    pub fn parse(&mut self, xml: &str) -> Result<(), String> {
        let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
        let root = doc.root_element();
        let root_name = root.tag_name().name();
        if !["packarch_config", "openbox_config", "labwc_config"]
            .iter()
            .any(|r| eq(root_name, r))
        {
            return Err(format!("racine <{root_name}> inconnue"));
        }
        let mut warnings = vec![];
        let mut r = Reader {
            warnings: &mut warnings,
        };
        for section in elements(root) {
            match name(section).as_str() {
                "core" => self.parse_core(&mut r, section),
                "placement" => {
                    for c in elements(section) {
                        match name(c).as_str() {
                            "policy" => match Placement::parse(text(c)) {
                                Some(p) => self.placement = p,
                                None => r.warn(c, format!("politique de placement inconnue: {}", text(c))),
                            },
                            _ => r.unknown(c, "placement"),
                        }
                    }
                }
                "theme" => self.parse_theme(&mut r, section),
                "margin" => {
                    for c in elements(section) {
                        match name(c).as_str() {
                            "top" => r.int(c, &mut self.margin.top),
                            "bottom" => r.int(c, &mut self.margin.bottom),
                            "left" => r.int(c, &mut self.margin.left),
                            "right" => r.int(c, &mut self.margin.right),
                            _ => r.unknown(c, "margin"),
                        }
                    }
                }
                "windowswitcher" => self.parse_switcher(&mut r, section),
                "resistance" => {
                    for c in elements(section) {
                        match name(c).as_str() {
                            "screenedgestrength" => r.int(c, &mut self.screen_edge_strength),
                            "windowedgestrength" => r.int(c, &mut self.window_edge_strength),
                            _ => r.unknown(c, "resistance"),
                        }
                    }
                }
                "resize" => {
                    for c in elements(section) {
                        match name(c).as_str() {
                            "popupshow" => match text(c).to_ascii_lowercase().as_str() {
                                "never" => self.resize_popup = PopupShow::Never,
                                "always" => self.resize_popup = PopupShow::Always,
                                "nonpixel" => self.resize_popup = PopupShow::Nonpixel,
                                v => r.warn(c, format!("popupShow inconnu: {v}")),
                            },
                            "drawcontents" => r.bool(c, &mut self.resize_draw_contents),
                            _ => r.unknown(c, "resize"),
                        }
                    }
                }
                "focus" => {
                    for c in elements(section) {
                        match name(c).as_str() {
                            "followmouse" => r.bool(c, &mut self.follow_mouse),
                            "raiseonfocus" => r.bool(c, &mut self.raise_on_focus),
                            _ => r.unknown(c, "focus"),
                        }
                    }
                }
                "snapping" => {
                    for c in elements(section) {
                        match name(c).as_str() {
                            "range" => {
                                // <range>10</range> ou <range inner="" outer=""/>
                                if let Ok(v) = text(c).parse::<i32>() {
                                    self.snap_inner = v;
                                    self.snap_outer = v;
                                }
                                for (attr, target) in
                                    [("inner", &mut self.snap_inner), ("outer", &mut self.snap_outer)]
                                {
                                    if let Some(v) = c.attribute(attr) {
                                        match v.parse() {
                                            Ok(n) => *target = n,
                                            Err(_) => r.warn(c, format!("{attr}: nombre attendu")),
                                        }
                                    }
                                }
                            }
                            "topmaximize" => r.bool(c, &mut self.top_maximize),
                            _ => r.unknown(c, "snapping"),
                        }
                    }
                }
                "desktops" => self.parse_desktops(&mut r, section),
                "regions" => {
                    self.regions.clear();
                    for c in elements(section) {
                        if name(c) != "region" {
                            r.unknown(c, "regions");
                            continue;
                        }
                        let Some(n) = c.attribute("name") else {
                            r.warn(c, "région sans nom");
                            continue;
                        };
                        let region = Region {
                            name: n.to_string(),
                            x: r.percent(c, "x"),
                            y: r.percent(c, "y"),
                            width: r.percent(c, "width"),
                            height: r.percent(c, "height"),
                        };
                        self.regions.push(region);
                    }
                }
                "keyboard" => self.parse_keyboard(&mut r, section),
                "mouse" => self.parse_mouse(&mut r, section),
                "libinput" => self.parse_libinput(&mut r, section),
                "windowrules" => self.parse_rules(&mut r, section),
                "animations" => self.parse_animations(&mut r, section),
                "menu" => {
                    for c in elements(section) {
                        match name(c).as_str() {
                            "showicons" => r.bool(c, &mut self.menu_show_icons),
                            _ => r.unknown(c, "menu"),
                        }
                    }
                }
                _ => r.unknown(section, root_name),
            }
        }
        self.warnings = warnings;
        Ok(())
    }

    fn parse_core(&mut self, r: &mut Reader, section: roxmltree::Node) {
        for c in elements(section) {
            match name(c).as_str() {
                "decoration" => match text(c).to_ascii_lowercase().as_str() {
                    "server" => self.server_decoration = true,
                    "client" => self.server_decoration = false,
                    v => r.warn(c, format!("decoration: server ou client attendu, pas « {v} »")),
                },
                "gap" => r.int(c, &mut self.gap),
                "xwaylandpersistence" => r.bool(c, &mut self.xwayland_persistence),
                "xwaylandsharp" => r.bool(c, &mut self.xwayland_sharp),
                "adaptivesync" | "reuseoutputmode" | "allowtearing" => {}
                _ => r.unknown(c, "core"),
            }
        }
    }

    fn parse_theme(&mut self, r: &mut Reader, theme: roxmltree::Node) {
        for c in elements(theme) {
            let v = text(c);
            match name(c).as_str() {
                "name" => self.theme_name = v.to_string(),
                "icon" => self.icon_theme = v.to_string(),
                "cornerradius" => match v.parse::<i32>() {
                    Ok(x) if x >= 0 => self.corner_radius = x,
                    _ => r.warn(c, format!("cornerRadius invalide: {v}")),
                },
                "keepborder" => r.bool(c, &mut self.keep_border),
                "dropshadows" => r.bool(c, &mut self.drop_shadows),
                "dropshadowsontiled" => r.bool(c, &mut self.drop_shadows_on_tiled),
                "titlebar" => {
                    for t in elements(c) {
                        match name(t).as_str() {
                            "layout" => (self.buttons_left, self.buttons_right) = parse_layout(text(t)),
                            "showtitle" => r.bool(t, &mut self.show_title),
                            _ => r.unknown(t, "titlebar"),
                        }
                    }
                }
                "font" => {
                    let place = c.attribute("place").unwrap_or("").to_ascii_lowercase();
                    match place.as_str() {
                        "" => {
                            for f in [
                                &mut self.font_active,
                                &mut self.font_inactive,
                                &mut self.font_menu_header,
                                &mut self.font_menu_item,
                                &mut self.font_osd,
                            ] {
                                *f = parse_font(r, c, f);
                            }
                        }
                        "activewindow" => self.font_active = parse_font(r, c, &self.font_active),
                        "inactivewindow" => self.font_inactive = parse_font(r, c, &self.font_inactive),
                        "menuheader" => self.font_menu_header = parse_font(r, c, &self.font_menu_header),
                        "menuitem" => self.font_menu_item = parse_font(r, c, &self.font_menu_item),
                        "onscreendisplay" | "osd" => self.font_osd = parse_font(r, c, &self.font_osd),
                        p => r.warn(c, format!("place de police inconnue: {p}")),
                    }
                }
                _ => r.unknown(c, "theme"),
            }
        }
    }

    fn parse_switcher(&mut self, r: &mut Reader, section: roxmltree::Node) {
        let s = &mut self.switcher;
        r.attr_bool(section, "preview", &mut s.preview);
        r.attr_bool(section, "outlines", &mut s.outlines);
        r.attr_bool(section, "unshade", &mut s.unshade);
        for c in elements(section) {
            match name(c).as_str() {
                "osd" => {
                    r.attr_bool(c, "show", &mut s.show);
                    if let Some(st) = c.attribute("style") {
                        s.style = st.to_string();
                    }
                }
                "fields" => {
                    s.fields.clear();
                    for f in elements(c) {
                        if name(f) != "field" {
                            r.unknown(f, "fields");
                            continue;
                        }
                        let width = f
                            .attribute("width")
                            .and_then(|w| w.trim_end_matches('%').parse().ok())
                            .unwrap_or(0.0);
                        s.fields.push(SwitcherField {
                            content: f.attribute("content").unwrap_or("title").to_string(),
                            width,
                        });
                    }
                }
                _ => r.unknown(c, "windowSwitcher"),
            }
        }
    }

    fn parse_desktops(&mut self, r: &mut Reader, section: roxmltree::Node) {
        let mut number = None;
        for c in elements(section) {
            match name(c).as_str() {
                "popuptime" => {
                    let mut v = self.desktop_popup_ms as i32;
                    r.int(c, &mut v);
                    self.desktop_popup_ms = v.max(0) as u32;
                }
                "names" => {
                    self.desktop_names = elements(c).map(|n| text(n).to_string()).collect();
                }
                "number" => {
                    let mut v = 0;
                    r.int(c, &mut v);
                    number = Some(v.clamp(1, 32) as usize);
                }
                _ => r.unknown(c, "desktops"),
            }
        }
        if let Some(n) = number {
            while self.desktop_names.len() < n {
                self.desktop_names
                    .push((self.desktop_names.len() + 1).to_string());
            }
            self.desktop_names.truncate(n);
        }
        if self.desktop_names.is_empty() {
            self.desktop_names.push("1".into());
        }
    }

    fn parse_keyboard(&mut self, r: &mut Reader, section: roxmltree::Node) {
        let mut binds = vec![];
        for c in elements(section) {
            match name(c).as_str() {
                "numlock" => self.numlock = r.opt_bool(c),
                "layoutscope" => self.layout_per_window = eq(text(c), "window"),
                "repeatrate" => r.int(c, &mut self.repeat_rate),
                "repeatdelay" => r.int(c, &mut self.repeat_delay),
                "default" => {}
                "keybind" => {
                    let Some(key) = c.attribute("key") else {
                        r.warn(c, "raccourci sans touche");
                        continue;
                    };
                    let parsed = parse_combo(key).and_then(|(mods, k)| {
                        parse_keysym(k)
                            .map(|sym| (mods, sym))
                            .ok_or_else(|| format!("touche inconnue: {k}"))
                    });
                    match parsed {
                        Ok((mods, sym)) => {
                            let mut allow = false;
                            r.attr_bool(c, "allowWhenLocked", &mut allow);
                            let actions = actions::parse_children(c, r.warnings);
                            binds.push(Keybind {
                                mods,
                                key: sym,
                                allow_when_locked: allow,
                                actions,
                            });
                        }
                        Err(e) => r.warn(c, e),
                    }
                }
                _ => r.unknown(c, "keyboard"),
            }
        }
        if !binds.is_empty() {
            self.keybinds = binds;
        }
    }

    fn parse_mouse(&mut self, r: &mut Reader, section: roxmltree::Node) {
        let mut binds = vec![];
        for c in elements(section) {
            match name(c).as_str() {
                "doubleclicktime" => {
                    let mut v = self.double_click_ms as i32;
                    r.int(c, &mut v);
                    self.double_click_ms = v.max(0) as u32;
                }
                "dragthreshold" | "scrollfactor" | "default" => {}
                "context" => {
                    let Some(ctx) = c.attribute("name").and_then(Context::parse) else {
                        r.warn(c, format!("contexte inconnu: {:?}", c.attribute("name")));
                        continue;
                    };
                    for m in elements(c) {
                        if name(m) != "mousebind" {
                            r.unknown(m, "context");
                            continue;
                        }
                        let actions = actions::parse_children(m, r.warnings);
                        match Mousebind::parse(
                            ctx,
                            m.attribute("button"),
                            m.attribute("direction"),
                            m.attribute("action").unwrap_or("Press"),
                            actions,
                        ) {
                            Ok(b) => binds.push(b),
                            Err(e) => r.warn(m, e),
                        }
                    }
                }
                _ => r.unknown(c, "mouse"),
            }
        }
        if !binds.is_empty() {
            self.mousebinds = binds;
        }
    }

    fn parse_libinput(&mut self, r: &mut Reader, section: roxmltree::Node) {
        for d in elements(section) {
            if name(d) != "device" {
                r.unknown(d, "libinput");
                continue;
            }
            let mut dev = InputDevice {
                category: d.attribute("category").unwrap_or("default").to_string(),
                ..Default::default()
            };
            for c in elements(d) {
                match name(c).as_str() {
                    "tap" => dev.tap = r.opt_bool(c),
                    "naturalscroll" => dev.natural_scroll = r.opt_bool(c),
                    "middleemulation" => dev.middle_emulation = r.opt_bool(c),
                    "disablewhiletyping" => dev.disable_while_typing = r.opt_bool(c),
                    "lefthanded" => dev.left_handed = r.opt_bool(c),
                    "pointerspeed" => dev.pointer_speed = text(c).parse().ok(),
                    "accelprofile" => dev.accel_profile = Some(text(c).to_string()),
                    "scrollmethod" => dev.scroll_method = Some(text(c).to_string()),
                    "tapbuttonmap" | "tapanddrag" | "draglock" | "threefingerdrag" | "clickmethod"
                    | "sendeventsmode" | "calibrationmatrix" | "scrollfactor" => {}
                    _ => r.unknown(c, "device"),
                }
            }
            self.input_devices.push(dev);
        }
    }

    fn parse_rules(&mut self, r: &mut Reader, section: roxmltree::Node) {
        for c in elements(section) {
            if name(c) != "windowrule" {
                r.unknown(c, "windowRules");
                continue;
            }
            let attr_bool = |r: &mut Reader, a: &str| {
                c.attribute(a).and_then(|v| {
                    let b = parse_bool(v);
                    if b.is_none() {
                        r.warn(c, format!("{a}: booléen attendu"));
                    }
                    b
                })
            };
            let rule = WindowRule {
                identifier: c.attribute("identifier").map(str::to_string),
                title: c.attribute("title").map(str::to_string),
                actions: actions::parse_children(c, r.warnings),
                server_decoration: attr_bool(r, "serverDecoration"),
                skip_taskbar: attr_bool(r, "skipTaskbar"),
                skip_window_switcher: attr_bool(r, "skipWindowSwitcher"),
                fixed_position: attr_bool(r, "fixedPosition"),
            };
            if rule.identifier.is_none() && rule.title.is_none() {
                r.warn(c, "règle sans identifier ni title, ignorée");
                continue;
            }
            self.window_rules.push(rule);
        }
    }

    /// `<animations enabled="yes" speed="1.0">` et un élément par animation
    /// (`<window-open duration="160" curve="spring" stiffness="600"
    /// damping="40"/>`, `<menu enabled="no"/>`) (§ 10.5)
    fn parse_animations(&mut self, r: &mut Reader, section: roxmltree::Node) {
        use crate::anim::{Curve, Kind};
        let a = &mut self.animations;
        r.attr_bool(section, "enabled", &mut a.enabled);
        if let Some(v) = section.attribute("speed") {
            match v.trim().parse::<f64>() {
                Ok(s) if s >= 0.0 => a.speed = s,
                _ => r.warn(section, format!("speed: nombre positif attendu, pas « {v} »")),
            }
        }
        for c in elements(section) {
            let Some(kind) = Kind::parse(c.tag_name().name()) else {
                r.unknown(c, "animations");
                continue;
            };
            let mut s = *a.setting_mut(kind);
            r.attr_bool(c, "enabled", &mut s.enabled);
            if let Some(v) = c.attribute("duration") {
                match v.trim().parse::<u32>() {
                    Ok(ms) => s.duration_ms = ms.min(5000),
                    Err(_) => r.warn(c, format!("duration: millisecondes attendues, pas « {v} »")),
                }
            }
            if let Some(v) = c.attribute("curve") {
                match Curve::parse(v) {
                    Some(curve) => s.curve = curve,
                    None => r.warn(c, format!("courbe inconnue: {v}")),
                }
            }
            if let Curve::Spring { stiffness, damping } = &mut s.curve {
                for (attr, target) in [("stiffness", stiffness), ("damping", damping)] {
                    if let Some(v) = c.attribute(attr) {
                        match v.trim().parse::<f64>() {
                            Ok(x) if x > 0.0 => *target = x,
                            _ => r.warn(c, format!("{attr}: nombre positif attendu")),
                        }
                    }
                }
            }
            *a.setting_mut(kind) = s;
        }
    }

    pub fn region(&self, name: &str) -> Option<&Region> {
        self.regions.iter().find(|r| r.name == name)
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
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        for root in ["packarch_config", "openbox_config"] {
            let mut c = Config::default();
            c.parse(&format!("<{root}/>")).unwrap();
        }
        assert!(Config::default().parse("<sway/>").is_err());
    }

    #[test]
    fn defaults() {
        let c = Config::default();
        assert!(!c.mousebinds.is_empty());
        assert!(!c.keybinds.is_empty());
    }

    #[test]
    fn warnings() {
        let mut c = Config::default();
        c.parse(
            r#"<packarch_config>
            <focus><followMouse>peut-être</followMouse><vole/></focus>
            <keyboard><keybind key="W-pasunetouche"><action name="Close"/></keybind>
            <keybind key="W-x"><action name="Inconnue"/></keybind></keyboard>
            <bidule/>
            </packarch_config>"#,
        )
        .unwrap();
        assert_eq!(c.warnings.len(), 5, "{:?}", c.warnings);
        assert!(c.warnings[0].starts_with("ligne 2:"));
    }

    #[test]
    fn sections() {
        let mut c = Config::default();
        c.parse(
            r#"<packarch_config>
            <core><xwaylandSharp>yes</xwaylandSharp></core>
            <desktops><popupTime>300</popupTime><names><name>Un</name><name>Deux</name></names></desktops>
            <regions><region name="gauche" x="0%" y="0%" width="50%" height="100%"/></regions>
            <snapping><range inner="5" outer="15"/><topMaximize>no</topMaximize></snapping>
            <windowRules><windowRule identifier="yad" title="Thèmes*"><action name="AutoPlace" policy="center"/></windowRule></windowRules>
            </packarch_config>"#,
        )
        .unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert!(c.xwayland_sharp);
        assert_eq!(c.desktop_names, vec!["Un", "Deux"]);
        assert_eq!(c.desktop_popup_ms, 300);
        assert_eq!(c.region("gauche").unwrap().width, 50.0);
        assert_eq!((c.snap_inner, c.snap_outer, c.top_maximize), (5, 15, false));
        assert!(c.window_rules[0].matches("yad", "Thèmes Packarch"));
        assert!(!c.window_rules[0].matches("kitty", "Thèmes Packarch"));
    }

    #[test]
    fn animations() {
        use crate::anim::{Curve, Kind};
        let mut c = Config::default();
        c.parse(
            r#"<packarch_config><animations enabled="yes" speed="0.5">
            <window-open duration="200" curve="linear"/>
            <workspace-switch curve="spring" stiffness="300"/>
            <menu enabled="no"/><vole/>
            </animations></packarch_config>"#,
        )
        .unwrap();
        assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
        let a = &mut c.animations;
        assert_eq!(a.speed, 0.5);
        assert_eq!(a.setting_mut(Kind::WindowOpen).duration_ms, 200);
        assert_eq!(a.setting_mut(Kind::WindowOpen).curve, Curve::Linear);
        assert!(matches!(
            a.setting_mut(Kind::WorkspaceSwitch).curve,
            Curve::Spring { stiffness, .. } if stiffness == 300.0
        ));
        assert!(!a.setting_mut(Kind::Menu).enabled);
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("*fire*", "Firefox"));
        assert!(wildcard("a?c", "abc"));
        assert!(!wildcard("a?c", "ac"));
        assert!(wildcard("*", ""));
    }

    #[test]
    fn packarch_rc() {
        // Le rc.xml installé par Packarch, s'il est présent: aucun avertissement
        let path = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/labwc/rc.xml"));
        if let Some(xml) = path.and_then(|p| std::fs::read_to_string(p).ok()) {
            let mut c = Config::default();
            c.parse(&xml).unwrap();
            assert!(!c.theme_name.is_empty());
            assert!(c.warnings.is_empty(), "{:#?}", c.warnings);
            assert!(c.keybinds.len() > 80, "{}", c.keybinds.len());
            assert!(c.mousebinds.len() > 30, "{}", c.mousebinds.len());
            assert_eq!(c.regions.len(), 9);
            assert_eq!(c.desktop_names.len(), 4);
        }
    }
}
