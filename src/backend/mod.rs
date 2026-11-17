// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backends (cahier des charges § 4.3): winit (fenêtre imbriquée) et
//! headless (sans écran, pour les tests et les captures). udev: 0.4.

pub mod headless;
pub mod winit;

use std::time::Duration;

use smithay::backend::egl::EGLDevice;
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::layer_map_for_output;
use smithay::desktop::space::SpaceElement;
use smithay::output::Output;
use smithay::wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState};
use tracing::{info, warn};

use crate::render::Shaders;
use crate::state::State;

pub enum Backend {
    Winit(winit::Winit),
    Headless(headless::Headless),
}

impl Backend {
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        match self {
            Backend::Winit(w) => w.backend.renderer(),
            Backend::Headless(h) => &mut h.renderer,
        }
    }
}

/// Préparation commune: shaders, dmabuf (v4 avec retour si le nœud de rendu
/// est connu, nécessaire aux clients EGL de Mesa)
pub fn init_renderer(state: &mut State, renderer: &mut GlesRenderer) {
    match Shaders::compile(renderer) {
        Ok(s) => state.shaders = Some(s),
        Err(e) => warn!("shaders: {e}"),
    }
    let formats = renderer.dmabuf_formats();
    let node = EGLDevice::device_for_display(renderer.egl_context().display())
        .ok()
        .and_then(|d| d.try_get_render_node().ok().flatten());
    let mut dmabuf_state = DmabufState::new();
    let global = match node.map(|n| DmabufFeedbackBuilder::new(n.dev_id(), formats.clone()).build()) {
        Some(Ok(feedback)) => {
            info!("dmabuf v4");
            dmabuf_state.create_global_with_default_feedback::<State>(&state.dh, &feedback)
        }
        _ => {
            warn!("dmabuf v3 (nœud de rendu inconnu)");
            dmabuf_state.create_global::<State>(&state.dh, formats)
        }
    };
    state.dmabuf = Some((dmabuf_state, global));
}

/// Après un rendu: rappels de trame, ménage
pub fn after_render(state: &mut State, output: &Output) {
    let time = state.start.elapsed();
    let throttle = Some(Duration::from_secs(1));
    for view in &state.views {
        if view.mapped {
            view.window
                .send_frame(output, time, throttle, |_, _| Some(output.clone()));
        }
    }
    {
        let map = layer_map_for_output(output);
        for layer in map.layers() {
            layer.send_frame(output, time, throttle, |_, _| Some(output.clone()));
        }
    }
    layer_map_for_output(output).cleanup();
    for view in &state.views {
        view.window.refresh();
    }
    state.popups.cleanup();
    let _ = state.dh.flush_clients();
}
