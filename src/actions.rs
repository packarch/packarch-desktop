// SPDX-License-Identifier: MIT OR Apache-2.0
//! Actions (cahier des charges § 7): structures typées, construites à la
//! lecture de la configuration. Leur exécution est dans `wm.rs`.

use crate::config::{Placement, parse_bool};
use crate::view::{Axis, Edge};

/// Bureau visé par GoToDesktop et SendToDesktop
#[derive(Debug, Clone, PartialEq)]
pub enum DesktopTarget {
    /// Numéro, à partir de 0
    Index(usize),
    Left,
    Right,
    Last,
    Current,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Execute(String),
    ShowMenu {
        menu: String,
        at_cursor: bool,
    },
    Focus,
    Unfocus,
    Raise,
    Lower,
    Close,
    Kill,
    Iconify,
    Maximize(Axis),
    UnMaximize(Axis),
    ToggleMaximize(Axis),
    ToggleFullscreen,
    ToggleDecorations,
    Shade,
    Unshade,
    ToggleShade,
    ToggleAlwaysOnTop,
    ToggleAlwaysOnBottom,
    ToggleOmnipresent,
    Move,
    Resize,
    MoveRelative {
        x: i32,
        y: i32,
    },
    ResizeRelative {
        left: i32,
        right: i32,
        top: i32,
        bottom: i32,
    },
    SnapToEdge(Edge),
    UnSnap,
    SnapToRegion(String),
    GoToDesktop {
        to: DesktopTarget,
        wrap: bool,
    },
    SendToDesktop {
        to: DesktopTarget,
        follow: bool,
        wrap: bool,
    },
    NextWindow,
    PreviousWindow,
    ToggleShowDesktop,
    Reconfigure,
    Exit,
    AutoPlace(Option<Placement>),
    None,
}

/// Attributs d'un élément `<action>` (ou de ses enfants, à la Openbox:
/// `<action name="Execute"><command>…</command></action>`)
pub struct Attrs<'a, 'i> {
    node: roxmltree::Node<'a, 'i>,
}

impl<'a> Attrs<'a, '_> {
    fn get(&self, name: &str) -> Option<&'a str> {
        if let Some(v) = self
            .node
            .attributes()
            .find(|a| a.name().eq_ignore_ascii_case(name))
        {
            return Some(v.value());
        }
        self.node
            .children()
            .find(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case(name))
            .and_then(|c| c.text())
            .map(str::trim)
    }

    fn bool(&self, name: &str, default: bool) -> Result<bool, String> {
        match self.get(name) {
            None => Ok(default),
            Some(v) => parse_bool(v).ok_or_else(|| format!("{name}: booléen attendu, pas « {v} »")),
        }
    }

    fn int(&self, name: &str) -> Result<i32, String> {
        match self.get(name) {
            None => Ok(0),
            Some(v) => v
                .trim()
                .parse()
                .map_err(|_| format!("{name}: nombre attendu, pas « {v} »")),
        }
    }

    fn axis(&self) -> Result<Axis, String> {
        match self.get("direction").map(str::to_ascii_lowercase).as_deref() {
            None | Some("both") => Ok(Axis::Both),
            Some("horizontal") => Ok(Axis::Horizontal),
            Some("vertical") => Ok(Axis::Vertical),
            Some(v) => Err(format!("direction inconnue: {v}")),
        }
    }

    fn desktop(&self) -> Result<DesktopTarget, String> {
        let v = self.get("to").ok_or("attribut « to » manquant")?;
        Ok(match v.to_ascii_lowercase().as_str() {
            "left" => DesktopTarget::Left,
            "right" => DesktopTarget::Right,
            "last" => DesktopTarget::Last,
            "current" => DesktopTarget::Current,
            n => match n.parse::<usize>() {
                Ok(i) if i >= 1 => DesktopTarget::Index(i - 1),
                _ => return Err(format!("bureau inconnu: {v}")),
            },
        })
    }
}

/// Construit une action depuis un élément `<action name="…">`
pub fn parse(node: roxmltree::Node) -> Result<Action, String> {
    let a = Attrs { node };
    let name = a.get("name").ok_or("action sans nom")?;
    use Action::*;
    Ok(match name.to_ascii_lowercase().as_str() {
        "execute" => Execute(
            a.get("command")
                .or_else(|| a.get("execute"))
                .ok_or("Execute sans commande")?
                .to_string(),
        ),
        "showmenu" => ShowMenu {
            menu: a.get("menu").ok_or("ShowMenu sans menu")?.to_string(),
            at_cursor: a.bool("atCursor", true)?,
        },
        "focus" => Focus,
        "unfocus" => Unfocus,
        "raise" => Raise,
        "lower" => Lower,
        "close" => Close,
        "kill" => Kill,
        "iconify" => Iconify,
        "maximize" => Maximize(a.axis()?),
        "unmaximize" => UnMaximize(a.axis()?),
        "togglemaximize" => ToggleMaximize(a.axis()?),
        "togglefullscreen" => ToggleFullscreen,
        "toggledecorations" => ToggleDecorations,
        "shade" => Shade,
        "unshade" => Unshade,
        "toggleshade" => ToggleShade,
        "togglealwaysontop" => ToggleAlwaysOnTop,
        "togglealwaysonbottom" => ToggleAlwaysOnBottom,
        "toggleomnipresent" => ToggleOmnipresent,
        "move" => Move,
        "resize" => Resize,
        "moverelative" => MoveRelative {
            x: a.int("x")?,
            y: a.int("y")?,
        },
        "resizerelative" => ResizeRelative {
            left: a.int("left")?,
            right: a.int("right")?,
            top: a.int("top")?,
            bottom: a.int("bottom")?,
        },
        "snaptoedge" => {
            // combine="yes" (quarts d'écran en combinant deux bords): accepté,
            // pas encore appliqué
            a.bool("combine", false)?;
            SnapToEdge(match a.get("direction").map(str::to_ascii_lowercase).as_deref() {
                Some("left") => Edge::Left,
                Some("right") => Edge::Right,
                Some("up") => Edge::Up,
                Some("down") => Edge::Down,
                Some("center") => Edge::Center,
                v => return Err(format!("SnapToEdge: direction inconnue: {v:?}")),
            })
        }
        "unsnap" => UnSnap,
        "snaptoregion" => SnapToRegion(a.get("region").ok_or("SnapToRegion sans région")?.to_string()),
        "gotodesktop" => GoToDesktop {
            to: a.desktop()?,
            wrap: a.bool("wrap", true)?,
        },
        "sendtodesktop" => SendToDesktop {
            to: a.desktop()?,
            follow: a.bool("follow", true)?,
            wrap: a.bool("wrap", true)?,
        },
        "nextwindow" => NextWindow,
        "previouswindow" => PreviousWindow,
        "toggleshowdesktop" => ToggleShowDesktop,
        "reconfigure" => Reconfigure,
        "exit" => Exit,
        "autoplace" => AutoPlace(match a.get("policy") {
            Option::None => Option::None,
            Some(p) => {
                Some(Placement::parse(p).ok_or_else(|| format!("AutoPlace: politique inconnue: {p}"))?)
            }
        }),
        "none" => None,
        _ => return Err(format!("action inconnue ou pas encore prise en charge: {name}")),
    })
}

/// Actions des enfants `<action>` d'un élément; les erreurs vont dans
/// `warnings`
pub fn parse_children(node: roxmltree::Node, warnings: &mut Vec<String>) -> Vec<Action> {
    node.children()
        .filter(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case("action"))
        .filter_map(|c| match parse(c) {
            Ok(a) => Some(a),
            Err(e) => {
                warnings.push(format!("ligne {}: {e}", line(c)));
                Option::None
            }
        })
        .collect()
}

/// Numéro de ligne d'un élément, pour les messages
pub fn line(node: roxmltree::Node) -> u32 {
    node.document().text_pos_at(node.range().start).row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(xml: &str) -> Result<Action, String> {
        let doc = roxmltree::Document::parse(xml).unwrap();
        parse(doc.root_element())
    }

    #[test]
    fn actions() {
        assert_eq!(
            act(r#"<action name="Execute" command="kitty"/>"#),
            Ok(Action::Execute("kitty".into()))
        );
        assert_eq!(
            act(r#"<action name="Execute"><command>foot</command></action>"#),
            Ok(Action::Execute("foot".into()))
        );
        assert_eq!(
            act(r#"<action name="GoToDesktop" to="2"/>"#),
            Ok(Action::GoToDesktop {
                to: DesktopTarget::Index(1),
                wrap: true
            })
        );
        assert_eq!(
            act(r#"<action name="SendToDesktop" to="right" follow="no" wrap="yes"/>"#),
            Ok(Action::SendToDesktop {
                to: DesktopTarget::Right,
                follow: false,
                wrap: true
            })
        );
        assert_eq!(
            act(r#"<action name="ToggleMaximize" direction="vertical"/>"#),
            Ok(Action::ToggleMaximize(Axis::Vertical))
        );
        assert_eq!(
            act(r#"<action name="ResizeRelative" right="-20"/>"#),
            Ok(Action::ResizeRelative {
                left: 0,
                right: -20,
                top: 0,
                bottom: 0
            })
        );
        assert_eq!(
            act(r#"<action name="AutoPlace" policy="center"/>"#),
            Ok(Action::AutoPlace(Some(Placement::Center)))
        );
        assert!(act(r#"<action name="Vole"/>"#).is_err());
        assert!(act(r#"<action name="GoToDesktop" to="0"/>"#).is_err());
    }
}
