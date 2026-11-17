// SPDX-License-Identifier: MIT OR Apache-2.0
//! Cadre des fenêtres (cahier des charges § 10.1): disposition de la barre
//! de titre et des boutons, zones de la souris, dessin du titre et des
//! boutons en mémoire.

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight};
use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::config::{Button, Config, Font};
use crate::theme::{Color, Justify, Mask, Theme};

/// Largeur de la zone de saisie des bords, au-delà du cadre (§ 8.7)
pub const GRAB: i32 = 8;

/// Bords touchés par un redimensionnement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

/// Partie du cadre sous le pointeur
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Title,
    Button(Button),
    Border(Edges),
}

/// Taille d'une police en pixels logiques (points à 96 ppp)
pub fn font_px(font: &Font) -> f32 {
    font.size * 96.0 / 72.0
}

/// Hauteur d'une ligne de texte pour une police
pub fn line_height(font: &Font) -> i32 {
    (font_px(font) * 1.35).ceil() as i32
}

/// Parties du cadre affichées
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub title: bool,
    pub border: bool,
}

#[cfg(test)]
impl Frame {
    pub const FULL: Frame = Frame {
        title: true,
        border: true,
    };
}

/// Disposition du cadre d'une vue, en coordonnées relatives au coin haut
/// gauche du cadre
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub size: Size<i32, Logical>,
    pub border: i32,
    pub title_height: i32,
    pub client: Rectangle<i32, Logical>,
    pub buttons: Vec<(Button, Rectangle<i32, Logical>)>,
    pub title: Rectangle<i32, Logical>,
}

impl Layout {
    pub fn title_height(config: &Config, theme: &Theme) -> i32 {
        line_height(&config.font_active).max(line_height(&config.font_inactive)) + 2 * theme.padding_height
    }

    pub fn new(config: &Config, theme: &Theme, client: Size<i32, Logical>, frame: Frame) -> Layout {
        let b = if frame.border { theme.border_width } else { 0 };
        let th = if frame.title {
            Layout::title_height(config, theme)
        } else {
            0
        };
        let size = Size::from((client.w + 2 * b, client.h + th + 2 * b));
        let pad = theme.padding_width;
        let bw = line_height(&config.font_active) + pad;
        let mut buttons = vec![];
        let (left, right): (&[Button], &[Button]) = if frame.title {
            (&config.buttons_left, &config.buttons_right)
        } else {
            (&[], &[])
        };
        let mut x = b + pad / 2;
        for &btn in left {
            buttons.push((btn, Rectangle::new((x, b).into(), (bw, th).into())));
            x += bw;
        }
        let left_end = x;
        let mut x = size.w - b - pad / 2;
        for &btn in right.iter().rev() {
            x -= bw;
            buttons.push((btn, Rectangle::new((x, b).into(), (bw, th).into())));
        }
        let right_start = x;
        let title_x = left_end + pad;
        let title = Rectangle::new(
            (title_x, b).into(),
            ((right_start - pad - title_x).max(0), th).into(),
        );
        Layout {
            size,
            border: b,
            title_height: th,
            client: Rectangle::new((b, b + th).into(), client),
            buttons,
            title,
        }
    }

    /// Partie du cadre au point `p` (relatif au cadre); `None` hors du cadre
    /// et de la zone de saisie, ou sur le client
    pub fn part_at(&self, p: Point<f64, Logical>) -> Option<Part> {
        let (w, h) = (self.size.w as f64, self.size.h as f64);
        if self.border == 0 && self.title_height == 0 {
            return None;
        }
        let g = GRAB as f64;
        if p.x < -g || p.y < -g || p.x >= w + g || p.y >= h + g {
            return None;
        }
        if self.client.to_f64().contains(p) {
            return None;
        }
        // Coins: 2 × GRAB le long de chaque bord
        let corner = 2.0 * g;
        let inside = p.x >= 0.0 && p.y >= 0.0 && p.x < w && p.y < h;
        let edges = Edges {
            left: p.x < (self.border as f64).max(0.0)
                || (p.x < corner && (p.y < 0.0 || p.y >= h - self.border as f64)),
            right: p.x >= w - self.border as f64
                || (p.x >= w - corner && (p.y < 0.0 || p.y >= h - self.border as f64)),
            top: p.y < self.border as f64 || (p.y < corner && (p.x < 0.0 || p.x >= w)),
            bottom: p.y >= h - self.border as f64 || (p.y >= h - corner && (p.x < 0.0 || p.x >= w)),
        };
        let in_title_bar =
            inside && p.y >= self.border as f64 && p.y < (self.border + self.title_height) as f64;
        if in_title_bar && !edges.left && !edges.right {
            for (b, r) in &self.buttons {
                if r.to_f64().contains(p) {
                    return Some(Part::Button(*b));
                }
            }
            return Some(Part::Title);
        }
        if edges.left || edges.right || edges.top || edges.bottom {
            return Some(Part::Border(edges));
        }
        Some(Part::Title)
    }
}

/// Image RVBA prémultipliée (ordre des octets R, V, B, A)
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Image {
    pub fn new(width: usize, height: usize) -> Image {
        Image {
            width,
            height,
            data: vec![0; width * height * 4],
        }
    }

    /// Pose un pixel de couleur `c` avec une couverture `cov` (0 à 1)
    fn blend(&mut self, x: i32, y: i32, c: Color, cov: f32) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let i = (y as usize * self.width + x as usize) * 4;
        let a = (c.a * cov).clamp(0.0, 1.0);
        let src = [c.r * a, c.g * a, c.b * a, a];
        for (k, s) in src.iter().enumerate() {
            let d = self.data[i + k] as f32 / 255.0;
            self.data[i + k] = ((s + d * (1.0 - a)) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    fn fill(&mut self, c: Color) {
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                self.blend(x, y, c, 1.0);
            }
        }
    }
}

/// Dessin du texte (cosmic-text)
pub struct Text {
    fonts: FontSystem,
    cache: SwashCache,
    /// Aucune police installée (vérifié une fois)
    no_fonts: Option<bool>,
}

impl Text {
    pub fn new() -> Text {
        Text {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
            no_fonts: None,
        }
    }

    /// Une police au moins est installée (sans elle, cosmic-text panique au
    /// placement du texte: le texte n'est alors pas écrit)
    fn has_fonts(&mut self) -> bool {
        if self.no_fonts.is_none() {
            let none = self.fonts.db().faces().next().is_none();
            if none {
                tracing::warn!("aucune police trouvée: titres et menus sans texte");
            }
            self.no_fonts = Some(none);
        }
        self.no_fonts == Some(false)
    }

    fn buffer(&mut self, text: &str, font: &Font, scale: f64) -> Buffer {
        let px = font_px(font) * scale as f32;
        let mut buffer = Buffer::new(
            &mut self.fonts,
            Metrics::new(px, line_height(font) as f32 * scale as f32),
        );
        let mut attrs = Attrs::new().family(Family::Name(&font.name));
        if font.bold {
            attrs = attrs.weight(Weight::BOLD);
        }
        if font.italic {
            attrs = attrs.style(Style::Italic);
        }
        buffer.set_size(&mut self.fonts, None, None);
        buffer.set_text(&mut self.fonts, text, &attrs, Shaping::Advanced);
        buffer.shape_until_scroll(&mut self.fonts, false);
        buffer
    }

    fn width(buffer: &Buffer) -> f32 {
        buffer.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    /// Largeur d'un texte, en pixels à l'échelle `scale`
    pub fn measure(&mut self, text: &str, font: &Font, scale: f64) -> f32 {
        if text.is_empty() || !self.has_fonts() {
            return 0.0;
        }
        Text::width(&self.buffer(text, font, scale))
    }

    /// Titre dans une image de `width` × `height` pixels physiques, aligné
    /// et tronqué avec « … »
    pub fn title(
        &mut self,
        title: &str,
        font: &Font,
        color: Color,
        justify: Justify,
        width: usize,
        height: usize,
        scale: f64,
    ) -> Image {
        let mut img = Image::new(width, height);
        if width == 0 || height == 0 || title.is_empty() || !self.has_fonts() {
            return img;
        }
        let mut text = title.to_string();
        let mut buffer = self.buffer(&text, font, scale);
        let mut chars: Vec<char> = title.chars().collect();
        while Text::width(&buffer) > width as f32 && !chars.is_empty() {
            // Retire environ ce qui dépasse, puis affine caractère par caractère
            let excess = (Text::width(&buffer) - width as f32) / Text::width(&buffer);
            let cut = ((chars.len() as f32 * excess) as usize).max(1);
            chars.truncate(chars.len().saturating_sub(cut));
            text = chars.iter().collect::<String>().trim_end().to_string() + "…";
            buffer = self.buffer(&text, font, scale);
        }
        let w = Text::width(&buffer);
        let x0 = match justify {
            Justify::Left => 0.0,
            Justify::Center => ((width as f32 - w) / 2.0).max(0.0),
            Justify::Right => (width as f32 - w).max(0.0),
        }
        .round() as i32;
        let line = buffer.layout_runs().next().map(|r| r.line_height).unwrap_or(0.0);
        let y0 = ((height as f32 - line) / 2.0).round() as i32;
        let base = cosmic_text::Color::rgba(255, 255, 255, 255);
        let Text { fonts, cache, .. } = self;
        buffer.draw(fonts, cache, base, |x, y, w, h, c| {
            let cov = c.a() as f32 / 255.0;
            for dy in 0..h as i32 {
                for dx in 0..w as i32 {
                    img.blend(x0 + x + dx, y0 + y + dy, color, cov);
                }
            }
        });
        img
    }
}

/// État d'un bouton
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Normal,
    Hover,
    Pressed,
}

/// Bouton dans une image de `width` × `height` pixels physiques: fond
/// (s'il diffère de la barre de titre) et image centrée, agrandie d'un
/// facteur entier
pub fn button_image(
    theme: &Theme,
    active: bool,
    button: Button,
    toggled: bool,
    state: ButtonState,
    width: usize,
    height: usize,
    scale: f64,
) -> Image {
    let colors = if active { &theme.active } else { &theme.inactive };
    let mut img = Image::new(width, height);
    let bg = match state {
        ButtonState::Normal => colors.button_bg,
        ButtonState::Hover => colors.button_hover_bg,
        ButtonState::Pressed => colors.button_pressed_bg,
    };
    if bg != colors.title_bg {
        img.fill(bg);
    }
    let (normal, toggled_mask) = &theme.masks[&button];
    let mask: &Mask = if toggled { toggled_mask } else { normal };
    let color = colors.button_image[&button][state as usize];
    let k = (scale.round() as usize).max(1);
    let (mw, mh) = (mask.width * k, mask.height * k);
    let x0 = (width.saturating_sub(mw) / 2) as i32;
    let y0 = (height.saturating_sub(mh) / 2) as i32;
    for y in 0..mh {
        for x in 0..mw {
            if mask.bits[(y / k) * mask.width + x / k] {
                img.blend(x0 + x as i32, y0 + y as i32, color, 1.0);
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeRc;

    fn layout() -> Layout {
        let mut rc = ThemeRc::default();
        rc.parse("border.width: 2\npadding.width: 8\npadding.height: 10\n");
        let theme = Theme::from_rc("test", &rc, None);
        Layout::new(&Config::default(), &theme, (400, 300).into(), Frame::FULL)
    }

    #[test]
    fn geometry() {
        let l = layout();
        assert_eq!(l.client.loc, Point::from((2, 2 + l.title_height)));
        assert_eq!(l.size, Size::from((404, 300 + l.title_height + 4)));
        // Boutons à droite, dans l'ordre iconify, max, close
        let xs: Vec<_> = l.buttons.iter().map(|(b, r)| (*b, r.loc.x)).collect();
        let close = xs.iter().find(|(b, _)| *b == Button::Close).unwrap().1;
        let max = xs.iter().find(|(b, _)| *b == Button::Maximize).unwrap().1;
        assert!(max < close);
        assert!(l.title.loc.x + l.title.size.w <= max);
    }

    #[test]
    fn parts() {
        let l = layout();
        let p = |x: f64, y: f64| l.part_at((x, y).into());
        assert_eq!(p(100.0, 10.0), Some(Part::Title));
        assert_eq!(p(100.0, 100.0), None, "client");
        assert_eq!(p(-50.0, 10.0), None, "dehors");
        let close = l.buttons.iter().find(|(b, _)| *b == Button::Close).unwrap().1;
        let c = close.to_f64();
        assert_eq!(p(c.loc.x + 2.0, c.loc.y + 2.0), Some(Part::Button(Button::Close)));
        let Some(Part::Border(e)) = p(-3.0, 100.0) else {
            panic!()
        };
        assert!(e.left && !e.top && !e.bottom);
        let Some(Part::Border(e)) = p(-3.0, -3.0) else {
            panic!()
        };
        assert!(e.left && e.top);
        let h = l.size.h as f64;
        let Some(Part::Border(e)) = p(200.0, h + 3.0) else {
            panic!()
        };
        assert!(e.bottom && !e.left && !e.right);
        let Some(Part::Border(e)) = p(200.0, -3.0) else {
            panic!()
        };
        assert!(e.top && !e.left);
    }
}
