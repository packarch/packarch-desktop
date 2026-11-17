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

#[derive(Default)]
pub struct Apps {
    index: Option<Vec<Entry>>,
    cache: HashMap<String, Option<App>>,
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

impl Apps {
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
}
