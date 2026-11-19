// SPDX-License-Identifier: MIT OR Apache-2.0
//! Applications installées (fichiers `.desktop`): nom et icône d'une vue
//! d'après son identifiant (app_id), pour la liste Alt+Tab et la liste des
//! fenêtres (§ 8.3).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone)]
struct Entry {
    /// Nom du fichier sans `.desktop`, en minuscules
    stem: String,
    wm_class: Option<String>,
    app: App,
}

/// Application à lancer depuis le menu des applications
#[derive(Debug, Clone, PartialEq)]
pub struct Launcher {
    pub name: String,
    pub icon: Option<String>,
    /// Commande de `Exec`, sans les codes `%f`, `%U`…
    pub exec: String,
    /// À lancer dans un terminal (`Terminal=true`)
    pub terminal: bool,
    pub categories: Vec<String>,
}

#[derive(Default)]
pub struct Apps {
    index: Option<Vec<Entry>>,
    cache: HashMap<String, Option<App>>,
    launchers: Option<Vec<Launcher>>,
}

fn dirs() -> Vec<PathBuf> {
    let mut out = vec![];
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.map(|h| h.join(".local/share")));
    if let Some(d) = data_home {
        out.push(d.join("applications"));
    }
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data.split(':').filter(|d| !d.is_empty()) {
        out.push(Path::new(d).join("applications"));
    }
    out
}

/// Langues de l'utilisateur, de la plus précise à la plus générale
/// (`fr_FR`, `fr`)
fn languages() -> Vec<String> {
    let lang = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .unwrap_or_default();
    let base = lang.split(['.', '@']).next().unwrap_or("").to_string();
    let mut out = vec![];
    if !base.is_empty() && base != "C" && base != "POSIX" {
        out.push(base.clone());
        if let Some((l, _)) = base.split_once('_') {
            out.push(l.to_string());
        }
    }
    out
}

/// Lit la section `[Desktop Entry]` d'un fichier `.desktop`
pub fn parse(text: &str, langs: &[String]) -> Option<(App, Option<String>)> {
    let mut in_entry = false;
    let mut name = None;
    let mut localized: Vec<(usize, String)> = vec![];
    let mut icon = None;
    let mut wm_class = None;
    let mut hidden = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Name" => name = Some(v.to_string()),
            "Icon" if !v.is_empty() => icon = Some(v.to_string()),
            "StartupWMClass" => wm_class = Some(v.to_lowercase()),
            "NoDisplay" | "Hidden" if v == "true" => hidden = true,
            _ => {
                if let Some(l) = k.strip_prefix("Name[").and_then(|k| k.strip_suffix(']'))
                    && let Some(p) = langs.iter().position(|x| x == l)
                {
                    localized.push((p, v.to_string()));
                }
            }
        }
    }
    let _ = hidden;
    localized.sort_by_key(|(p, _)| *p);
    let name = localized.into_iter().next().map(|(_, n)| n).or(name)?;
    Some((App { name, icon }, wm_class))
}

/// Ligne `Exec` sans ses codes de champ: `%f`, `%F`, `%u`, `%U`, `%i`,
/// `%c`, `%k` et les codes obsolètes sont retirés, `%%` donne `%`
pub fn strip_field_codes(exec: &str) -> String {
    let mut out = String::new();
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if let Some('%') = chars.next() {
            out.push('%');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Vrai si la liste `A;B;` contient un des bureaux de `desktops`
fn shows_in(list: &str, desktops: &[String]) -> bool {
    list.split(';')
        .any(|d| desktops.iter().any(|x| x.eq_ignore_ascii_case(d.trim())))
}

/// Programme trouvé: chemin absolu existant, ou nom dans le PATH
fn in_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// Lanceur d'un fichier `.desktop`, s'il doit paraître dans le menu des
/// applications de `desktops` (bureaux de `XDG_CURRENT_DESKTOP`): type
/// Application, ni `NoDisplay` ni `Hidden`, `OnlyShowIn`/`NotShowIn`
/// respectés, programme de `TryExec` présent (vérifié par `exists`)
pub fn parse_launcher(
    text: &str,
    langs: &[String],
    desktops: &[String],
    exists: impl Fn(&str) -> bool,
) -> Option<Launcher> {
    let (app, _) = parse(text, langs)?;
    let mut in_entry = false;
    let mut exec = None;
    let mut terminal = false;
    let mut categories = vec![];
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        let Some((k, v)) = line.split_once('=').filter(|_| in_entry) else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Type" if v != "Application" => return None,
            "NoDisplay" | "Hidden" if v == "true" => return None,
            "OnlyShowIn" if !shows_in(v, desktops) => return None,
            "NotShowIn" if shows_in(v, desktops) => return None,
            "TryExec" if !v.is_empty() && !exists(v) => return None,
            "Exec" => exec = Some(strip_field_codes(v)),
            "Terminal" => terminal = v == "true",
            "Categories" => {
                categories = v
                    .split(';')
                    .filter(|c| !c.is_empty())
                    .map(str::to_string)
                    .collect()
            }
            _ => {}
        }
    }
    let exec = exec.filter(|e| !e.is_empty())?;
    Some(Launcher {
        name: app.name,
        icon: app.icon,
        exec,
        terminal,
        categories,
    })
}

impl Apps {
    /// Applications du menu, triées par nom (lues une fois, oubliées au
    /// rechargement)
    pub fn launchers(&mut self) -> &[Launcher] {
        if self.launchers.is_none() {
            let langs = languages();
            let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP")
                .unwrap_or_else(|_| "packarch".into())
                .split(':')
                .map(str::to_string)
                .collect();
            let mut seen: Vec<String> = vec![];
            let mut out: Vec<Launcher> = vec![];
            for d in dirs() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                let mut files: Vec<PathBuf> = rd.flatten().map(|f| f.path()).collect();
                files.sort();
                for path in files {
                    if path.extension().is_none_or(|e| e != "desktop") {
                        continue;
                    }
                    let stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    // Le premier dossier gagne, même pour masquer
                    if seen.contains(&stem) {
                        continue;
                    }
                    seen.push(stem);
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    if let Some(l) = parse_launcher(&text, &langs, &desktops, in_path) {
                        out.push(l);
                    }
                }
            }
            out.sort_by_cached_key(|l| l.name.to_lowercase());
            self.launchers = Some(out);
        }
        self.launchers.as_deref().unwrap()
    }

    fn index(&mut self) -> &[Entry] {
        if self.index.is_none() {
            let langs = languages();
            let mut entries: Vec<Entry> = vec![];
            for d in dirs() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for f in rd.flatten() {
                    let path = f.path();
                    if path.extension().is_none_or(|e| e != "desktop") {
                        continue;
                    }
                    let stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    // Le premier dossier gagne (le dossier personnel d'abord)
                    if entries.iter().any(|e| e.stem == stem) {
                        continue;
                    }
                    let Ok(text) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    if let Some((app, wm_class)) = parse(&text, &langs) {
                        entries.push(Entry { stem, wm_class, app });
                    }
                }
            }
            self.index = Some(entries);
        }
        self.index.as_deref().unwrap()
    }

    /// Oublie les applications lues (rechargement)
    pub fn clear(&mut self) {
        self.index = None;
        self.cache.clear();
        self.launchers = None;
    }

    /// Application d'une vue d'après son app_id: fichier du même nom, fin du
    /// nom (`org.xfce.thunar` → `thunar`), ou `StartupWMClass`
    pub fn get(&mut self, app_id: &str) -> Option<App> {
        if app_id.is_empty() {
            return None;
        }
        if let Some(a) = self.cache.get(app_id) {
            return a.clone();
        }
        let id = app_id.to_lowercase();
        let last = id.rsplit('.').next().unwrap_or(&id).to_string();
        let index = self.index();
        let found = index
            .iter()
            .find(|e| e.stem == id)
            .or_else(|| index.iter().find(|e| e.wm_class.as_deref() == Some(&id)))
            .or_else(|| index.iter().find(|e| e.stem.rsplit('.').next() == Some(&last)))
            .map(|e| e.app.clone());
        self.cache.insert(app_id.to_string(), found.clone());
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_file() {
        let text = "[Desktop Entry]\nName=Files\nName[fr]=Fichiers\nIcon=org.x.files\nStartupWMClass=Files\n[Desktop Action new]\nName=Nouvelle\n";
        let (app, class) = parse(text, &["fr_FR".into(), "fr".into()]).unwrap();
        assert_eq!(app.name, "Fichiers");
        assert_eq!(app.icon.as_deref(), Some("org.x.files"));
        assert_eq!(class.as_deref(), Some("files"));
        let (app, _) = parse(text, &[]).unwrap();
        assert_eq!(app.name, "Files");
        assert!(parse("[Autre]\nName=x\n", &[]).is_none());
    }

    #[test]
    fn launcher() {
        let desktops = ["packarch".to_string(), "wlroots".to_string()];
        let text = "[Desktop Entry]\nType=Application\nName=Htop\nExec=htop %F\nTerminal=true\nCategories=System;Monitor;\nIcon=htop\n[Desktop Action x]\nExec=autre\n";
        let l = parse_launcher(text, &[], &desktops, |_| true).unwrap();
        assert_eq!(l.exec, "htop");
        assert!(l.terminal);
        assert_eq!(l.categories, ["System", "Monitor"]);
        let hidden = |extra: &str| {
            let t = format!("[Desktop Entry]\nType=Application\nName=A\nExec=a\n{extra}\n");
            parse_launcher(&t, &[], &desktops, |p| p == "present").is_none()
        };
        assert!(!hidden(""));
        assert!(hidden("NoDisplay=true"));
        assert!(hidden("OnlyShowIn=GNOME;KDE;"));
        assert!(!hidden("OnlyShowIn=XFCE;packarch;"));
        assert!(hidden("NotShowIn=packarch;"));
        assert!(hidden("TryExec=absent"));
        assert!(!hidden("TryExec=present"));
        assert!(hidden("Type=Link"));
        assert_eq!(strip_field_codes("a --x=%u 100%% %F"), "a --x= 100%");
    }
}
