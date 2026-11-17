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

use super::{Backend, after_render, init_renderer};
use crate::render::{CLEAR, output_elements};
use crate::state::State;

pub struct Headless {
    pub renderer: GlesRenderer,
    output: Output,
    damage: OutputDamageTracker,
    target: GlesTexture,
    size: Size<i32, Physical>,
    /// Captures demandées, faites après le prochain rendu
    pub captures: Vec<PathBuf>,
}

pub fn init(
    event_loop: &mut EventLoop<'static, State>,
    state: &mut State,
    size: (i32, i32),
    scale: f64,
) -> anyhow::Result<()> {
    // Sûr: l'affichage EGL vit aussi longtemps que le moteur de rendu
    let display = unsafe { EGLDisplay::new(EGLSurfacelessDisplay)? };
    let context = EGLContext::new(&display)?;
    let mut renderer = unsafe { GlesRenderer::new(context)? };
    let size: Size<i32, Physical> = size.into();
    let target: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?;
    let mode = Mode {
        size,
        refresh: 60_000,
    };
    let output = Output::new(
        "HEADLESS-1".into(),
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
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.outputs.push(output.clone());
    init_renderer(state, &mut renderer);
    let damage = OutputDamageTracker::from_output(&output);
    info!("headless: {}×{}, échelle {scale}", size.w, size.h);
    state.backend = Some(Backend::Headless(Headless {
        renderer,
        output,
        damage,
        target,
        size,
        captures: vec![],
    }));

    event_loop
        .handle()
        .insert_source(Timer::immediate(), |_, _, state| {
            render(state);
            TimeoutAction::ToDuration(Duration::from_micros(16_667))
        })
        .map_err(|e| anyhow::anyhow!("minuterie: {e}"))?;
    Ok(())
}

fn render(state: &mut State) {
    let Some(Backend::Headless(mut h)) = state.backend.take() else {
        return;
    };
    let output = h.output.clone();
    let elements = output_elements(state, &mut h.renderer, &output);
    let Headless {
        renderer,
        damage,
        target,
        size,
        captures,
        ..
    } = &mut h;
    match renderer.bind(target) {
        Ok(mut fb) => {
            if let Err(e) = damage.render_output(renderer, &mut fb, 1, &elements, CLEAR.premultiplied()) {
                warn!("rendu: {e:?}");
            }
            for path in captures.drain(..) {
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
    after_render(state, &output);
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
