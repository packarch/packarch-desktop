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
use smithay::backend::renderer::element::{AsRenderElements, Id, Kind};
use smithay::backend::renderer::gles::element::{PixelShaderElement, TextureShaderElement};
use smithay::backend::renderer::gles::{
    GlesPixelProgram, GlesRenderer, GlesTarget, GlesTexProgram, GlesTexture, Uniform, UniformName,
    UniformType,
};
use smithay::backend::renderer::{Bind, Offscreen as _, Renderer};
use smithay::desktop::layer_map_for_output;
use smithay::output::Output;
use smithay::render_elements;
use smithay::utils::{Logical, Physical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::shell::wlr_layer::Layer;
use tracing::warn;

use crate::config::Button;
use crate::deco::{ButtonState, Layout, button_image};
use crate::state::State;
use crate::theme::Color;

render_elements! {
    pub Elem<=GlesRenderer>;
    Surface=WaylandSurfaceRenderElement<GlesRenderer>,
    Pixel=PixelShaderElement,
    Clipped=TextureShaderElement,
    Memory=MemoryRenderBufferRenderElement<GlesRenderer>,
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
    uniforms: Vec<Uniform<'_>>,
) -> PixelShaderElement {
    PixelShaderElement::new(program.clone(), area, None, 1.0, uniforms, Kind::Unspecified)
}

/// Éléments de rendu d'un écran, du premier plan vers l'arrière-plan
pub fn output_elements(state: &mut State, renderer: &mut GlesRenderer, output: &Output) -> Vec<Elem> {
    let scale = output.current_scale().fractional_scale();
    let output_loc = state.output_location(output);
    let mut elements: Vec<Elem> = vec![];
    let Some(shaders) = state.shaders.as_ref() else {
        return elements;
    };
    let (frame_prog, shadow_prog, clip_prog) = (
        shaders.frame.clone(),
        shaders.shadow.clone(),
        shaders.clip.clone(),
    );

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

    elements.extend(layer_elements(renderer, Layer::Overlay));
    elements.extend(layer_elements(renderer, Layer::Top));

    let focused = state.focused_root();
    let config = state.config.clone();
    let theme = state.theme.clone();
    for i in (0..state.views.len()).rev() {
        let view = &state.views[i];
        if !view.mapped || view.minimized {
            continue;
        }
        let geo = view.window.geometry();
        let active = focused.as_ref() == Some(view.surface());
        let maximized = view.maximized.is_some();
        let layout = Layout::new(&config, &theme, geo.size);
        let colors = if active { &theme.active } else { &theme.inactive };
        let frame_loc = view.loc - layout.client.loc - output_loc;
        let client_loc = view.loc - output_loc;
        let radius = if maximized { 0 } else { config.corner_radius } as f32;

        // Fenêtres surgissantes (menus des applications), devant le cadre
        let surface = view.surface().clone();
        for (popup, offset) in smithay::desktop::PopupManager::popups_for_surface(&surface) {
            let loc = (client_loc + offset - popup.geometry().loc).to_physical_precise_round(scale);
            elements.extend(render_elements_from_surface_tree(
                renderer,
                popup.wl_surface(),
                loc,
                scale,
                1.0,
                Kind::Unspecified,
            ));
        }

        // Titre et boutons
        let key = DecoKey {
            layout: layout.clone(),
            title: view.title(),
            active,
            maximized,
            hovered: view.hovered,
            pressed: view.pressed,
            scale: scale.to_bits(),
        };
        let view = &mut state.views[i];
        if view.deco.as_ref().is_none_or(|d| d.key != key) {
            view.deco = Some(build_deco(&mut state.text, &config, &theme, key));
        }
        let deco = view.deco.as_ref().unwrap();
        let mut mem = |(buf, px_size): &(MemoryRenderBuffer, Size<i32, Logical>),
                       rect: Rectangle<i32, Logical>| {
            let loc = (frame_loc + rect.loc).to_f64().to_physical(scale);
            match MemoryRenderBufferRenderElement::from_buffer(
                renderer,
                loc,
                buf,
                None,
                Some(Rectangle::from_size(px_size.to_f64())),
                Some(rect.size),
                Kind::Unspecified,
            ) {
                Ok(e) => elements.push(Elem::Memory(e)),
                Err(e) => warn!("image du cadre: {e:?}"),
            }
        };
        for (rect, buf) in &deco.buttons {
            mem(buf, *rect);
        }
        if let Some(buf) = &deco.title {
            mem(buf, layout.title);
        }

        // Cadre
        let frame_area = Rectangle::new(frame_loc, layout.size);
        elements.push(Elem::Pixel(pixel(
            &frame_prog,
            frame_area,
            vec![
                Uniform::new("border_color", colors.border.premultiplied()),
                Uniform::new("title_color", colors.title_bg.premultiplied()),
                Uniform::new("radius_top", radius),
                Uniform::new("radius_bottom", radius),
                Uniform::new("border", layout.border as f32),
                Uniform::new("title_height", layout.title_height as f32),
                Uniform::new("px", px(scale)),
            ],
        )));

        // Contenu de la fenêtre
        let inner_radius = (radius - layout.border as f32).max(0.0);
        if inner_radius > 0.0 {
            match render_offscreen(renderer, view, scale) {
                Ok(()) => {
                    let off = view.offscreen.as_ref().unwrap();
                    let tex = TextureRenderElement::from_static_texture(
                        off.id.clone(),
                        renderer.context_id(),
                        client_loc.to_f64().to_physical(scale),
                        off.texture.clone(),
                        1,
                        Transform::Normal,
                        None,
                        None,
                        Some(geo.size),
                        None,
                        Kind::Unspecified,
                    );
                    elements.push(Elem::Clipped(TextureShaderElement::new(
                        tex,
                        clip_prog.clone(),
                        vec![
                            Uniform::new("geo_size", (geo.size.w as f32, geo.size.h as f32)),
                            Uniform::new("radius", inner_radius),
                            Uniform::new("px", px(scale)),
                        ],
                    )));
                }
                Err(e) => {
                    warn!("rendu hors écran: {e}");
                    view.offscreen = None;
                }
            }
        } else {
            view.offscreen = None;
            let loc = (client_loc - geo.loc).to_physical_precise_round(scale);
            elements.extend(render_elements_from_surface_tree(
                renderer,
                &surface,
                loc,
                scale,
                1.0,
                Kind::Unspecified,
            ));
        }

        // Ombre
        let shadows = config.drop_shadows && (!maximized || config.drop_shadows_on_tiled);
        if shadows && colors.shadow_size > 0 && colors.shadow_color.a > 0.0 {
            let s = colors.shadow_size;
            let area = Rectangle::new(
                frame_loc - Point::from((s, s)),
                layout.size + Size::from((2 * s, 2 * s)),
            );
            elements.push(Elem::Pixel(pixel(
                &shadow_prog,
                area,
                vec![
                    Uniform::new("shadow_color", colors.shadow_color.premultiplied()),
                    Uniform::new(
                        "shape",
                        [s as f32, s as f32, layout.size.w as f32, layout.size.h as f32],
                    ),
                    Uniform::new("radius_top", radius),
                    Uniform::new("radius_bottom", radius),
                    Uniform::new("sigma", (s as f32 / 2.0).max(0.5)),
                    Uniform::new("px", px(scale)),
                ],
            )));
        }
    }

    elements.extend(layer_elements(renderer, Layer::Bottom));
    elements.extend(layer_elements(renderer, Layer::Background));
    elements
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
    let off = view.offscreen.as_mut().unwrap();
    let loc: Point<i32, Physical> =
        Point::<i32, Logical>::from((-geo.loc.x, -geo.loc.y)).to_physical_precise_round(scale);
    let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = render_elements_from_surface_tree(
        renderer,
        view.window.toplevel().unwrap().wl_surface(),
        loc,
        scale,
        1.0,
        Kind::Unspecified,
    );
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
