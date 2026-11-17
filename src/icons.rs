// SPDX-License-Identifier: MIT OR Apache-2.0
//! Icônes des menus (cahier des charges § 5.5, § 10.4): chemin direct, ou
//! nom cherché dans le thème d'icônes (`theme/icon`), ses parents
//! (`Inherits`), `hicolor` puis `/usr/share/pixmaps`. Formats png et svg,
//! mis à la taille demandée et gardés en cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use resvg::tiny_skia::{self, Pixmap};
use tracing::debug;

/// Dossier d'un thème d'icônes (section de `index.theme`)
#[derive(Debug, Clone)]
struct Dir {
    path: PathBuf,
    size: u32,
    scale: u32,
    min: u32,
    max: u32,
    scalable: bool,
}

impl Dir {
    /// Écart entre la taille du dossier et celle demandée (0: convient)
    fn distance(&self, px: u32) -> u32 {
        let (lo, hi) = (self.min * self.scale, self.max * self.scale);
        if px < lo { lo - px } else { px.saturating_sub(hi) }
    }
}

/// Dossiers de base des thèmes d'icônes, par priorité
fn base_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(h) = &home {
        dirs.push(h.join(".icons"));
    }
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    if let Some(d) = data_home {
        dirs.push(d.join("icons"));
    }
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    for d in data.split(':').filter(|d| !d.is_empty()) {
        dirs.push(Path::new(d).join("icons"));
    }
    dirs
}

/// Lit `index.theme`: dossiers et parents
fn read_index(text: &str, root: &Path) -> (Vec<Dir>, Vec<String>) {
    let mut sections: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            sections
                .entry(current.clone())
                .or_default()
                .insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let Some(head) = sections.get("Icon Theme") else {
        return (vec![], vec![]);
    };
    let list = |k: &str| -> Vec<String> {
        head.get(k)
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let inherits = list("Inherits");
    let mut names = list("Directories");
    names.extend(list("ScaledDirectories"));
    let mut dirs = vec![];
    for name in names {
        let Some(sec) = sections.get(&name) else {
            continue;
        };
        let num = |k: &str| sec.get(k).and_then(|v| v.parse::<u32>().ok());
        let Some(size) = num("Size") else { continue };
        let kind = sec.get("Type").map(String::as_str).unwrap_or("Threshold");
        let threshold = num("Threshold").unwrap_or(2);
        let (min, max) = match kind {
            "Fixed" => (size, size),
            "Scalable" => (num("MinSize").unwrap_or(size), num("MaxSize").unwrap_or(size)),
            _ => (size.saturating_sub(threshold), size + threshold),
        };
        dirs.push(Dir {
            path: root.join(&name),
            size,
            scale: num("Scale").unwrap_or(1).max(1),
            min,
            max,
            scalable: kind == "Scalable",
        });
    }
    (dirs, inherits)
}

/// Icônes chargées
#[derive(Default)]
pub struct Icons {
    theme: String,
    /// Dossiers du thème et de ses parents, dans l'ordre de recherche
    chain: Option<Vec<Vec<Dir>>>,
    cache: HashMap<(String, u32), Option<Arc<Pixmap>>>,
}

impl Icons {
    /// Change de thème d'icônes (rechargement de la configuration)
    pub fn set_theme(&mut self, theme: &str) {
        if self.theme != theme || self.chain.is_none() {
            self.theme = theme.to_string();
            self.chain = None;
            self.cache.clear();
        }
    }

    /// Oublie les icônes en cache (elles ont pu changer sur le disque)
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    fn chain(&mut self) -> &[Vec<Dir>] {
        if self.chain.is_none() {
            let bases = base_dirs();
            let mut chain = vec![];
            let mut seen: Vec<String> = vec![];
            let mut queue = vec![self.theme.clone()];
            while let Some(name) = queue.first().cloned() {
                queue.remove(0);
                if name.is_empty() || seen.contains(&name) {
                    continue;
                }
                seen.push(name.clone());
                let mut dirs = vec![];
                let mut parents = vec![];
                for b in &bases {
                    let root = b.join(&name);
                    if let Ok(text) = std::fs::read_to_string(root.join("index.theme")) {
                        let (d, p) = read_index(&text, &root);
                        if parents.is_empty() {
                            parents = p;
                        }
                        dirs.extend(d);
                    }
                }
                chain.push(dirs);
                queue.extend(parents);
                if queue.is_empty() && !seen.iter().any(|s| s == "hicolor") {
                    queue.push("hicolor".into());
                }
            }
            self.chain = Some(chain);
        }
        self.chain.as_deref().unwrap()
    }

    /// Fichier d'une icône, pour une taille en pixels physiques
    fn find(&mut self, name: &str, px: u32) -> Option<PathBuf> {
        if name.contains('/') {
            let p = PathBuf::from(name);
            return p.is_file().then_some(p);
        }
        let exts = ["png", "svg"];
        for dirs in self.chain() {
            // Taille exacte d'abord, puis la plus proche (en préférant plus grand)
            let mut sorted: Vec<&Dir> = dirs.iter().collect();
            sorted.sort_by_key(|d| (d.distance(px), !d.scalable && d.size * d.scale < px, d.size));
            for d in sorted {
                for e in exts {
                    let f = d.path.join(format!("{name}.{e}"));
                    if f.is_file() {
                        return Some(f);
                    }
                }
            }
        }
        for e in exts {
            let f = PathBuf::from(format!("/usr/share/pixmaps/{name}.{e}"));
            if f.is_file() {
                return Some(f);
            }
        }
        None
    }

    /// Icône de `px` × `px` pixels physiques (prémultipliée)
    pub fn get(&mut self, name: &str, px: u32) -> Option<Arc<Pixmap>> {
        let key = (name.to_string(), px);
        if let Some(v) = self.cache.get(&key) {
            return v.clone();
        }
        let icon = self.find(name, px).and_then(|path| match load(&path, px) {
            Some(p) => Some(Arc::new(p)),
            None => {
                debug!("icône illisible: {}", path.display());
                None
            }
        });
        if icon.is_none() {
            debug!("icône introuvable: {name}");
        }
        self.cache.insert(key, icon.clone());
        icon
    }
}

/// Charge une image et la met à `px` × `px`, proportions gardées
fn load(path: &Path, px: u32) -> Option<Pixmap> {
    let data = std::fs::read(path).ok()?;
    let is_svg = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"));
    if is_svg {
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let size = tree.size();
        let k = px as f32 / size.width().max(size.height());
        let mut out = Pixmap::new(px, px)?;
        let dx = (px as f32 - size.width() * k) / 2.0;
        let dy = (px as f32 - size.height() * k) / 2.0;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_row(k, 0.0, 0.0, k, dx, dy),
            &mut out.as_mut(),
        );
        return Some(out);
    }
    let src = decode_png(&data)?;
    Some(resize(&src, px))
}

/// PNG en pixmap prémultipliée
fn decode_png(data: &[u8]) -> Option<Pixmap> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(data));
    dec.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let mut out = Pixmap::new(w, h)?;
    let px = out.data_mut();
    for i in 0..(w * h) as usize {
        let (r, g, b, a) = match info.color_type {
            png::ColorType::Rgba => (buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2], buf[i * 4 + 3]),
            png::ColorType::GrayscaleAlpha => {
                let v = buf[i * 2];
                (v, v, v, buf[i * 2 + 1])
            }
            png::ColorType::Rgb => (buf[i * 3], buf[i * 3 + 1], buf[i * 3 + 2], 255),
            png::ColorType::Grayscale => (buf[i], buf[i], buf[i], 255),
            png::ColorType::Indexed => return None,
        };
        let m = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
        px[i * 4..i * 4 + 4].copy_from_slice(&[m(r), m(g), m(b), a]);
    }
    Some(out)
}

/// Réduction (moyenne des pixels couverts) ou agrandissement (plus proche
/// voisin) dans un carré de `px`, proportions gardées et centré
fn resize(src: &Pixmap, px: u32) -> Pixmap {
    let mut out = Pixmap::new(px.max(1), px.max(1)).unwrap();
    let (sw, sh) = (src.width() as f64, src.height() as f64);
    let k = px as f64 / sw.max(sh);
    let (dw, dh) = ((sw * k).round().max(1.0) as u32, (sh * k).round().max(1.0) as u32);
    let (ox, oy) = ((px - dw.min(px)) / 2, (px - dh.min(px)) / 2);
    let s = src.data();
    let stride = src.width() as usize;
    let d = out.data_mut();
    for y in 0..dh.min(px) {
        let y0 = y as f64 / k;
        let y1 = ((y + 1) as f64 / k).min(sh);
        for x in 0..dw.min(px) {
            let x0 = x as f64 / k;
            let x1 = ((x + 1) as f64 / k).min(sw);
            let mut acc = [0.0f64; 4];
            let mut n = 0.0;
            let (ya, yb) = (
                y0.floor() as usize,
                (y1.ceil() as usize).max(y0.floor() as usize + 1),
            );
            let (xa, xb) = (
                x0.floor() as usize,
                (x1.ceil() as usize).max(x0.floor() as usize + 1),
            );
            for sy in ya..yb.min(sh as usize) {
                for sx in xa..xb.min(sw as usize) {
                    let i = (sy * stride + sx) * 4;
                    for c in 0..4 {
                        acc[c] += s[i + c] as f64;
                    }
                    n += 1.0;
                }
            }
            if n > 0.0 {
                let i = (((y + oy) * px + x + ox) * 4) as usize;
                for c in 0..4 {
                    d[i + c] = (acc[c] / n).round() as u8;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index() {
        let text = "[Icon Theme]\nName=T\nInherits=Parent, hicolor\nDirectories=16x16/apps,scalable/apps\n\n[16x16/apps]\nSize=16\nType=Fixed\n\n[scalable/apps]\nSize=48\nType=Scalable\nMinSize=8\nMaxSize=512\n";
        let (dirs, inherits) = read_index(text, Path::new("/t"));
        assert_eq!(inherits, vec!["Parent", "hicolor"]);
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0].distance(16), 0);
        assert_eq!(dirs[0].distance(24), 8);
        assert!(dirs[1].scalable && dirs[1].distance(24) == 0);
    }

    #[test]
    fn shrink() {
        let mut src = Pixmap::new(4, 2).unwrap();
        src.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        let out = resize(&src, 2);
        // 4×2 → 2×1, en haut du carré 2×2 (décalage arrondi vers le bas)
        assert_eq!(out.pixel(0, 0).unwrap().red(), 255);
        assert_eq!(out.pixel(1, 0).unwrap().alpha(), 255);
        assert_eq!(out.pixel(0, 1).unwrap().alpha(), 0);
    }
}
