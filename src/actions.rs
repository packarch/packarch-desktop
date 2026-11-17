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
    // Priorité 2 (§ 7.2)
    /// Jusqu'au bord de la zone utile, ou d'une autre vue (`snapWindows`)
    MoveToEdge {
        direction: Edge,
        snap_windows: bool,
    },
    GrowToEdge(Edge),
    ShrinkToEdge(Edge),
    /// Position du cadre, relative à la zone utile de l'écran de la vue
    MoveTo {
        x: i32,
        y: i32,
    },
    /// Taille du client
    ResizeTo {
        width: i32,
        height: i32,
    },
    MoveToCursor,
    ToggleKeybinds,
    FocusOutput(OutputTarget),
    MoveToOutput(OutputTarget),
    /// Actions selon l'état de la vue: `then` si une requête correspond
    If {
        queries: Vec<Query>,
        then: Vec<Action>,
        otherwise: Vec<Action>,
    },
    /// `then` pour chaque vue qui correspond, `otherwise` si aucune
    ForEach {
        queries: Vec<Query>,
        then: Vec<Action>,
        otherwise: Vec<Action>,
    },
}

/// Écran visé: par son nom, ou dans une direction depuis l'écran courant
#[derive(Debug, Clone, PartialEq)]
pub enum OutputTarget {
    Name(String),
    Direction(Edge),
}

/// Critères d'une requête `If`/`ForEach`; tous ceux donnés doivent
/// correspondre
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub identifier: Option<String>,
    pub title: Option<String>,
    pub maximized: Option<bool>,
    pub iconified: Option<bool>,
    pub focused: Option<bool>,
    pub omnipresent: Option<bool>,
    pub shaded: Option<bool>,
    pub fullscreen: Option<bool>,
    /// Bureau (à partir de 0), ou `current`
    pub desktop: Option<DesktopQuery>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DesktopQuery {
    Index(usize),
    Current,
}

fn edge(v: Option<&str>, what: &str) -> Result<Edge, String> {
    Ok(match v.map(str::to_ascii_lowercase).as_deref() {
        Some("left") => Edge::Left,
        Some("right") => Edge::Right,
        Some("up") => Edge::Up,
        Some("down") => Edge::Down,
        v => return Err(format!("{what}: direction inconnue: {v:?}")),
    })
}

fn query(node: roxmltree::Node) -> Result<Query, String> {
    let a = Attrs { node };
    let b = |name: &str| -> Result<Option<bool>, String> {
        match a.get(name) {
            Option::None => Ok(Option::None),
            Some(v) => parse_bool(v)
                .map(Some)
                .ok_or_else(|| format!("{name}: booléen attendu, pas « {v} »")),
        }
    };
    Ok(Query {
        identifier: a.get("identifier").map(str::to_string),
        title: a.get("title").map(str::to_string),
        maximized: b("maximized")?,
        iconified: b("iconified")?,
        focused: b("focused")?,
        omnipresent: b("omnipresent")?,
        shaded: b("shaded")?,
        fullscreen: b("fullscreen")?,
        desktop: match a.get("desktop") {
            Option::None => Option::None,
            Some(v) if v.eq_ignore_ascii_case("current") => Some(DesktopQuery::Current),
            Some(v) => match v.parse::<usize>() {
                Ok(n) if n >= 1 => Some(DesktopQuery::Index(n - 1)),
                _ => return Err(format!("desktop: numéro ou « current » attendu, pas « {v} »")),
            },
        },
    })
}

/// Actions d'un élément enfant (`<then>`, `<else>`): une erreur de l'une
/// d'elles rend toute l'action invalide
fn branch(node: roxmltree::Node, name: &str) -> Result<Vec<Action>, String> {
    let Some(child) = node
        .children()
        .find(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case(name))
    else {
        return Ok(vec![]);
    };
    let mut warnings = vec![];
    let actions = parse_children(child, &mut warnings);
    if warnings.is_empty() {
        Ok(actions)
    } else {
        Err(warnings.join("; "))
    }
}

/// `If` et `ForEach`: requêtes (`<query>`, ou attributs de l'action
/// elle-même), `<then>` et `<else>`
fn conditional(node: roxmltree::Node) -> Result<(Vec<Query>, Vec<Action>, Vec<Action>), String> {
    let mut queries = vec![];
    for q in node
        .children()
        .filter(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case("query"))
    {
        queries.push(query(q)?);
    }
    if queries.is_empty() {
        queries.push(query(node)?);
    }
    Ok((queries, branch(node, "then")?, branch(node, "else")?))
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
        "movetoedge" => MoveToEdge {
            direction: edge(a.get("direction"), "MoveToEdge")?,
            snap_windows: a.bool("snapWindows", true)?,
        },
        "growtoedge" => GrowToEdge(edge(a.get("direction"), "GrowToEdge")?),
        "shrinktoedge" => ShrinkToEdge(edge(a.get("direction"), "ShrinkToEdge")?),
        "moveto" => MoveTo {
            x: a.int("x")?,
            y: a.int("y")?,
        },
        "resizeto" => ResizeTo {
            width: a.int("width")?,
            height: a.int("height")?,
        },
        "movetocursor" => MoveToCursor,
        "togglekeybinds" => ToggleKeybinds,
        "focusoutput" | "movetooutput" => {
            let target = match (a.get("output"), a.get("direction")) {
                (Some(o), _) => OutputTarget::Name(o.to_string()),
                (Option::None, d @ Some(_)) => OutputTarget::Direction(edge(d, name)?),
                _ => return Err(format!("{name}: attribut output ou direction attendu")),
            };
            if name.eq_ignore_ascii_case("focusoutput") {
                FocusOutput(target)
            } else {
                MoveToOutput(target)
            }
        }
        "if" => {
            let (queries, then, otherwise) = conditional(node)?;
            If {
                queries,
                then,
                otherwise,
            }
        }
        "foreach" => {
            let (queries, then, otherwise) = conditional(node)?;
            ForEach {
                queries,
                then,
                otherwise,
            }
        }
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
        assert_eq!(
            act(r#"<action name="MoveToEdge" direction="left" snapWindows="no"/>"#),
            Ok(Action::MoveToEdge {
                direction: Edge::Left,
                snap_windows: false
            })
        );
        assert_eq!(
            act(r#"<action name="MoveToOutput" direction="right"/>"#),
            Ok(Action::MoveToOutput(OutputTarget::Direction(Edge::Right)))
        );
        let a = act(
            r#"<action name="If"><query identifier="kitty" maximized="yes"/><query title="Thèmes*"/>
               <then><action name="Close"/></then><else><action name="Maximize"/></else></action>"#,
        )
        .unwrap();
        let Action::If {
            queries,
            then,
            otherwise,
        } = a
        else {
            panic!()
        };
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].maximized, Some(true));
        assert_eq!(then, vec![Action::Close]);
        assert_eq!(otherwise, vec![Action::Maximize(Axis::Both)]);
        assert!(act(r#"<action name="If"><then><action name="Vole"/></then></action>"#).is_err());
        assert!(act(r#"<action name="Vole"/>"#).is_err());
        assert!(act(r#"<action name="GoToDesktop" to="0"/>"#).is_err());
    }
}
