// SPDX-License-Identifier: MIT OR Apache-2.0
//! Menus (cahier des charges § 5.5, § 10.4): lecture de `menu.xml` et de la
//! sortie des pipemenus. L'affichage et la navigation sont dans `ui.rs`,
//! le dessin dans `draw.rs`.

pub mod draw;
pub mod ui;

use std::collections::HashMap;
use std::path::Path;

use smithay::desktop::Window;
use tracing::warn;

use crate::actions::{self, Action, line};

/// Libellé avec sa lettre d'accélérateur (`_` avant la lettre, `__` pour
/// un `_`)
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub text: String,
    /// Position (en caractères) de la lettre soulignée
    pub accel_pos: Option<usize>,
}

impl Label {
    pub fn parse(s: &str) -> Label {
        let mut text = String::new();
        let mut accel_pos = None;
        let mut chars = s.chars().peekable();
        let mut n = 0;
        while let Some(c) = chars.next() {
            if c == '_' {
                match chars.peek() {
                    Some('_') => {
                        chars.next();
                        text.push('_');
                        n += 1;
                    }
                    Some(_) if accel_pos.is_none() => accel_pos = Some(n),
                    _ => {}
                }
                continue;
            }
            text.push(c);
            n += 1;
        }
        Label { text, accel_pos }
    }

    pub fn plain(s: &str) -> Label {
        Label {
            text: s.to_string(),
            accel_pos: None,
        }
    }

    /// Lettre d'accélérateur: celle marquée, sinon la première
    pub fn accel(&self) -> Option<char> {
        let c = match self.accel_pos {
            Some(p) => self.text.chars().nth(p),
            None => self.text.chars().find(|c| !c.is_whitespace()),
        }?;
        c.to_lowercase().next()
    }
}

#[derive(Debug, Clone)]
pub enum Entry {
    Item {
        label: Label,
        icon: Option<String>,
        actions: Vec<Action>,
        /// Vue visée (liste des fenêtres), sinon celle du menu
        target: Option<Window>,
        /// Case cochée (menu de fenêtre)
        checked: bool,
        enabled: bool,
    },
    /// Séparateur, ou titre de section s'il a un libellé
    Separator(Option<String>),
    /// Sous-menu, par son identifiant
    Menu(String),
}

impl Entry {
    pub fn item(label: &str, actions: Vec<Action>) -> Entry {
        Entry::Item {
            label: Label::parse(label),
            icon: None,
            actions,
            target: None,
            checked: false,
            enabled: true,
        }
    }

    pub fn is_selectable(&self, menus: &Menus) -> bool {
        match self {
            Entry::Item { enabled, .. } => *enabled,
            Entry::Separator(_) => false,
            Entry::Menu(id) => menus.get(id).is_some(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Menu {
    pub id: String,
    pub label: Label,
    pub icon: Option<String>,
    /// Pipemenu: commande dont la sortie donne le contenu
    pub execute: Option<String>,
    pub entries: Vec<Entry>,
}

/// Ensemble des menus, par identifiant
#[derive(Debug, Clone, Default)]
pub struct Menus {
    pub map: HashMap<String, Menu>,
}

/// Menus fournis par le compositeur (§ 5.5)
pub const ROOT: &str = "root-menu";
pub const CLIENT: &str = "client-menu";
pub const CLIENT_LIST: &str = "client-list-combined-menu";
/// Applications installées, par catégorie (fichiers `.desktop`)
pub const APPS: &str = "applications-menu";

/// Terminaux essayés dans l'ordre, après `$TERMINAL`
const TERMINALS: &str = "ghostty foot kitty alacritty konsole xfce4-terminal gnome-terminal xterm";

/// Commande qui ouvre `command` (ligne de commande du shell) dans le
/// premier terminal installé: `$TERMINAL`, sinon ghostty, foot, kitty…; vide: le
/// terminal seul
pub fn in_terminal(command: &str) -> String {
    if command.is_empty() {
        return format!(
            "for t in \"$TERMINAL\" {TERMINALS}; do command -v \"$t\" >/dev/null 2>&1 && exec \"$t\"; done"
        );
    }
    format!(
        "for t in \"$TERMINAL\" {TERMINALS}; do command -v \"$t\" >/dev/null 2>&1 || continue; \
         case \"${{t##*/}}\" in gnome-terminal) exec \"$t\" -- {command};; \
         xfce4-terminal) exec \"$t\" -x {command};; *) exec \"$t\" -e {command};; esac; done"
    )
}

/// Catégories principales de freedesktop.org: identifiant, libellé, icône
const CATEGORIES: &[(&[&str], &str, &str)] = &[
    (&["Utility"], "Accessoires", "applications-utilities"),
    (&["Office"], "Bureautique", "applications-office"),
    (&["Development"], "Développement", "applications-development"),
    (&["Education"], "Éducation", "applications-education"),
    (&["Graphics"], "Graphisme", "applications-graphics"),
    (&["Network"], "Internet", "applications-internet"),
    (&["Game"], "Jeux", "applications-games"),
    (
        &["AudioVideo", "Audio", "Video"],
        "Multimédia",
        "applications-multimedia",
    ),
    (&["Settings"], "Paramètres", "preferences-system"),
    (&["Science"], "Sciences", "applications-science"),
    (&["System"], "Système", "applications-system"),
];

/// Menu des applications (`applications-menu`) et ses sous-menus par
/// catégorie, dans l'ordre des libellés; « Autres » pour les applications
/// sans catégorie connue. Une application a une seule catégorie: la
/// première des siennes qui est connue
pub fn applications_menus(launchers: &[crate::apps::Launcher]) -> Vec<Menu> {
    let mut groups: Vec<Vec<Entry>> = vec![vec![]; CATEGORIES.len() + 1];
    for l in launchers {
        let group = l
            .categories
            .iter()
            .find_map(|c| {
                CATEGORIES
                    .iter()
                    .position(|(ids, _, _)| ids.contains(&c.as_str()))
            })
            .unwrap_or(CATEGORIES.len());
        let command = if l.terminal {
            in_terminal(&l.exec)
        } else {
            l.exec.clone()
        };
        groups[group].push(Entry::Item {
            label: Label::plain(&l.name),
            icon: l.icon.clone(),
            actions: vec![Action::Execute(command)],
            target: None,
            checked: false,
            enabled: true,
        });
    }
    let mut menus = vec![];
    let mut root = vec![];
    for (i, entries) in groups.into_iter().enumerate() {
        if entries.is_empty() {
            continue;
        }
        let (label, icon) = CATEGORIES
            .get(i)
            .map(|(_, l, ic)| (*l, *ic))
            .unwrap_or(("Autres", "applications-other"));
        let id = format!("{APPS}:{label}");
        root.push(Entry::Menu(id.clone()));
        menus.push(Menu {
            id,
            label: Label::plain(label),
            icon: Some(icon.into()),
            execute: None,
            entries,
        });
    }
    if root.is_empty() {
        root.push(Entry::Item {
            label: Label::plain("—"),
            icon: None,
            actions: vec![],
            target: None,
            checked: false,
            enabled: false,
        });
    }
    menus.push(Menu {
        id: APPS.into(),
        label: Label::plain("Applications"),
        icon: Some("applications-all".into()),
        execute: None,
        entries: root,
    });
    menus
}

fn is(node: roxmltree::Node, name: &str) -> bool {
    node.is_element() && node.tag_name().name().eq_ignore_ascii_case(name)
}

fn attr<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value())
}

impl Menus {
    pub fn get(&self, id: &str) -> Option<&Menu> {
        self.map.get(id)
    }

    /// Seulement le menu racine par défaut
    pub fn fallback() -> Menus {
        let mut menus = Menus::default();
        menus.map.insert(ROOT.into(), default_root());
        menus
    }

    /// Lit `menu.xml`; `Err` si le fichier n'est pas du XML valide. Un
    /// fichier absent donne le menu racine par défaut
    pub fn load(dir: &Path) -> Result<Menus, String> {
        let path = dir.join("menu.xml");
        let mut menus = Menus::default();
        match std::fs::read_to_string(&path) {
            Ok(xml) => {
                let mut warnings = vec![];
                menus
                    .parse(&xml, "", &mut warnings)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                for w in warnings {
                    warn!("menu.xml, {w}");
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("{}: {e}", path.display()),
        }
        if !menus.map.contains_key(ROOT) {
            menus.map.insert(ROOT.into(), default_root());
        }
        Ok(menus)
    }

    /// Lit un document de menus (`menu.xml`) ou la sortie d'un pipemenu
    /// (`<openbox_pipe_menu>`). `prefix` distingue les identifiants des
    /// sous-menus d'un pipemenu de ceux de `menu.xml`. Rend les entrées de
    /// premier niveau (pour un pipemenu)
    pub fn parse(
        &mut self,
        xml: &str,
        prefix: &str,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<Entry>, String> {
        let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
        let root = doc.root_element();
        let mut counter = 0;
        // Dans menu.xml, les menus de premier niveau sont des définitions
        let definitions = prefix.is_empty();
        Ok(self.entries(root, prefix, definitions, &mut counter, warnings))
    }

    fn entries(
        &mut self,
        node: roxmltree::Node,
        prefix: &str,
        definitions: bool,
        counter: &mut usize,
        warnings: &mut Vec<String>,
    ) -> Vec<Entry> {
        let mut out = vec![];
        for c in node.children().filter(|c| c.is_element()) {
            if is(c, "item") {
                out.push(Entry::Item {
                    label: Label::parse(attr(c, "label").unwrap_or("")),
                    icon: attr(c, "icon").filter(|s| !s.is_empty()).map(str::to_string),
                    actions: actions::parse_children(c, warnings),
                    target: None,
                    checked: false,
                    enabled: true,
                });
            } else if is(c, "separator") {
                out.push(Entry::Separator(
                    attr(c, "label").filter(|s| !s.is_empty()).map(str::to_string),
                ));
            } else if is(c, "menu") {
                let id = match attr(c, "id") {
                    Some(id) => format!("{prefix}{id}"),
                    None => {
                        *counter += 1;
                        format!("{prefix}#{counter}")
                    }
                };
                let label = attr(c, "label");
                let execute = attr(c, "execute").map(str::to_string);
                let has_children = c.children().any(|x| x.is_element());
                // Référence à un menu défini ailleurs: seulement un identifiant
                if label.is_none() && execute.is_none() && !has_children {
                    out.push(Entry::Menu(id));
                    continue;
                }
                let entries = self.entries(c, prefix, false, counter, warnings);
                if self.map.contains_key(&id) && definitions {
                    warnings.push(format!("ligne {}: menu « {id} » défini deux fois", line(c)));
                }
                self.map.insert(
                    id.clone(),
                    Menu {
                        id: id.clone(),
                        label: Label::parse(label.unwrap_or(&id)),
                        icon: attr(c, "icon").filter(|s| !s.is_empty()).map(str::to_string),
                        execute,
                        entries,
                    },
                );
                if !definitions {
                    out.push(Entry::Menu(id));
                }
            } else {
                warnings.push(format!(
                    "ligne {}: <{}> inconnu dans un menu, ignoré",
                    line(c),
                    c.tag_name().name()
                ));
            }
        }
        out
    }
}

/// Menu racine si `menu.xml` n'en a pas
fn default_root() -> Menu {
    Menu {
        id: ROOT.into(),
        label: Label::plain("packarch"),
        icon: None,
        execute: None,
        entries: vec![
            Entry::item("_Terminal", vec![Action::Execute(in_terminal(""))]),
            Entry::Separator(None),
            Entry::item("_Recharger la configuration", vec![Action::Reconfigure]),
            Entry::item("_Quitter", vec![Action::Exit]),
        ],
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn applications() {
        use crate::apps::Launcher;
        let l = |name: &str, cats: &[&str], terminal: bool| Launcher {
            name: name.into(),
            icon: None,
            exec: name.to_lowercase(),
            terminal,
            categories: cats.iter().map(|c| c.to_string()).collect(),
        };
        let menus = applications_menus(&[
            l("Firefox", &["Network", "WebBrowser"], false),
            l("Htop", &["System", "Monitor"], true),
            l("Thunar", &["System", "Utility", "FileManager"], false),
            l("Truc", &["X-Inconnu"], false),
            l("Mpv", &["AudioVideo", "Video"], false),
        ]);
        let root = menus.iter().find(|m| m.id == APPS).unwrap();
        let ids: Vec<_> = root
            .entries
            .iter()
            .map(|e| match e {
                Entry::Menu(id) => id.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(
            ids,
            [
                "applications-menu:Internet",
                "applications-menu:Multimédia",
                "applications-menu:Système",
                "applications-menu:Autres"
            ]
        );
        let system = menus.iter().find(|m| m.id.ends_with("Système")).unwrap();
        assert_eq!(system.entries.len(), 2);
        let Entry::Item { actions, .. } = &system.entries[0] else {
            panic!()
        };
        let Action::Execute(cmd) = &actions[0] else {
            panic!()
        };
        assert!(cmd.contains("-e htop") && cmd.contains("$TERMINAL"), "{cmd}");
        assert!(applications_menus(&[]).iter().any(|m| m.id == APPS));
        // Commandes du terminal valides pour sh
        for cmd in [in_terminal(""), in_terminal("htop -d 10")] {
            let ok = std::process::Command::new("sh")
                .args(["-n", "-c", &cmd])
                .status()
                .unwrap();
            assert!(ok.success(), "{cmd}");
        }
    }

    use super::*;

    #[test]
    fn labels() {
        let l = Label::parse("_Fichier");
        assert_eq!(l.text, "Fichier");
        assert_eq!(l.accel(), Some('f'));
        let l = Label::parse("Ou_vrir");
        assert_eq!((l.text.as_str(), l.accel_pos), ("Ouvrir", Some(2)));
        assert_eq!(l.accel(), Some('v'));
        let l = Label::parse("a__b");
        assert_eq!((l.text.as_str(), l.accel_pos), ("a_b", None));
        assert_eq!(Label::parse("Éteindre").accel(), Some('é'));
    }

    #[test]
    fn menu_xml() {
        let xml = r#"<openbox_menu>
          <menu id="root-menu" label="Packarch">
            <item label="Terminal" icon="utilities-terminal"><action name="Execute" command="kitty"/></item>
            <separator/>
            <menu id="apps" label="Applications" execute="gen-menu"/>
            <menu id="prefs" label="Préférences">
              <item label="Écrans"><action name="Execute"><command>wdisplays</command></action></item>
              <separator label="Titre"/>
            </menu>
            <menu id="ailleurs"/>
            <bidule/>
          </menu>
          <menu id="ailleurs" label="Ailleurs"><item label="x"/></menu>
        </openbox_menu>"#;
        let mut m = Menus::default();
        let mut w = vec![];
        m.parse(xml, "", &mut w).unwrap();
        assert_eq!(w.len(), 1, "{w:?}");
        let root = m.get("root-menu").unwrap();
        assert_eq!(root.entries.len(), 5);
        assert!(
            matches!(&root.entries[0], Entry::Item { icon: Some(i), actions, .. }
            if i == "utilities-terminal" && actions == &[Action::Execute("kitty".into())])
        );
        assert!(matches!(&root.entries[2], Entry::Menu(id) if id == "apps"));
        assert_eq!(m.get("apps").unwrap().execute.as_deref(), Some("gen-menu"));
        let prefs = m.get("prefs").unwrap();
        assert!(matches!(&prefs.entries[1], Entry::Separator(Some(t)) if t == "Titre"));
        assert!(matches!(&root.entries[4], Entry::Menu(id) if id == "ailleurs"));
        assert_eq!(m.get("ailleurs").unwrap().entries.len(), 1);
    }

    #[test]
    fn pipe_menu() {
        let xml = r#"<openbox_pipe_menu>
          <separator label="Thèmes"/>
          <item label="1. Adaptive"><action name="Execute"><command>skin adaptive</command></action></item>
          <menu id="Accessories" label="Accessoires" icon="applications-accessories">
            <item label="Thunar"><action name="Execute"><command>thunar</command></action></item>
          </menu>
        </openbox_pipe_menu>"#;
        let mut m = Menus::default();
        let mut w = vec![];
        let entries = m.parse(xml, "apps/", &mut w).unwrap();
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(entries.len(), 3);
        assert!(matches!(&entries[2], Entry::Menu(id) if id == "apps/Accessories"));
        assert_eq!(m.get("apps/Accessories").unwrap().entries.len(), 1);
    }

    #[test]
    fn packarch_menu() {
        // Le menu.xml de Packarch, s'il est présent: aucun avertissement
        let dir = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config/packarch"));
        if let Some(xml) = dir.and_then(|d| std::fs::read_to_string(d.join("menu.xml")).ok()) {
            let mut m = Menus::default();
            let mut w = vec![];
            m.parse(&xml, "", &mut w).unwrap();
            assert!(w.is_empty(), "{w:?}");
            assert!(m.get(ROOT).unwrap().entries.len() > 8);
        }
    }
}
