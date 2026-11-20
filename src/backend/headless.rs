// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backend headless: rendu OpenGL ES dans une texture, sans écran, à 60
//! images par seconde; captures PNG pour les tests (§ 13)

use std::path::{Path, PathBuf};
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::egl::native::EGLSurfacelessDisplay;
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::utils::{Physical, Rectangle, Size, Transform};
use tracing::{info, warn};

use super::{Backend, after_render, init_renderer, presented_now};
use crate::render::{CLEAR, output_elements};
use crate::state::State;

pub struct Headless {
    pub renderer: GlesRenderer,
    screens: Vec<Screen>,
    /// Captures demandées (premier écran), faites après le prochain rendu
    pub captures: Vec<PathBuf>,
}

/// Un écran sans affichage : sa texture et son suivi des dégâts
struct Screen {
    output: Output,
    damage: OutputDamageTracker,
    target: GlesTexture,
    size: Size<i32, Physical>,
    /// Table gamma reçue d'un client (retenue, sans effet sur l'image)
    gamma: Option<Vec<u16>>,
}

/// Taille d'une rampe gamma des écrans headless
const GAMMA_SIZE: u32 = 256;

impl Headless {
    pub fn gamma_size(&self, output: &Output) -> Option<u32> {
        self.screens
            .iter()
            .any(|s| s.output == *output)
            .then_some(GAMMA_SIZE)
    }

    pub fn set_gamma(&mut self, output: &Output, ramps: Option<&[u16]>) -> bool {
        let Some(s) = self.screens.iter_mut().find(|s| s.output == *output) else {
            return false;
        };
        if ramps.is_some_and(|r| r.len() != GAMMA_SIZE as usize * 3) {
            return false;
        }
        s.gamma = ramps.map(<[u16]>::to_vec);
        true
    }

    /// Table gamma en cours d'un écran, par son nom (scénarios)
    pub fn gamma(&self, name: &str) -> Option<&[u16]> {
        let s = self.screens.iter().find(|s| s.output.name() == name)?;
        s.gamma.as_deref()
    }
}

/// `count` écrans de `size`, posés côte à côte de gauche à droite
/// (HEADLESS-1, HEADLESS-2…)
pub fn init(
    event_loop: &mut EventLoop<'static, State>,
    state: &mut State,
    size: (i32, i32),
    scale: f64,
    count: usize,
) -> anyhow::Result<()> {
    // Sûr: l'affichage EGL vit aussi longtemps que le moteur de rendu
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay)? };
    let context = EGLContext::new(&display)?;
    let mut renderer = unsafe { GlesRenderer::new(context)? };
    let size: Size<i32, Physical> = size.into();
    let mode = Mode {
        size,
        refresh: 60_000,
    };
    let logical_w = (size.w as f64 / scale).round() as i32;
    let mut screens = vec![];
    for n in 0..count.max(1) {
        let target: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?;
        let output = Output::new(
            format!("HEADLESS-{}", n + 1),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Packarch".into(),
                model: "headless".into(),
            },
        );
        let _global = output.create_global::<State>(&state.dh);
        output.change_current_state(
            Some(mode),
            Some(Transform::Normal),
            Some(Scale::Fractional(scale)),
            Some((logical_w * n as i32, 0).into()),
        );
        output.set_preferred(mode);
        state.outputs.push(output.clone());
        let damage = OutputDamageTracker::from_output(&output);
        screens.push(Screen {
            output,
            damage,
            target,
            size,
            gamma: None,
        });
    }
    init_renderer(state, &mut renderer);
    info!(
        "headless: {} × {}×{}, échelle {scale}",
        screens.len(),
        size.w,
        size.h
    );
    state.backend = Some(Backend::Headless(Headless {
        renderer,
        screens,
        captures: vec![],
    }));

    event_loop
        .handle()
        .insert_source(Timer::immediate(), |_, _, state| {
            let count = match &state.backend {
                Some(Backend::Headless(h)) => h.screens.len(),
                _ => 0,
            };
            for i in 0..count {
                render(state, i);
            }
            TimeoutAction::ToDuration(Duration::from_micros(16_667))
        })
        .map_err(|e| anyhow::anyhow!("minuterie: {e}"))?;
    Ok(())
}

fn render(state: &mut State, i: usize) {
    let Some(Backend::Headless(mut h)) = state.backend.take() else {
        return;
    };
    let output = h.screens[i].output.clone();
    let mut elements = state.cursor_elements(&mut h.renderer, &output);
    let cursor = elements.len();
    elements.extend(output_elements(state, &mut h.renderer, &output));
    crate::protocols::capture(state, &mut h.renderer, &output, &elements, cursor);
    let Headless {
        renderer,
        screens,
        captures,
    } = &mut h;
    let Screen {
        damage, target, size, ..
    } = &mut screens[i];
    let captures = if i == 0 { std::mem::take(captures) } else { vec![] };
    // États du rendu : écran principal de chaque surface, comme en udev
    let mut states = None;
    match renderer.bind(target) {
        Ok(mut fb) => {
            // Une capture est rendue en entier : avec un rendu partiel, ses
            // pixels dépendraient des dégâts des images précédentes (donc du
            // rythme des clients), à un niveau près sur les bords lissés
            let age = if captures.is_empty() { 1 } else { 0 };
            match damage.render_output(renderer, &mut fb, age, &elements, CLEAR.premultiplied()) {
                Ok(r) => states = Some(r.states),
                Err(e) => warn!("rendu: {e:?}"),
            }
            for path in captures {
                let region = Rectangle::from_size((size.w, size.h).into());
                let pixels = renderer
                    .copy_framebuffer(&fb, region, Fourcc::Abgr8888)
                    .and_then(|m| renderer.map_texture(&m).map(|p| p.to_vec()));
                match pixels {
                    Ok(p) => match write_png(&path, size.w as u32, size.h as u32, &p) {
                        Ok(()) => info!("capture: {}", path.display()),
                        Err(e) => warn!("capture {}: {e}", path.display()),
                    },
                    Err(e) => warn!("capture: {e:?}"),
                }
            }
        }
        Err(e) => warn!("headless: {e:?}"),
    }
    state.backend = Some(Backend::Headless(h));
    let feedback = after_render(state, &output, states.as_ref());
    presented_now(state, &output, feedback);
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header()?;
    writer.write_image_data(rgba)?;
    Ok(())
}
