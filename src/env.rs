// SPDX-License-Identifier: MIT OR Apache-2.0
//! Environnement de la session (cahier des charges § 5.6): fichier
//! `environment` et disposition du clavier.

use std::path::Path;

/// Variables du fichier `environment`: `NOM=valeur`, `#` pour les
/// commentaires, `~` et `$VAR` développés en début de valeur
pub fn parse_environment(text: &str, lookup: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = vec![];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let mut value = value.trim().trim_matches('"').to_string();
        if let Some(rest) = value.strip_prefix('~') {
            if rest.is_empty() || rest.starts_with('/') {
                value = format!("{}{rest}", lookup("HOME").unwrap_or_default());
            }
        } else if let Some(rest) = value.strip_prefix('$') {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            let (var, tail) = rest.split_at(end);
            let known = vars.iter().rev().find(|(k, _)| k == var).map(|(_, v)| v.clone());
            value = format!("{}{tail}", known.or_else(|| lookup(var)).unwrap_or_default());
        }
        vars.retain(|(k, _)| k != name);
        vars.push((name.to_string(), value));
    }
    vars
}

pub fn read_environment(dir: &Path) -> Vec<(String, String)> {
    match std::fs::read_to_string(dir.join("environment")) {
        Ok(text) => parse_environment(&text, |k| std::env::var(k).ok()),
        Err(_) => vec![],
    }
}

/// Disposition du clavier
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Xkb {
    pub rules: String,
    pub model: String,
    pub layout: String,
    pub variant: String,
    pub options: Option<String>,
}

/// Disposition: variables `XKB_DEFAULT_*` du fichier `environment`, puis
/// de l'environnement du processus, puis celle du système (localectl:
/// `/etc/X11/xorg.conf.d/00-keyboard.conf`)
pub fn xkb(vars: &[(String, String)]) -> Xkb {
    let get = |name: &str| {
        vars.iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var(name).ok())
            .filter(|v| !v.is_empty())
    };
    let mut x = Xkb {
        rules: get("XKB_DEFAULT_RULES").unwrap_or_default(),
        model: get("XKB_DEFAULT_MODEL").unwrap_or_default(),
        layout: get("XKB_DEFAULT_LAYOUT").unwrap_or_default(),
        variant: get("XKB_DEFAULT_VARIANT").unwrap_or_default(),
        options: get("XKB_DEFAULT_OPTIONS"),
    };
    if x.layout.is_empty()
        && let Ok(text) = std::fs::read_to_string("/etc/X11/xorg.conf.d/00-keyboard.conf")
    {
        let system = parse_xorg_keyboard(&text);
        x.layout = system.layout;
        if x.variant.is_empty() {
            x.variant = system.variant;
        }
        if x.model.is_empty() {
            x.model = system.model;
        }
        if x.options.is_none() {
            x.options = system.options;
        }
    }
    x
}

/// `Option "XkbLayout" "fr"` … d'une section InputClass de Xorg
pub fn parse_xorg_keyboard(text: &str) -> Xkb {
    let mut x = Xkb::default();
    for line in text.lines() {
        let mut parts = line.split('"').skip(1).step_by(2);
        let (Some(key), Some(value)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !line.trim_start().starts_with("Option") {
            continue;
        }
        match key {
            "XkbLayout" => x.layout = value.to_string(),
            "XkbVariant" => x.variant = value.to_string(),
            "XkbModel" => x.model = value.to_string(),
            "XkbOptions" => x.options = Some(value.to_string()),
            _ => {}
        }
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment() {
        let text = "## commentaire\nXCURSOR_THEME=Vimix-Dark\nXCURSOR_SIZE=24\n\nCHEMIN=~/bin\nAUTRE=$CHEMIN/x\nexport Z=\"guillemets\"\nmauvais nom=1\nXCURSOR_SIZE=32\n";
        let vars = parse_environment(text, |k| (k == "HOME").then(|| "/home/a".to_string()));
        let get = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("XCURSOR_THEME"), Some("Vimix-Dark"));
        assert_eq!(get("XCURSOR_SIZE"), Some("32"));
        assert_eq!(get("CHEMIN"), Some("/home/a/bin"));
        assert_eq!(get("AUTRE"), Some("/home/a/bin/x"));
        assert_eq!(get("Z"), Some("guillemets"));
        assert_eq!(vars.len(), 5);
    }

    #[test]
    fn xorg() {
        let text = r#"Section "InputClass"
        Identifier "system-keyboard"
        MatchIsKeyboard "on"
        Option "XkbLayout" "fr"
        Option "XkbVariant" "oss"
EndSection"#;
        let x = parse_xorg_keyboard(text);
        assert_eq!((x.layout.as_str(), x.variant.as_str()), ("fr", "oss"));
        assert_eq!(x.options, None);
    }
}
