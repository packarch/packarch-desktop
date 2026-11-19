// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backend winit: le compositeur dans une fenêtre d'une session existante

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent, WinitGraphicsBackend};
use smithay::desktop::layer_map_for_output;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::EventLoop;
use smithay::utils::Transform;
use tracing::warn;

use super::{Backend, after_render, init_renderer, presented_now};
use crate::render::{CLEAR, output_elements};
use crate::state::State;

pub struct Winit {
    pub backend: WinitGraphicsBackend<GlesRenderer>,
    output: Output,
    damage: OutputDamageTracker,
}

pub fn init(event_loop: &mut EventLoop<'static, State>, state: &mut State, scale: f64) -> anyhow::Result<()> {
    let (mut backend, winit) = winit::init::<GlesRenderer>().map_err(|e| anyhow::anyhow!("winit: {e}"))?;
    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    let output = Output::new(
        "winit".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Packarch".into(),
            model: "winit".into(),
        },
    );
    let _global = output.create_global::<State>(&state.dh);
    output.change_current_state(
        Some(mode),
        Some(Transform::Flipped180),
        Some(Scale::Fractional(scale)),
        Some((0, 0).into()),
    );
    output.set_preferred(mode);
    state.outputs.push(output.clone());
    init_renderer(state, backend.renderer());
    let damage = OutputDamageTracker::from_output(&output);
    state.backend = Some(Backend::Winit(Winit {
        backend,
        output,
        damage,
    }));

    event_loop
        .handle()
        .insert_source(winit, move |event, _, state| match event {
            WinitEvent::Resized { size, .. } => {
                let Some(Backend::Winit(w)) = state.backend.as_mut() else {
                    return;
                };
                w.output.change_current_state(
                    Some(Mode {
                        size,
                        refresh: 60_000,
                    }),
                    None,
                    None,
                    None,
                );
                let output = w.output.clone();
                layer_map_for_output(&output).arrange();
            }
            WinitEvent::Input(event) => state.process_input(event),
            WinitEvent::Redraw => render(state),
            WinitEvent::CloseRequested => state.signal.stop(),
            _ => {}
        })
        .map_err(|e| anyhow::anyhow!("source winit: {e}"))?;
    Ok(())
}

fn render(state: &mut State) {
    let Some(Backend::Winit(mut w)) = state.backend.take() else {
        return;
    };
    let output = w.output.clone();
    let elements = output_elements(state, w.backend.renderer(), &output);
    crate::protocols::capture(state, w.backend.renderer(), &output, &elements, 0);
    // Âge du tampon: sa lecture échoue avec certains pilotes (NVIDIA);
    // en fenêtre imbriquée, tout redessiner suffit
    let age = 0;
    let damage = match w.backend.bind() {
        Ok((renderer, mut fb)) => {
            match w
                .damage
                .render_output(renderer, &mut fb, age, &elements, CLEAR.premultiplied())
            {
                Ok(r) => r.damage.cloned(),
                Err(e) => {
                    warn!("rendu: {e:?}");
                    None
                }
            }
        }
        Err(e) => {
            warn!("winit: {e}");
            None
        }
    };
    if let Some(damage) = damage
        && let Err(e) = w.backend.submit(Some(&damage))
    {
        warn!("winit: {e}");
    }
    w.backend.window().request_redraw();
    state.backend = Some(Backend::Winit(w));
    let feedback = after_render(state, &output, None);
    presented_now(state, &output, feedback);
}
