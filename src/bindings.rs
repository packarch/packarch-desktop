// SPDX-License-Identifier: MIT OR Apache-2.0
//! Raccourcis clavier et liaisons de la souris (cahier des charges § 5.3,
//! § 5.4): lecture et correspondance avec les événements.

use smithay::input::keyboard::{Keysym, ModifiersState, xkb};

use crate::actions::Action;

/// Modificateurs d'une liaison: W (Super), A (Alt), C (Ctrl), S (Maj),
/// M (Mod5, AltGr), H (Hyper, traité comme Super)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Mods {
    pub logo: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub mod5: bool,
}

impl Mods {
    pub fn from_state(m: &ModifiersState) -> Mods {
        Mods {
            logo: m.logo,
            alt: m.alt,
            ctrl: m.ctrl,
            shift: m.shift,
            mod5: m.iso_level3_shift,
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Mods::default()
    }
}

/// Sépare `W-S-Return` en modificateurs et nom de touche (ou de bouton)
pub fn parse_combo(s: &str) -> Result<(Mods, &str), String> {
    let mut mods = Mods::default();
    let mut rest = s.trim();
    while let Some((m, tail)) = rest.split_once('-') {
        if m.len() != 1 || tail.is_empty() {
            break;
        }
        match m {
            "W" | "H" => mods.logo = true,
            "A" => mods.alt = true,
            "C" => mods.ctrl = true,
            "S" => mods.shift = true,
            "M" => mods.mod5 = true,
            _ => return Err(format!("modificateur inconnu « {m} » dans « {s} »")),
        }
        rest = tail;
    }
    if rest.is_empty() {
        return Err(format!("touche manquante dans « {s} »"));
    }
    Ok((mods, rest))
}

/// Keysym d'un nom de touche (`Return`, `KP_5`, `a`, `1`), en minuscule
pub fn parse_keysym(name: &str) -> Option<Keysym> {
    let sym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
    let sym = if sym.raw() == 0 {
        xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE)
    } else {
        sym
    };
    (sym.raw() != 0).then(|| lower(sym))
}

/// Minuscule d'un keysym de lettre
pub fn lower(sym: Keysym) -> Keysym {
    match sym.key_char() {
        Some(c) if c.is_uppercase() => {
            let l = c.to_lowercase().next().unwrap_or(c);
            let k = xkb::utf32_to_keysym(l as u32);
            if k.raw() == 0 { sym } else { k }
        }
        _ => sym,
    }
}

#[derive(Debug, Clone)]
pub struct Keybind {
    pub mods: Mods,
    pub key: Keysym,
    /// Actif pendant le verrouillage (ext-session-lock, 0.4)
    #[allow(dead_code)]
    pub allow_when_locked: bool,
    pub actions: Vec<Action>,
}

/// Trouve le raccourci d'une touche. D'abord par le keysym de la disposition
/// active (avec et sans les modificateurs), puis par la touche physique
/// (keysym de la disposition « us » au premier niveau): `W-1` marche sur un
/// clavier AZERTY sans Maj, et `W-q` reste la touche Q.
pub fn find_keybind<'a>(
    binds: &'a [Keybind],
    mods: Mods,
    translated: &[Keysym],
    physical: Option<Keysym>,
) -> Option<&'a Keybind> {
    let same_mods = |b: &&Keybind| b.mods == mods;
    for sym in translated {
        let sym = lower(*sym);
        if let Some(b) = binds.iter().filter(same_mods).find(|b| b.key == sym) {
            return Some(b);
        }
    }
    let physical = lower(physical?);
    binds.iter().filter(same_mods).find(|b| b.key == physical)
}

/// Bouton de la souris
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Side,
    Extra,
}

impl MouseButton {
    pub fn from_code(code: u32) -> Option<MouseButton> {
        Some(match code {
            0x110 => MouseButton::Left,
            0x111 => MouseButton::Right,
            0x112 => MouseButton::Middle,
            0x113 => MouseButton::Side,
            0x114 => MouseButton::Extra,
            _ => return None,
        })
    }

    fn parse(s: &str) -> Option<MouseButton> {
        Some(match s.to_ascii_lowercase().as_str() {
            "left" => MouseButton::Left,
            "middle" => MouseButton::Middle,
            "right" => MouseButton::Right,
            "side" => MouseButton::Side,
            "extra" => MouseButton::Extra,
            _ => return None,
        })
    }
}

/// Sens de la molette
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDir {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    Press,
    Release,
    Click,
    DoubleClick,
    Drag,
    Scroll,
}

/// Zone d'un clic (§ 5.4)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Frame,
    Client,
    Title,
    TitleBar,
    Border,
    Top,
    Bottom,
    Left,
    Right,
    TLCorner,
    TRCorner,
    BLCorner,
    BRCorner,
    Icon,
    WindowMenu,
    Iconify,
    Maximize,
    Close,
    Shade,
    AllDesktops,
    Root,
}

impl Context {
    pub fn parse(s: &str) -> Option<Context> {
        use Context::*;
        Some(match s.to_ascii_lowercase().as_str() {
            "frame" => Frame,
            "client" => Client,
            "title" => Title,
            "titlebar" => TitleBar,
            "border" => Border,
            "top" => Top,
            "bottom" => Bottom,
            "left" => Left,
            "right" => Right,
            "tlcorner" => TLCorner,
            "trcorner" => TRCorner,
            "blcorner" => BLCorner,
            "brcorner" => BRCorner,
            "icon" => Icon,
            "windowmenu" | "menu" => WindowMenu,
            "iconify" => Iconify,
            "maximize" => Maximize,
            "close" => Close,
            "shade" => Shade,
            "alldesktops" | "desk" => AllDesktops,
            "root" | "desktop" => Root,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Mousebind {
    pub context: Context,
    pub mods: Mods,
    /// Bouton (sauf molette)
    pub button: Option<MouseButton>,
    pub direction: Option<ScrollDir>,
    pub event: MouseEvent,
    pub actions: Vec<Action>,
}

impl Mousebind {
    /// `<mousebind button="W-Left" action="Drag">` ou
    /// `<mousebind direction="Up" action="Scroll">`
    pub fn parse(
        context: Context,
        button: Option<&str>,
        direction: Option<&str>,
        event: &str,
        actions: Vec<Action>,
    ) -> Result<Mousebind, String> {
        let event = match event.to_ascii_lowercase().as_str() {
            "press" => MouseEvent::Press,
            "release" => MouseEvent::Release,
            "click" => MouseEvent::Click,
            "doubleclick" => MouseEvent::DoubleClick,
            "drag" => MouseEvent::Drag,
            "scroll" => MouseEvent::Scroll,
            other => return Err(format!("événement de souris inconnu: {other}")),
        };
        let mut mods = Mods::default();
        let mut btn = None;
        let mut dir = None;
        if let Some(b) = button {
            let (m, name) = parse_combo(b)?;
            mods = m;
            // Openbox: la molette peut aussi s'écrire button="Up"
            match MouseButton::parse(name) {
                Some(x) => btn = Some(x),
                None => dir = Some(parse_dir(name).ok_or_else(|| format!("bouton inconnu: {name}"))?),
            }
        }
        if let Some(d) = direction {
            let (m, name) = parse_combo(d)?;
            mods = m;
            dir = Some(parse_dir(name).ok_or_else(|| format!("direction inconnue: {name}"))?);
        }
        if event == MouseEvent::Scroll && dir.is_none() {
            return Err("molette sans direction".into());
        }
        if event != MouseEvent::Scroll && btn.is_none() {
            return Err("liaison de souris sans bouton".into());
        }
        Ok(Mousebind {
            context,
            mods,
            button: btn,
            direction: dir,
            event,
            actions,
        })
    }
}

fn parse_dir(s: &str) -> Option<ScrollDir> {
    Some(match s.to_ascii_lowercase().as_str() {
        "up" => ScrollDir::Up,
        "down" => ScrollDir::Down,
        "left" => ScrollDir::Left,
        "right" => ScrollDir::Right,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos() {
        let (m, k) = parse_combo("W-S-Return").unwrap();
        assert!(m.logo && m.shift && !m.alt);
        assert_eq!(k, "Return");
        let (m, k) = parse_combo("A-F4").unwrap();
        assert!(m.alt);
        assert_eq!(k, "F4");
        let (m, k) = parse_combo("minus").unwrap();
        assert!(m.is_empty());
        assert_eq!(k, "minus");
        // Le tiret seul est une touche
        let (m, k) = parse_combo("C--").unwrap();
        assert!(m.ctrl);
        assert_eq!(k, "-");
        assert!(parse_combo("X-a").is_err());
    }

    #[test]
    fn keysyms() {
        assert_eq!(parse_keysym("Return"), Some(Keysym::Return));
        assert_eq!(parse_keysym("a"), Some(Keysym::a));
        assert_eq!(parse_keysym("A"), Some(Keysym::a));
        assert_eq!(parse_keysym("KP_5"), Some(Keysym::KP_5));
        assert_eq!(parse_keysym("XF86AudioMute"), Some(Keysym::XF86_AudioMute));
        assert_eq!(parse_keysym("return"), Some(Keysym::Return));
        assert_eq!(parse_keysym("pasunetouche"), None);
    }

    fn bind(combo: &str) -> Keybind {
        let (mods, key) = parse_combo(combo).unwrap();
        Keybind {
            mods,
            key: parse_keysym(key).unwrap(),
            allow_when_locked: false,
            actions: vec![],
        }
    }

    #[test]
    fn azerty() {
        let binds = vec![bind("W-1"), bind("W-S-1"), bind("W-q"), bind("W-a")];
        let logo = Mods {
            logo: true,
            ..Mods::default()
        };
        let logo_shift = Mods {
            logo: true,
            shift: true,
            ..Mods::default()
        };
        // Touche « 1 » d'un clavier AZERTY: & sans Maj, 1 avec Maj
        let b = find_keybind(&binds, logo, &[Keysym::ampersand], Some(Keysym::_1)).unwrap();
        assert_eq!(b.key, Keysym::_1);
        assert!(!b.mods.shift);
        let b = find_keybind(
            &binds,
            logo_shift,
            &[Keysym::_1, Keysym::ampersand],
            Some(Keysym::_1),
        )
        .unwrap();
        assert!(b.mods.shift);
        // Touche A d'un AZERTY (Q physique en QWERTY): c'est W-a
        let b = find_keybind(&binds, logo, &[Keysym::a], Some(Keysym::q)).unwrap();
        assert_eq!(b.key, Keysym::a);
        // Majuscule produite par Maj: comparée en minuscule
        let binds = vec![bind("W-S-e")];
        assert!(find_keybind(&binds, logo_shift, &[Keysym::E], Some(Keysym::e)).is_some());
        assert!(find_keybind(&binds, logo, &[Keysym::e], Some(Keysym::e)).is_none());
    }

    #[test]
    fn mousebinds() {
        let m = Mousebind::parse(Context::Frame, Some("W-Left"), None, "Drag", vec![]).unwrap();
        assert!(m.mods.logo);
        assert_eq!(m.button, Some(MouseButton::Left));
        assert_eq!(m.event, MouseEvent::Drag);
        let m = Mousebind::parse(Context::Root, None, Some("Up"), "Scroll", vec![]).unwrap();
        assert_eq!(m.direction, Some(ScrollDir::Up));
        let m = Mousebind::parse(Context::Root, Some("Down"), None, "Scroll", vec![]).unwrap();
        assert_eq!(m.direction, Some(ScrollDir::Down));
        assert!(Mousebind::parse(Context::Root, Some("Left"), None, "Tap", vec![]).is_err());
    }
}
