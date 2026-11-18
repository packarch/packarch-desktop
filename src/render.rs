// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rendu (cahier des charges § 10): cadres arrondis, ombres, contenu des
//! fenêtres découpé aux coins, couches layer-shell.
//!
//! Chaque fenêtre décorée est d'abord rendue dans une texture à sa taille,
//! puis dessinée par un shader qui arrondit ses coins du bas: les
//! sous-surfaces sont découpées comme le reste, et la texture servira aux
//! animations (§ 10.5).

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::texture::TextureRenderElement;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{AsRenderElements, Id, Kind};
use smithay::backend::renderer::gles::element::{PixelShaderElement, TextureShaderElement};
use smithay::backend::renderer::gles::{
    GlesPixelProgram, GlesRenderer, GlesTarget, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::{Bind, Offscreen as _, Renderer, TextureFilter};
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::render_elements;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;
use tracing::warn;

use crate::anim::{Anim, lerp};
use crate::config::Button;
use crate::deco::{ButtonState, Layout, button_image};
use crate::menu::ui::SLIDE;
use crate::state::State;
use crate::theme::Color;

render_elements! {
    pub Base<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Pixel=PixelShaderElement,
    Clipped=TextureShaderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
}

render_elements! {
    pub Elem<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Pixel=PixelShaderElement,
    Clipped=TextureShaderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
    Anim=RelocateRenderElement<RescaleRenderElement<Base>>,
    Crop=CropRenderElement<Base>,
    Texture=TextureRenderElement<GlesTexture>,
}

/// Distance signée à un rectangle aux coins arrondis (rayon du haut et du
/// bas), anticrénelage sur un pixel physique (`px`, en unités logiques)
const SDF: &str = r#"
float rrect(vec2 p, vec2 origin, vec2 sz, float rt, float rb) {
    vec2 hs = sz * 0.5;
    vec2 q = p - origin - hs;
    float r = q.y < 0.0 ? rt : rb;
    r = min(r, min(hs.x, hs.y));
    vec2 d = abs(q) - hs + vec2(r);
    return length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - r;
}
float coverage(float d, float px) {
    return clamp(0.5 - d / px, 0.0, 1.0);
}
"#;

const HEADER: &str = r#"
precision highp float;
varying vec2 v_coords;
uniform vec2 size;
uniform float alpha;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
"#;

/// Cadre: bordure, barre de titre, trou du client
const FRAME: &str = r#"
uniform vec4 border_color;
uniform vec4 title_color;
uniform float radius_top;
uniform float radius_bottom;
uniform float border;
uniform float title_height;
uniform float px;
void main() {
    vec2 p = v_coords * size;
    float outer = coverage(rrect(p, vec2(0.0), size, radius_top, radius_bottom), px);
    vec2 o = vec2(border);
    vec2 s = size - 2.0 * border;
    float rti = max(radius_top - border, 0.0);
    float rbi = max(radius_bottom - border, 0.0);
    float title = coverage(rrect(p, o, vec2(s.x, title_height), rti, 0.0), px);
    float hole = coverage(rrect(p, o + vec2(0.0, title_height), vec2(s.x, s.y - title_height), 0.0, rbi), px);
    vec4 c = mix(border_color, title_color, title);
    gl_FragColor = c * outer * (1.0 - hole) * alpha;
}
"#;

/// Ombre: flou gaussien d'un rectangle arrondi, approché par la fonction
/// d'erreur de la distance à sa forme; rien sous la forme elle-même
const SHADOW: &str = r#"
uniform vec4 shadow_color;
uniform vec4 shape;
uniform float radius_top;
uniform float radius_bottom;
uniform float sigma;
uniform float px;
float erf_approx(float x) {
    float s = sign(x);
    x = abs(x);
    float t = 1.0 / (1.0 + 0.3275911 * x);
    float y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-x * x);
    return s * y;
}
void main() {
    vec2 p = v_coords * size;
    float d = rrect(p, shape.xy, shape.zw, radius_top, radius_bottom);
    float a = 0.5 - 0.5 * erf_approx(d / (sigma * 1.41421356));
    float outside = 1.0 - coverage(d, px);
    gl_FragColor = shadow_color * a * outside * alpha;
}
"#;

/// Contenu d'une fenêtre, coins du bas arrondis
const CLIP: &str = r#"
//_DEFINES_
#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif
precision highp float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif
uniform float alpha;
varying vec2 v_coords;
#if defined(DEBUG_FLAGS)
uniform float tint;
#endif
uniform vec2 geo_size;
uniform float radius;
uniform float px;
"#;

const CLIP_MAIN: &str = r#"
void main() {
    vec4 color = texture2D(tex, v_coords);
#if defined(NO_ALPHA)
    color = vec4(color.rgb, 1.0);
#endif
    vec2 p = v_coords * geo_size;
    color *= coverage(rrect(p, vec2(0.0), geo_size, 0.0, radius), px) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

pub struct Shaders {
    frame: GlesPixelProgram,
    shadow: GlesPixelProgram,
    clip: GlesTexProgram,
}

impl Shaders {
    pub fn compile(renderer: &mut GlesRenderer) -> anyhow::Result<Shaders> {
        let f1 = |n| UniformName::new(n, UniformType::_1f);
        let f4 = |n| UniformName::new(n, UniformType::_4f);
        let frame = renderer.compile_custom_pixel_shader(
            format!("{HEADER}{SDF}{FRAME}"),
            &[
                f4("border_color"),
                f4("title_color"),
                f1("radius_top"),
                f1("radius_bottom"),
                f1("border"),
                f1("title_height"),
                f1("px"),
            ],
        )?;
        let shadow = renderer.compile_custom_pixel_shader(
            format!("{HEADER}{SDF}{SHADOW}"),
            &[
                f4("shadow_color"),
                f4("shape"),
                f1("radius_top"),
                f1("radius_bottom"),
                f1("sigma"),
                f1("px"),
            ],
        )?;
        let clip = renderer.compile_custom_texture_shader(
            format!("{CLIP}{SDF}{CLIP_MAIN}"),
            &[
                UniformName::new("geo_size", UniformType::_2f),
                f1("radius"),
                f1("px"),
            ],
        )?;
        Ok(Shaders { frame, shadow, clip })
    }
}

/// Rendu hors écran du contenu d'une fenêtre
pub struct WindowTexture {
    texture: GlesTexture,
    damage: OutputDamageTracker,
    size: Size<i32, Physical>,
    scale: f64,
    id: Id,
    rendered: bool,
}

/// Images du cadre déjà dessinées, refaites quand leur clé change
pub struct DecoCache {
    key: DecoKey,
    title: Option<(MemoryRenderBuffer, Size<i32, Logical>)>,
    buttons: Vec<(Rectangle<i32, Logical>, (MemoryRenderBuffer, Size<i32, Logical>))>,
}

#[derive(Clone, PartialEq)]
struct DecoKey {
    layout: Layout,
    title: String,
    active: bool,
    maximized: bool,
    hovered: Option<Button>,
    pressed: Option<Button>,
    scale: u64,
}

/// Image en mémoire et sa taille en pixels (servant de zone source)
fn mem_buffer(img: crate::deco::Image) -> (MemoryRenderBuffer, Size<i32, Logical>) {
    let size = Size::from((img.width as i32, img.height as i32));
    let buf = MemoryRenderBuffer::from_slice(
        &img.data,
        Fourcc::Abgr8888,
        (img.width as i32, img.height as i32),
        1,
        Transform::Normal,
        None,
    );
    (buf, size)
}

fn px(scale: f64) -> f32 {
    (1.0 / scale) as f32
}

fn pixel(
    program: &GlesPixelProgram,
    area: Rectangle<i32, Logical>,
    alpha: f32,
    uniforms: Vec<Uniform<'_>>,
) -> PixelShaderElement {
    PixelShaderElement::new(program.clone(), area, None, alpha, uniforms, Kind::Unspecified)
}

/// Programmes des shaders, copiés pour la durée d'un rendu
#[derive(Clone)]
struct Progs {
    frame: GlesPixelProgram,
    shadow: GlesPixelProgram,
    clip: GlesTexProgram,
}

/// Paramètres du shader du cadre
#[derive(Clone)]
struct FrameParams {
    border_color: Color,
    title_color: Color,
    radius: f32,
    border: f32,
    title_height: f32,
}

/// Contenu d'une vue rendu hors écran
#[derive(Clone)]
struct Content {
    texture: GlesTexture,
    id: Id,
    /// Géométrie du client, coordonnées globales
    rect: Rectangle<i32, Logical>,
    radius: f32,
}

#[derive(Clone)]
struct ShadowParams {
    color: Color,
    size: i32,
    radius: f32,
}

/// Tout ce qu'il faut pour redessiner une vue telle qu'elle était affichée:
/// sert aux animations de fermeture et de réduction, quand le client n'a
/// plus d'image
#[derive(Clone)]
pub struct Snapshot {
    /// Cadre, coordonnées globales
    pub frame: Rectangle<i32, Logical>,
    /// Images du titre et des boutons: tampon, taille en pixels, destination
    deco: Vec<(MemoryRenderBuffer, Size<i32, Logical>, Rectangle<i32, Logical>)>,
    frame_params: Option<FrameParams>,
    content: Option<Content>,
    shadow: Option<ShadowParams>,
}

/// Vue fermée ou réduite, animée jusqu'à disparaître (§ 10.5)
pub struct Ghost {
    pub snapshot: Snapshot,
    pub anim: Anim,
    pub kind: GhostKind,
    /// Vue réduite (une restauration interrompt l'animation)
    pub window: Option<smithay::desktop::Window>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GhostKind {
    Close,
    /// Vers ce point (bas de l'écran)
    Minimize(Point<i32, Logical>),
}

/// Apparition d'une vue
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Appear {
    Open,
    /// Depuis ce point (restauration d'une vue réduite)
    Restore(Point<i32, Logical>),
}

/// Transformation d'une image animée
#[derive(Debug, Clone, Copy)]
struct Look {
    alpha: f32,
    scale: f64,
    /// Décalage du centre, logique
    offset: Point<f64, Logical>,
}

impl Look {
    fn none() -> Look {
        Look {
            alpha: 1.0,
            scale: 1.0,
            offset: Point::from((0.0, 0.0)),
        }
    }

    fn is_identity(&self) -> bool {
        self.alpha >= 1.0 && self.scale == 1.0 && self.offset == Point::from((0.0, 0.0))
    }
}

/// Apparence d'une vue qui apparaît, à la valeur `p` de son animation
fn appear_look(kind: Appear, frame: Rectangle<i32, Logical>, p: f64) -> Look {
    let p = p.max(0.0);
    match kind {
        Appear::Open => Look {
            alpha: p.min(1.0) as f32,
            scale: lerp(0.94, 1.0, p),
            offset: Point::from((0.0, 0.0)),
        },
        Appear::Restore(from) => minimize_look(frame, from, 1.0 - p),
    }
}

/// Réduction: fondu et rétrécissement vers `to`, à la progression `p`
fn minimize_look(frame: Rectangle<i32, Logical>, to: Point<i32, Logical>, p: f64) -> Look {
    let p = p.clamp(0.0, 1.0);
    let center = frame.loc.to_f64() + frame.size.to_f64().to_point().downscale(2.0);
    let d = to.to_f64() - center;
    Look {
        alpha: (1.0 - p) as f32,
        scale: lerp(1.0, 0.3, p),
        offset: Point::from((d.x * p, d.y * p)),
    }
}

/// Applique une transformation aux éléments d'une image, autour du centre
/// de `frame`
fn transform(
    elements: Vec<Base>,
    look: Look,
    frame: Rectangle<i32, Logical>,
    output_loc: Point<i32, Logical>,
    scale: f64,
) -> Vec<Elem> {
    if look.scale == 1.0 && look.offset == Point::from((0.0, 0.0)) {
        return elements.into_iter().map(Elem::from).collect();
    }
    let center = (frame.loc - output_loc).to_f64() + frame.size.to_f64().to_point().downscale(2.0);
    let origin = center.to_physical(scale).to_i32_round();
    let offset = look.offset.to_physical(scale).to_i32_round();
    elements
        .into_iter()
        .map(|e| {
            Elem::Anim(RelocateRenderElement::from_element(
                RescaleRenderElement::from_element(e, origin, look.scale),
                offset,
                Relocate::Relative,
            ))
        })
        .collect()
}

impl From<Base> for Elem {
    fn from(b: Base) -> Elem {
        match b {
            Base::Surface(e) => Elem::Surface(e),
            Base::Pixel(e) => Elem::Pixel(e),
            Base::Clipped(e) => Elem::Clipped(e),
            Base::Memory(e) => Elem::Memory(e),
        }
    }
}

/// Texture où un écran est rendu avant d'être agrandi par la loupe
pub struct MagnifierTexture {
    texture: GlesTexture,
    size: Size<i32, Physical>,
    damage: OutputDamageTracker,
}

/// Éléments d'un écran, du premier plan vers l'arrière-plan, avec la loupe
/// devant s'il le faut
pub fn output_elements(state: &mut State, renderer: &mut GlesRenderer, output: &Output) -> Vec<Elem> {
    let scene = scene_elements(state, renderer, output);
    // Pixels nets dans la loupe si demandé (s'applique à tout l'écran pendant
    // qu'elle est ouverte)
    let filter = if state.magnifier.is_some() && !state.config.magnifier.use_filter {
        TextureFilter::Nearest
    } else {
        TextureFilter::Linear
    };
    let _ = renderer.upscale_filter(filter);
    match magnifier_elements(state, renderer, output, &scene) {
        Ok(Some(mut front)) => {
            front.extend(scene);
            front
        }
        Ok(None) => scene,
        Err(e) => {
            warn!("loupe: {e}");
            scene
        }
    }
}

/// Loupe sur l'écran sous le pointeur: l'image de l'écran est rendue dans
/// une texture, dont une partie est dessinée agrandie, avec une bordure
fn magnifier_elements(
    state: &mut State,
    renderer: &mut GlesRenderer,
    output: &Output,
    scene: &[Elem],
) -> anyhow::Result<Option<Vec<Elem>>> {
    let Some(zoom) = state.magnifier else {
        state.magnifier_textures.clear();
        return Ok(None);
    };
    let geo = state.output_geometry(output);
    if !geo.to_f64().contains(state.pointer) || state.output_off(output) {
        return Ok(None);
    }
    let scale = output.current_scale().fractional_scale();
    let size = geo.size.to_f64().to_physical(scale).to_i32_round::<i32>();
    if size.w <= 0 || size.h <= 0 {
        return Ok(None);
    }
    state
        .magnifier_textures
        .retain(|(o, t)| o != output || t.size == size);
    if !state.magnifier_textures.iter().any(|(o, _)| o == output) {
        let texture: GlesTexture = renderer.create_buffer(
            Fourcc::Abgr8888,
            size.to_logical(1).to_buffer(1, Transform::Normal),
        )?;
        let damage = OutputDamageTracker::new(size, scale, Transform::Normal);
        state.magnifier_textures.push((
            output.clone(),
            MagnifierTexture {
                texture,
                size,
                damage,
            },
        ));
    }
    let tex = &mut state
        .magnifier_textures
        .iter_mut()
        .find(|(o, _)| o == output)
        .expect("texture de la loupe")
        .1;
    {
        let mut target: GlesTarget = renderer.bind(&mut tex.texture)?;
        tex.damage
            .render_output(renderer, &mut target, 0, scene, CLEAR.premultiplied())?;
    }
    let texture = tex.texture.clone();
    let cfg = &state.config.magnifier;
    let p = state.pointer - geo.loc.to_f64();
    let (dst, src) = crate::magnifier::magnifier_rects(geo.size, p, cfg, zoom);
    // Partie de la source sur l'écran; la loupe garde sa taille, le reste
    // est rempli de la couleur de fond
    let screen = Rectangle::from_size(geo.size.to_f64());
    let mut out = vec![];
    let Some(shaders) = state.shaders.as_ref() else {
        return Ok(None);
    };
    let frame = dst.to_i32_round::<i32>();
    let bw = state.theme.magnifier_border_width;
    if bw > 0 && frame.size != geo.size {
        out.push(Elem::Pixel(pixel(
            &shaders.frame,
            Rectangle::new(
                frame.loc - Point::from((bw, bw)),
                frame.size + Size::from((2 * bw, 2 * bw)),
            ),
            1.0,
            vec![
                Uniform::new("border_color", state.theme.magnifier_border_color.premultiplied()),
                Uniform::new("title_color", [0.0f32; 4]),
                Uniform::new("radius_top", 0.0f32),
                Uniform::new("radius_bottom", 0.0f32),
                Uniform::new("border", bw as f32),
                Uniform::new("title_height", 0.0f32),
                Uniform::new("px", px(scale)),
            ],
        )));
    }
    if let Some(visible) = src.intersection(screen) {
        let shown = Rectangle::new(
            dst.loc + (visible.loc - src.loc).upscale(zoom),
            visible.size.upscale(zoom),
        );
        // Texture à l'échelle 1: la source est en pixels physiques
        let src_px = Rectangle::new(visible.loc.upscale(scale), visible.size.upscale(scale));
        out.push(Elem::Texture(TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            shown.loc.to_physical(scale),
            texture,
            1,
            Transform::Normal,
            None,
            Some(src_px),
            Some(shown.size.to_i32_round()),
            None,
            Kind::Unspecified,
        )));
    }
    out.push(Elem::Pixel(pixel(
        &shaders.frame,
        frame,
        1.0,
        vec![
            Uniform::new("border_color", CLEAR.premultiplied()),
            Uniform::new("title_color", CLEAR.premultiplied()),
            Uniform::new("radius_top", 0.0f32),
            Uniform::new("radius_bottom", 0.0f32),
            Uniform::new("border", frame.size.w.max(frame.size.h) as f32),
            Uniform::new("title_height", 0.0f32),
            Uniform::new("px", px(scale)),
        ],
    )));
    Ok(Some(out))
}

/// Éléments de la scène d'un écran, du premier plan vers l'arrière-plan
fn scene_elements(state: &mut State, renderer: &mut GlesRenderer, output: &Output) -> Vec<Elem> {
    let scale = output.current_scale().fractional_scale();
    let output_loc = state.output_location(output);
    let mut elements: Vec<Elem> = vec![];
    let Some(shaders) = state.shaders.as_ref() else {
        return elements;
    };
    let progs = Progs {
        frame: shaders.frame.clone(),
        shadow: shaders.shadow.clone(),
        clip: shaders.clip.clone(),
    };
    let now = state.now();

    // Écran éteint (wlopm): noir
    if state.output_off(output) {
        return elements;
    }

    // Verrouillé: seulement la surface de verrouillage de l'écran
    if state.is_locked() {
        if let Some(l) = state.lock_surface(output) {
            elements.extend(render_elements_from_surface_tree(
                renderer,
                l.wl_surface(),
                Point::<i32, Physical>::from((0, 0)),
                scale,
                1.0,
                Kind::Unspecified,
            ));
        }
        return elements;
    }

    let layer_elements = |renderer: &mut GlesRenderer, layer: Layer| -> Vec<Elem> {
        let map = layer_map_for_output(output);
        let mut out = vec![];
        for surface in map.layers_on(layer).rev() {
            let Some(geo) = map.layer_geometry(surface) else {
                continue;
            };
            let loc = geo.loc.to_physical_precise_round(scale);
            out.extend(surface.render_elements::<Elem>(renderer, loc, Scale::from(scale), 1.0));
        }
        out
    };

    // Menus et OSD, devant tout le reste
    elements.extend(menu_elements(state, renderer, scale, output_loc, &progs, now));
    elements.extend(osd_elements(state, renderer, output, scale, &progs, now));

    // Fenêtres X11 sans cadre (menus, bulles), devant les vues
    for (surface, geo) in state.unmanaged_surfaces().into_iter().rev() {
        let loc = (geo.loc - output_loc).to_physical_precise_round(scale);
        elements.extend(render_elements_from_surface_tree(
            renderer,
            &surface,
            loc,
            scale,
            1.0,
            Kind::Unspecified,
        ));
    }

    let stacking = state.stacking();
    let full = state.fullscreen_top();
    elements.extend(layer_elements(renderer, Layer::Overlay));
    if let Some(i) = full {
        elements.extend(animated_view(
            state, renderer, i, scale, output_loc, &progs, now, 0.0,
        ));
    }
    elements.extend(layer_elements(renderer, Layer::Top));

    // Aperçu du collage, devant les fenêtres: fondu et croissance à chaque
    // nouveau rectangle
    if state.snap_preview.is_none() {
        state.snap_shown = None;
    }
    if let Some((full, anim)) = state.snap_shown {
        let p = match anim {
            Some(a) if !a.done(now) => a.value(now),
            _ => 1.0,
        };
        let k = lerp(0.9, 1.0, p);
        let size = full.size.to_f64().upscale(k).to_i32_round::<i32>();
        let rect = Rectangle::new(full.loc + (full.size - size).to_point().downscale(2), size);
        let alpha = p.clamp(0.0, 1.0) as f32;
        let c = state.theme.active.title_bg;
        let fill = crate::theme::Color::rgba(c.r, c.g, c.b, 0.35);
        let border = state.theme.active.label_text;
        let radius = state.config.corner_radius as f32;
        elements.push(Elem::Pixel(pixel(
            &progs.frame,
            Rectangle::new(rect.loc - output_loc, rect.size),
            alpha,
            vec![
                Uniform::new(
                    "border_color",
                    crate::theme::Color::rgba(border.r, border.g, border.b, 0.5).premultiplied(),
                ),
                Uniform::new("title_color", fill.premultiplied()),
                Uniform::new("radius_top", radius),
                Uniform::new("radius_bottom", radius),
                Uniform::new("border", 2.0f32),
                Uniform::new("title_height", (rect.size.h - 4).max(0) as f32),
                Uniform::new("px", px(scale)),
            ],
        )));
    }

    // Vues fermées ou réduites, encore animées
    state.ghosts.retain(|g| !g.anim.done(now));
    for g in state.ghosts.iter().rev() {
        let p = g.anim.value(now);
        let look = match g.kind {
            GhostKind::Close => Look {
                alpha: (1.0 - p).clamp(0.0, 1.0) as f32,
                scale: lerp(1.0, 0.94, p),
                offset: Point::from((0.0, 0.0)),
            },
            GhostKind::Minimize(to) => minimize_look(g.snapshot.frame, to, p),
        };
        let base = snapshot_elements(&g.snapshot, renderer, &progs, scale, output_loc, look.alpha, None);
        elements.extend(transform(base, look, g.snapshot.frame, output_loc, scale));
    }

    // Contour d'un redimensionnement sans dessin du contenu
    if let Some(f) = state.resize_outline {
        let c = state.theme.osd.hilight_bg;
        let radius = state.config.corner_radius as f32;
        elements.push(Elem::Pixel(pixel(
            &progs.frame,
            Rectangle::new(f.loc - output_loc, f.size),
            1.0,
            vec![
                Uniform::new("border_color", c.premultiplied()),
                Uniform::new("title_color", [0.0f32; 4]),
                Uniform::new("radius_top", radius),
                Uniform::new("radius_bottom", radius),
                Uniform::new("border", 2.0f32),
                Uniform::new("title_height", 0.0f32),
                Uniform::new("px", px(scale)),
            ],
        )));
    }

    // Contour de la vue choisie pendant Alt+Tab
    if state.config.switcher.outlines
        && let Some(i) = state
            .cycle
            .as_ref()
            .and_then(|c| c.windows.get(c.index))
            .and_then(|w| state.window_index(w))
        && state.visible(&state.views[i])
    {
        let f = state.frame_rect(i);
        let c = state.theme.osd.hilight_bg;
        let radius = state.config.corner_radius as f32;
        elements.push(Elem::Pixel(pixel(
            &progs.frame,
            Rectangle::new(
                f.loc - output_loc - Point::from((2, 2)),
                f.size + Size::from((4, 4)),
            ),
            1.0,
            vec![
                Uniform::new("border_color", c.premultiplied()),
                Uniform::new("title_color", [0.0f32; 4]),
                Uniform::new("radius_top", radius + 2.0),
                Uniform::new("radius_bottom", radius + 2.0),
                Uniform::new("border", 3.0f32),
                Uniform::new("title_height", 0.0f32),
                Uniform::new("px", px(scale)),
            ],
        )));
    }

    // Changement de bureau: les vues de l'ancien bureau sortent, celles du
    // nouveau entrent, d'un écran de large (§ 10.5)
    let width = state.output_geometry(output).size.w as f64;
    let slide = match state.desk_anim {
        Some((a, from)) if !a.done(now) => {
            let p = a.value(now);
            let dir = if state.desktop > from { 1.0 } else { -1.0 };
            Some((p, dir, from))
        }
        Some(_) => {
            state.desk_anim = None;
            None
        }
        None => None,
    };
    for &i in stacking.iter().rev() {
        if Some(i) != full {
            let shift = match slide {
                Some((p, dir, _)) if !state.views[i].omnipresent => dir * width * (1.0 - p),
                _ => 0.0,
            };
            elements.extend(animated_view(
                state, renderer, i, scale, output_loc, &progs, now, shift,
            ));
        }
    }
    if let Some((p, dir, from)) = slide {
        let mut old: Vec<usize> = (0..state.views.len())
            .filter(|&i| {
                let v = &state.views[i];
                v.mapped && !v.minimized && v.on_desktop(from) && !v.on_desktop(state.desktop)
            })
            .collect();
        old.sort_by_key(|&i| state.views[i].stack);
        for &i in old.iter().rev() {
            elements.extend(animated_view(
                state,
                renderer,
                i,
                scale,
                output_loc,
                &progs,
                now,
                -dir * width * p,
            ));
        }
    }

    elements.extend(layer_elements(renderer, Layer::Bottom));
    elements.extend(layer_elements(renderer, Layer::Background));
    elements
}

/// Éléments d'une vue, avec son animation d'apparition éventuelle
fn animated_view(
    state: &mut State,
    renderer: &mut GlesRenderer,
    i: usize,
    scale: f64,
    output_loc: Point<i32, Logical>,
    progs: &Progs,
    now: std::time::Duration,
    shift: f64,
) -> Vec<Elem> {
    let frame = state.frame_rect(i);
    let mut look = match state.views[i].appear {
        Some((anim, kind)) if !anim.done(now) => appear_look(kind, frame, anim.value(now)),
        Some(_) => {
            state.views[i].appear = None;
            Look::none()
        }
        None => Look::none(),
    };
    look.offset.x += shift;
    // Rectangle animé (maximisation, collage): l'image actuelle est étirée
    // du cadre affiché au rectangle intermédiaire
    let geom = match state.views[i].geom_anim {
        Some((a, from, to)) if !a.done(now) && look.is_identity() => {
            let p = a.value(now);
            let r = |a: i32, b: i32| lerp(a as f64, b as f64, p);
            Some(Rectangle::<f64, Logical>::new(
                (r(from.loc.x, to.loc.x), r(from.loc.y, to.loc.y)).into(),
                (r(from.size.w, to.size.w), r(from.size.h, to.size.h)).into(),
            ))
        }
        Some(_) => {
            state.views[i].geom_anim = None;
            None
        }
        None => None,
    };
    // Enroulement: l'image est découpée à la hauteur animée
    let shade = match state.views[i].shade_anim.take() {
        Some((a, shading, full)) if !a.done(now) => {
            let p = a.value(now).clamp(0.0, 1.0);
            let short = frame
                .size
                .h
                .min(full.as_ref().map(|f| f.frame.size.h).unwrap_or(frame.size.h));
            let layout = state.layout(&state.views[i]);
            let title_only = layout.title_height + 2 * layout.border;
            let (lo, hi) = if shading {
                (
                    short,
                    full.as_ref().map(|f| f.frame.size.h).unwrap_or(frame.size.h),
                )
            } else {
                (title_only.min(frame.size.h), frame.size.h)
            };
            let h = if shading {
                lerp(hi as f64, lo as f64, p)
            } else {
                lerp(lo as f64, hi as f64, p)
            };
            state.views[i].shade_anim = Some((a, shading, full.clone()));
            Some((h.round() as i32, shading, full))
        }
        _ => None,
    };
    if let Some((h, shading, full)) = shade {
        let snap_frame = full.as_ref().map(|f| f.frame).unwrap_or(frame);
        let elements = match (shading, full) {
            (true, Some(f)) => snapshot_elements(&f, renderer, progs, scale, output_loc, 1.0, None),
            _ => view_elements(state, renderer, i, scale, output_loc, progs, 1.0),
        };
        let crop = Rectangle::<i32, Logical>::new(snap_frame.loc - output_loc, (snap_frame.size.w, h).into())
            .to_physical_precise_round(scale);
        return elements
            .into_iter()
            .filter_map(|e| CropRenderElement::from_element(e, scale, crop).map(Elem::Crop))
            .collect();
    }
    let base = view_elements(state, renderer, i, scale, output_loc, progs, look.alpha);
    if let Some(target) = geom {
        let f = frame.to_f64();
        let sx = target.size.w / f.size.w.max(1.0);
        let sy = target.size.h / f.size.h.max(1.0);
        let origin = (frame.loc - output_loc).to_physical_precise_round(scale);
        let offset = (target.loc - f.loc).to_physical(scale).to_i32_round();
        return base
            .into_iter()
            .map(|e| {
                Elem::Anim(RelocateRenderElement::from_element(
                    RescaleRenderElement::from_element(e, origin, (sx, sy)),
                    offset,
                    Relocate::Relative,
                ))
            })
            .collect();
    }
    if look.is_identity() {
        base.into_iter().map(Elem::from).collect()
    } else {
        transform(base, look, frame, output_loc, scale)
    }
}

/// Menus ouverts et menus qui disparaissent: image et ombre (§ 10.4)
fn menu_elements(
    state: &mut State,
    renderer: &mut GlesRenderer,
    scale: f64,
    output_loc: Point<i32, Logical>,
    progs: &Progs,
    now: std::time::Duration,
) -> Vec<Elem> {
    state.menu_images(scale);
    let mut elements = vec![];
    let t = state.theme.menu.clone();
    let mut push = |elements: &mut Vec<Elem>,
                    image: &crate::menu::ui::MenuImage,
                    rect: Rectangle<i32, Logical>,
                    alpha: f32,
                    dy: f64| {
        let loc = (rect.loc - output_loc).to_f64() + Point::from((0.0, dy));
        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            loc.to_physical(scale),
            &image.buffer,
            Some(alpha),
            Some(Rectangle::from_size(image.px.to_f64())),
            Some(rect.size),
            Kind::Unspecified,
        ) {
            Ok(e) => elements.push(Elem::Memory(e)),
            Err(e) => warn!("image du menu: {e:?}"),
        }
        let s = t.shadow_size;
        if s > 0 && t.shadow_color.a > 0.0 {
            let r = t.corner_radius as f32;
            let area = Rectangle::new(
                (rect.loc - output_loc) + Point::from((-s, dy.round() as i32 - s)),
                rect.size + Size::from((2 * s, 2 * s)),
            );
            elements.push(Elem::Pixel(shadow(
                progs,
                area,
                s,
                rect.size,
                r,
                t.shadow_color,
                alpha,
                scale,
            )));
        }
    };
    if let Some(ms) = state.menu.as_mut() {
        for level in ms.levels.iter_mut().rev() {
            let p = match level.anim {
                Some(a) if !a.done(now) => a.value(now),
                _ => {
                    level.anim = None;
                    1.0
                }
            };
            if let Some(image) = &level.image {
                let alpha = p.clamp(0.0, 1.0) as f32;
                push(&mut elements, image, level.rect(), alpha, -SLIDE * (1.0 - p));
            }
        }
    }
    state.menu_ghosts.retain(|g| !g.anim.done(now));
    for g in state.menu_ghosts.iter().rev() {
        let p = g.anim.value(now);
        push(
            &mut elements,
            &g.image,
            g.rect,
            (1.0 - p).clamp(0.0, 1.0) as f32,
            -SLIDE * p,
        );
    }
    elements
}

/// Image flottante (OSD) et son ombre
fn floating(
    renderer: &mut GlesRenderer,
    progs: &Progs,
    buffer: &MemoryRenderBuffer,
    px_size: Size<i32, Logical>,
    rect: Rectangle<i32, Logical>,
    alpha: f32,
    scale: f64,
    t: &crate::theme::OsdTheme,
) -> Vec<Elem> {
    let mut out = vec![];
    match MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        rect.loc.to_f64().to_physical(scale),
        buffer,
        Some(alpha),
        Some(Rectangle::from_size(px_size.to_f64())),
        Some(rect.size),
        Kind::Unspecified,
    ) {
        Ok(e) => out.push(Elem::Memory(e)),
        Err(e) => warn!("image de l'OSD: {e:?}"),
    }
    let s = t.shadow_size;
    if s > 0 && t.shadow_color.a > 0.0 {
        let area = Rectangle::new(
            rect.loc - Point::from((s, s)),
            rect.size + Size::from((2 * s, 2 * s)),
        );
        out.push(Elem::Pixel(shadow(
            progs,
            area,
            s,
            rect.size,
            t.corner_radius as f32,
            t.shadow_color,
            alpha,
            scale,
        )));
    }
    out
}

/// OSD (Alt+Tab, nom du bureau), centré sur l'écran, et celui qui disparaît
fn osd_elements(
    state: &mut State,
    renderer: &mut GlesRenderer,
    output: &Output,
    scale: f64,
    progs: &Progs,
    now: std::time::Duration,
) -> Vec<Elem> {
    if state
        .osd
        .as_ref()
        .and_then(|o| o.hide_at)
        .is_some_and(|h| now >= h)
    {
        state.hide_osd();
    }
    let geo = state.output_geometry(output);
    let size = geo.size;
    state.osd_image(scale, size);
    // Centré sur l'écran, ou sur le point demandé (taille d'une vue)
    let at = state.osd_center().map(|c| c - geo.loc);
    let center = move |s: Size<i32, Logical>| match at {
        Some(c) => Rectangle::new(c - s.to_point().downscale(2), s),
        None => Rectangle::new(Point::from(((size.w - s.w) / 2, (size.h - s.h) / 2)), s),
    };
    let t = state.theme.osd.clone();
    let mut out = vec![];
    if let Some(o) = state.osd.as_mut() {
        let p = match o.anim {
            Some(a) if !a.done(now) => a.value(now),
            _ => {
                o.anim = None;
                1.0
            }
        };
        if let Some(img) = &o.image {
            out.extend(floating(
                renderer,
                progs,
                &img.buffer,
                img.px,
                center(img.size),
                p.clamp(0.0, 1.0) as f32,
                scale,
                &t,
            ));
        }
    }
    if state.osd_ghost.as_ref().is_some_and(|g| g.anim.done(now)) {
        state.osd_ghost = None;
    }
    if let Some(g) = &state.osd_ghost {
        let p = g.anim.value(now);
        out.extend(floating(
            renderer,
            progs,
            &g.image.buffer,
            g.image.px,
            center(g.image.size),
            (1.0 - p).clamp(0.0, 1.0) as f32,
            scale,
            &t,
        ));
    }
    out
}

/// Ombre d'un rectangle arrondi de taille `size`, placée dans `area`
fn shadow(
    progs: &Progs,
    area: Rectangle<i32, Logical>,
    s: i32,
    size: Size<i32, Logical>,
    radius: f32,
    color: Color,
    alpha: f32,
    scale: f64,
) -> PixelShaderElement {
    pixel(
        &progs.shadow,
        area,
        alpha,
        vec![
            Uniform::new("shadow_color", color.premultiplied()),
            Uniform::new("shape", [s as f32, s as f32, size.w as f32, size.h as f32]),
            Uniform::new("radius_top", radius),
            Uniform::new("radius_bottom", radius),
            Uniform::new("sigma", (s as f32 / 2.0).max(0.5)),
            Uniform::new("px", px(scale)),
        ],
    )
}

/// Éléments d'un instantané: titre et boutons, cadre, contenu (ou `live`,
/// les surfaces du client), ombre
fn snapshot_elements(
    snap: &Snapshot,
    renderer: &mut GlesRenderer,
    progs: &Progs,
    scale: f64,
    output_loc: Point<i32, Logical>,
    alpha: f32,
    live: Option<Vec<Base>>,
) -> Vec<Base> {
    let mut elements: Vec<Base> = vec![];
    for (buf, px_size, rect) in &snap.deco {
        let loc = (rect.loc - output_loc).to_f64().to_physical(scale);
        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            loc,
            buf,
            Some(alpha),
            Some(Rectangle::from_size(px_size.to_f64())),
            Some(rect.size),
            Kind::Unspecified,
        ) {
            Ok(e) => elements.push(Base::Memory(e)),
            Err(e) => warn!("image du cadre: {e:?}"),
        }
    }
    let frame_loc = snap.frame.loc - output_loc;
    if let Some(f) = &snap.frame_params {
        elements.push(Base::Pixel(pixel(
            &progs.frame,
            Rectangle::new(frame_loc, snap.frame.size),
            alpha,
            vec![
                Uniform::new("border_color", f.border_color.premultiplied()),
                Uniform::new("title_color", f.title_color.premultiplied()),
                Uniform::new("radius_top", f.radius),
                Uniform::new("radius_bottom", f.radius),
                Uniform::new("border", f.border),
                Uniform::new("title_height", f.title_height),
                Uniform::new("px", px(scale)),
            ],
        )));
    }
    if let Some(c) = &snap.content {
        let size = c.rect.size;
        let tex = TextureRenderElement::from_static_texture(
            c.id.clone(),
            renderer.context_id(),
            (c.rect.loc - output_loc).to_f64().to_physical(scale),
            c.texture.clone(),
            1,
            Transform::Normal,
            Some(alpha),
            None,
            Some(size),
            None,
            Kind::Unspecified,
        );
        elements.push(Base::Clipped(TextureShaderElement::new(
            tex,
            progs.clip.clone(),
            vec![
                Uniform::new("geo_size", (size.w as f32, size.h as f32)),
                Uniform::new("radius", c.radius),
                Uniform::new("px", px(scale)),
            ],
        )));
    }
    if let Some(live) = live {
        elements.extend(live);
    }
    if let Some(s) = &snap.shadow {
        let area = Rectangle::new(
            frame_loc - Point::from((s.size, s.size)),
            snap.frame.size + Size::from((2 * s.size, 2 * s.size)),
        );
        elements.push(Base::Pixel(shadow(
            progs,
            area,
            s.size,
            snap.frame.size,
            s.radius,
            s.color,
            alpha,
            scale,
        )));
    }
    elements
}

/// Éléments d'une vue: fenêtres surgissantes, titre et boutons, cadre,
/// contenu, ombre. L'instantané de la vue est gardé pour les animations
fn view_elements(
    state: &mut State,
    renderer: &mut GlesRenderer,
    i: usize,
    scale: f64,
    output_loc: Point<i32, Logical>,
    progs: &Progs,
    alpha: f32,
) -> Vec<Base> {
    let mut elements: Vec<Base> = vec![];
    let focused = state.focused_root();
    let layout = state.layout(&state.views[i]);
    let config = &state.config;
    let theme = &state.theme;
    let view = &state.views[i];
    let geo = view.window.geometry();
    let active = focused.as_ref() == Some(view.surface());
    let free = view.is_free();
    let colors = if active { &theme.active } else { &theme.inactive };
    let frame_global = Rectangle::new(view.loc - layout.client.loc, layout.size);
    let client_loc = view.loc - output_loc;
    let radius = if free { config.corner_radius } else { 0 } as f32;
    let shaded = view.shaded;
    let fullscreen = view.tiling == crate::view::Tiling::Fullscreen;

    // Fenêtres surgissantes (menus des applications), devant le cadre
    let surface = view.surface().clone();
    if !shaded {
        for (popup, offset) in smithay::desktop::PopupManager::popups_for_surface(&surface) {
            let loc = (client_loc + offset - popup.geometry().loc).to_physical_precise_round(scale);
            elements.extend(render_elements_from_surface_tree(
                renderer,
                popup.wl_surface(),
                loc,
                scale,
                alpha,
                Kind::Unspecified,
            ));
        }
    }

    // Titre et boutons
    let key = DecoKey {
        layout: layout.clone(),
        title: view.title(),
        active,
        maximized: view.tiling == crate::view::Tiling::Maximized(crate::view::Axis::Both),
        hovered: view.hovered,
        pressed: view.pressed,
        scale: scale.to_bits(),
    };
    let frame_params = (layout.size != layout.client.size).then_some(FrameParams {
        border_color: colors.border,
        title_color: colors.title_bg,
        radius,
        border: layout.border as f32,
        title_height: layout.title_height as f32,
    });
    let shadow_size = colors.shadow_size;
    let shadow_color = colors.shadow_color;
    let shadows = config.drop_shadows && (free || config.drop_shadows_on_tiled) && !fullscreen;
    let shadow = (shadows && shadow_size > 0 && shadow_color.a > 0.0).then_some(ShadowParams {
        color: shadow_color,
        size: shadow_size,
        radius,
    });
    let use_offscreen =
        !shaded && !fullscreen && (radius > layout.border as f32 || config.animations.enabled);
    if layout.title_height > 0 && state.views[i].deco.as_ref().is_none_or(|d| d.key != key) {
        let deco = build_deco(&mut state.text, &state.config, &state.theme, key);
        state.views[i].deco = Some(deco);
    }
    let view = &mut state.views[i];
    let mut deco = vec![];
    if layout.title_height > 0
        && let Some(d) = view.deco.as_ref()
    {
        for (rect, (buf, px_size)) in &d.buttons {
            deco.push((
                buf.clone(),
                *px_size,
                Rectangle::new(frame_global.loc + rect.loc, rect.size),
            ));
        }
        if let Some((buf, px_size)) = &d.title {
            deco.push((
                buf.clone(),
                *px_size,
                Rectangle::new(frame_global.loc + layout.title.loc, layout.title.size),
            ));
        }
    }

    // Contenu de la fenêtre: hors écran (coins arrondis, animations), sinon
    // les surfaces du client directement
    let inner_radius = (radius - layout.border as f32).max(0.0);
    let mut content = None;
    let mut live = None;
    if use_offscreen {
        match render_offscreen(renderer, view, scale) {
            Ok(()) => {
                let off = view.offscreen.as_ref().unwrap();
                content = Some(Content {
                    texture: off.texture.clone(),
                    id: off.id.clone(),
                    rect: Rectangle::new(view.loc, geo.size),
                    radius: inner_radius,
                });
            }
            Err(e) => {
                if geo.size.w > 0 && geo.size.h > 0 {
                    warn!("rendu hors écran: {e}");
                }
                view.offscreen = None;
            }
        }
    } else {
        view.offscreen = None;
    }
    if content.is_none() && !shaded {
        let loc = (client_loc - geo.loc).to_physical_precise_round(scale);
        live = Some(render_elements_from_surface_tree(
            renderer,
            &surface,
            loc,
            scale,
            alpha,
            Kind::Unspecified,
        ));
    }
    let snap = Snapshot {
        frame: frame_global,
        deco,
        frame_params,
        content,
        shadow,
    };
    elements.extend(snapshot_elements(
        &snap, renderer, progs, scale, output_loc, alpha, live,
    ));
    view.last = Some(snap);
    elements
}

impl State {
    /// Une animation est en cours: l'écran doit être redessiné à chaque image
    pub fn animating(&self) -> bool {
        let now = self.now();
        self.ghosts.iter().any(|g| !g.anim.done(now))
            || self
                .views
                .iter()
                .any(|v| v.appear.is_some() || v.geom_anim.is_some() || v.shade_anim.is_some())
            || self.desk_anim.is_some()
            || self.menu_ghosts.iter().any(|g| !g.anim.done(now))
            || self
                .menu
                .as_ref()
                .is_some_and(|m| m.levels.iter().any(|l| l.anim.is_some()))
            || self
                .osd
                .as_ref()
                .is_some_and(|o| o.anim.is_some() || o.hide_at.is_some())
            || self.osd_ghost.is_some()
            || self
                .snap_shown
                .is_some_and(|(_, a)| a.is_some_and(|a| !a.done(now)))
    }

    /// Fin d'une vue affichée (fermeture, réduction): sa dernière image
    /// est animée
    pub fn ghost_view(&mut self, i: usize, kind: GhostKind) {
        let anim_kind = match kind {
            GhostKind::Close => crate::anim::Kind::WindowClose,
            GhostKind::Minimize(_) => crate::anim::Kind::Minimize,
        };
        let now = self.now();
        let visible = self.visible(&self.views[i]);
        let Some(snapshot) = self.views[i].last.take() else {
            return;
        };
        if !visible {
            return;
        }
        let Some(mut anim) = self.config.animations.start(anim_kind, now) else {
            return;
        };
        // Interrompt une apparition: part de l'état affiché
        if let Some((a, _)) = self.views[i].appear.take() {
            anim = anim.resumed_from(1.0 - a.value(now).clamp(0.0, 1.0));
        }
        let window = matches!(kind, GhostKind::Minimize(_)).then(|| self.views[i].window.clone());
        self.ghosts.push(Ghost {
            snapshot,
            anim,
            kind,
            window,
        });
    }

    /// Point vers lequel une vue se réduit: le bas de son écran
    pub fn minimize_point(&self, i: usize) -> Point<i32, Logical> {
        let frame = self.frame_rect(i);
        let center = frame.loc.to_f64() + frame.size.to_f64().to_point().downscale(2.0);
        let screen = self
            .output_at(center)
            .map(|o| self.output_geometry(&o))
            .unwrap_or(frame);
        Point::from((center.x.round() as i32, screen.loc.y + screen.size.h))
    }

    /// Une vue apparaît (`Open`) ou est restaurée (`Restore`)
    pub fn appear(&mut self, i: usize, kind: Appear) {
        let now = self.now();
        let anim_kind = match kind {
            Appear::Open => crate::anim::Kind::WindowOpen,
            Appear::Restore(_) => crate::anim::Kind::Minimize,
        };
        let Some(mut anim) = self.config.animations.start(anim_kind, now) else {
            return;
        };
        // Restauration pendant la réduction: repart de l'état affiché
        let window = self.views[i].window.clone();
        if let Some(pos) = self
            .ghosts
            .iter()
            .position(|g| g.window.as_ref() == Some(&window))
        {
            let g = self.ghosts.remove(pos);
            anim = anim.resumed_from(1.0 - g.anim.value(now).clamp(0.0, 1.0));
        }
        self.views[i].appear = Some((anim, kind));
    }
}

fn build_deco(
    text: &mut crate::deco::Text,
    config: &crate::config::Config,
    theme: &crate::theme::Theme,
    key: DecoKey,
) -> DecoCache {
    let scale = f64::from_bits(key.scale);
    let colors = if key.active {
        &theme.active
    } else {
        &theme.inactive
    };
    let phys = |s: Size<i32, Logical>| {
        let p = s.to_f64().to_physical(scale);
        (p.w.round().max(0.0) as usize, p.h.round().max(0.0) as usize)
    };
    let title = (config.show_title && !key.title.is_empty()).then(|| {
        let (w, h) = phys(key.layout.title.size);
        let font = if key.active {
            &config.font_active
        } else {
            &config.font_inactive
        };
        mem_buffer(text.title(&key.title, font, colors.label_text, theme.justify, w, h, scale))
    });
    let buttons = key
        .layout
        .buttons
        .iter()
        .map(|(b, rect)| {
            let state = if key.pressed == Some(*b) {
                ButtonState::Pressed
            } else if key.hovered == Some(*b) {
                ButtonState::Hover
            } else {
                ButtonState::Normal
            };
            let toggled = *b == Button::Maximize && key.maximized;
            let (w, h) = phys(rect.size);
            (
                *rect,
                mem_buffer(button_image(theme, key.active, *b, toggled, state, w, h, scale)),
            )
        })
        .collect();
    DecoCache { key, title, buttons }
}

/// Rend l'arbre de surfaces de la fenêtre dans sa texture, si son contenu a
/// changé
fn render_offscreen(
    renderer: &mut GlesRenderer,
    view: &mut crate::view::View,
    scale: f64,
) -> anyhow::Result<()> {
    let geo = view.window.geometry();
    let size = geo.size.to_f64().to_physical(scale).to_i32_round::<i32>();
    if size.w <= 0 || size.h <= 0 {
        anyhow::bail!("taille nulle");
    }
    if view
        .offscreen
        .as_ref()
        .is_none_or(|o| o.size != size || o.scale != scale)
    {
        let texture: GlesTexture = renderer.create_buffer(
            Fourcc::Abgr8888,
            size.to_logical(1).to_buffer(1, Transform::Normal),
        )?;
        view.offscreen = Some(WindowTexture {
            texture,
            damage: OutputDamageTracker::new(size, scale, Transform::Normal),
            size,
            scale,
            id: Id::new(),
            rendered: false,
        });
    }
    let surface = view.surface().clone();
    let off = view.offscreen.as_mut().unwrap();
    let loc: Point<i32, Physical> =
        Point::<i32, Logical>::from((-geo.loc.x, -geo.loc.y)).to_physical_precise_round(scale);
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> =
        render_elements_from_surface_tree(renderer, &surface, loc, scale, 1.0, Kind::Unspecified);
    let mut target: GlesTarget = renderer.bind(&mut off.texture)?;
    let age = if off.rendered { 1 } else { 0 };
    let result = off
        .damage
        .render_output(renderer, &mut target, age, &elements, [0.0, 0.0, 0.0, 0.0])?;
    let changed = result.damage.is_some_and(|d| !d.is_empty());
    off.rendered = true;
    if changed {
        off.id = Id::new();
    }
    Ok(())
}

/// Couleur de fond, sous tout le reste
pub const CLEAR: Color = Color::rgba(0.1, 0.1, 0.12, 1.0);
