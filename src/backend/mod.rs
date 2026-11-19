// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backends (cahier des charges § 4.3): winit (fenêtre imbriquée) et
//! headless (sans écran, pour les tests et les captures), udev (session
//! réelle).

pub mod edid;
pub mod headless;
pub mod udev;
pub mod winit;

use std::time::Duration;

use smithay::backend::egl::EGLDevice;
use smithay::backend::renderer::ImportDma;
use smithay::backend::renderer::element::{Id, RenderElementStates, default_primary_scanout_output_compare};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::space::SpaceElement;
use smithay::desktop::utils::{
    OutputPresentationFeedback, send_frames_surface_tree, surface_presentation_feedback_flags_from_states,
    surface_primary_scanout_output, take_presentation_feedback_surface_tree,
    update_surface_primary_scanout_output, with_surfaces_surface_tree,
};
use smithay::desktop::{PopupManager, layer_map_for_output};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::compositor::{SurfaceData, TraversalAction, with_surface_tree_downward};
use smithay::wayland::dmabuf::{DmabufFeedbackBuilder, DmabufState};
use smithay::wayland::presentation::Refresh;
use smithay::wayland::seat::WaylandFocus;
use tracing::{info, warn};

use crate::render::Shaders;
use crate::state::State;

pub enum Backend {
    Winit(winit::Winit),
    Headless(headless::Headless),
    Udev(Box<udev::Udev>),
}

impl Backend {
    pub fn renderer(&mut self) -> &mut GlesRenderer {
        match self {
            Backend::Winit(w) => w.backend.renderer(),
            Backend::Headless(h) => &mut h.renderer,
            Backend::Udev(u) => &mut u.renderer,
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

/// Après un rendu: écran principal de chaque surface, rappels de trame,
/// barres des tâches et bureaux, ménage. Rend les retours de présentation
/// des surfaces dont c'est l'écran principal, à signaler quand l'image est
/// vraiment affichée.
///
/// `states` (ce que le rendu a dessiné) fixe l'écran principal de chaque
/// surface : une surface ne reçoit ses rappels de trame et ses retours de
/// présentation qu'au rythme de cet écran. Sinon, avec deux écrans non
/// synchronisés, un client recevrait deux rappels par période et XWayland
/// croirait l'écran à 120 Hz (vidéo saccadée). Les surfaces invisibles
/// reçoivent un rappel par seconde. Sans `states` (winit : un seul écran),
/// toutes les surfaces sont sur cet écran. Headless passe ses états, pour
/// que les scénarios exercent ce chemin (`expect-primary`).
pub fn after_render(
    state: &mut State,
    output: &Output,
    states: Option<&RenderElementStates>,
) -> OutputPresentationFeedback {
    let time = state.start.elapsed();
    state.confirm_lock();
    let surfaces = all_surfaces(state);
    let states = states.map(|s| with_offscreen_states(state, s));
    let states = states.as_ref();
    if let Some(states) = states {
        for s in &surfaces {
            with_surfaces_surface_tree(s, |surface, data| {
                update_surface_primary_scanout_output(
                    surface,
                    output,
                    data,
                    states,
                    default_primary_scanout_output_compare,
                );
            });
        }
    }
    let primary = |surface: &WlSurface, data: &SurfaceData| match states {
        Some(_) => surface_primary_scanout_output(surface, data),
        None => Some(output.clone()),
    };
    let mut feedback = OutputPresentationFeedback::new(output);
    for s in &surfaces {
        take_presentation_feedback_surface_tree(s, &mut feedback, primary, |surface, _| match states {
            Some(states) => surface_presentation_feedback_flags_from_states(surface, states),
            None => Kind::empty(),
        });
    }
    state.refresh_toplevels();
    state.refresh_workspaces();
    state.refresh_output_tools();
    let throttle = Some(Duration::from_secs(1));
    for s in &surfaces {
        send_frames_surface_tree(s, output, time, throttle, primary);
    }
    layer_map_for_output(output).cleanup();
    for view in &state.views {
        view.window.refresh();
    }
    state.sync_x11_positions();
    state.popups.cleanup();
    let _ = state.dh.flush_clients();
    feedback
}

/// États du rendu complétés par les fenêtres dessinées hors écran : leur
/// contenu est une texture, leurs surfaces ne figurent donc pas dans les
/// états. Sans cet ajout elles passeraient pour invisibles (un rappel de
/// trame par seconde : tout répond avec retard). Chaque surface de l'arbre
/// reçoit l'état de l'élément du contenu sur cet écran, qui tient compte
/// des fenêtres au-dessus : une fenêtre entièrement recouverte reste
/// invisible.
fn with_offscreen_states(state: &State, states: &RenderElementStates) -> RenderElementStates {
    let mut out = states.clone();
    for (surface, id) in &state.offscreen_shown {
        let Some(shown) = states.states.get(id).copied() else {
            continue;
        };
        if shown.visible_area == 0 {
            continue;
        }
        with_surface_tree_downward(
            surface,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |s, _, _| {
                let entry = out.states.entry(Id::from_wayland_resource(s)).or_insert(shown);
                if entry.visible_area < shown.visible_area {
                    *entry = shown;
                }
            },
            |_, _, _| true,
        );
    }
    out
}

/// Retour de présentation immédiat (winit, headless : pas de vblank connu)
pub fn presented_now(state: &State, output: &Output, mut feedback: OutputPresentationFeedback) {
    feedback.presented(state.monotonic.now(), refresh_of(output), 0, Kind::Vsync);
}

/// Période de rafraîchissement d'un écran
pub fn refresh_of(output: &Output) -> Refresh {
    output
        .current_mode()
        .map(|m| Refresh::fixed(Duration::from_secs_f64(1000.0 / m.refresh.max(1) as f64)))
        .unwrap_or(Refresh::Unknown)
}

/// Racines de toutes les surfaces affichables : vues (avec leurs menus
/// surgissants), couches de tous les écrans, fenêtres X11 hors gestion,
/// écrans de verrouillage
fn all_surfaces(state: &State) -> Vec<WlSurface> {
    let mut out = Vec::new();
    for view in state.views.iter().filter(|v| v.mapped) {
        if let Some(s) = view.window.wl_surface() {
            let s = s.into_owned();
            for (popup, _) in PopupManager::popups_for_surface(&s) {
                out.push(popup.wl_surface().clone());
            }
            out.push(s);
        }
    }
    for o in &state.outputs {
        for layer in layer_map_for_output(o).layers() {
            out.push(layer.wl_surface().clone());
            for (popup, _) in PopupManager::popups_for_surface(layer.wl_surface()) {
                out.push(popup.wl_surface().clone());
            }
        }
    }
    out.extend(state.unmanaged_surfaces().into_iter().map(|(s, _)| s));
    out.extend(state.lock_surfaces.iter().map(|(_, l)| l.wl_surface().clone()));
    out
}
