// SPDX-License-Identifier: MIT OR Apache-2.0
//! Capture d'images (cahier des charges § 9.2) : ext-image-capture-source
//! (sources : un écran, ou une fenêtre de ext-foreign-toplevel-list) et
//! ext-image-copy-capture (sessions et images). Pour grim 1.5 (`-T` : une
//! fenêtre) et xdg-desktop-portal-wlr 0.8 (partage d'une fenêtre ou d'un
//! écran). Réservé aux clients de confiance.
//!
//! Une session garde son image d'une copie à l'autre (`screencopy::Image`) :
//! une image n'est rendue au client que lorsqu'elle a changé (la première
//! tout de suite), avec les zones changées. Les sessions de curseur ne sont
//! pas prises en charge : leur session de capture est aussitôt arrêtée.

use std::sync::Mutex;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::desktop::PopupManager;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_output};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Logical, Physical, Point, Rectangle, Size};
use smithay::wayland::foreign_toplevel_list::ForeignToplevelHandle;
use tracing::warn;
use wayland_protocols::ext::image_capture_source::v1::server::{
    ext_foreign_toplevel_image_capture_source_manager_v1::{
        self as toplevel_sources, ExtForeignToplevelImageCaptureSourceManagerV1,
    },
    ext_image_capture_source_v1::{self, ExtImageCaptureSourceV1},
    ext_output_image_capture_source_manager_v1::{
        self as output_sources, ExtOutputImageCaptureSourceManagerV1,
    },
};
use wayland_protocols::ext::image_copy_capture::v1::server::{
    ext_image_copy_capture_cursor_session_v1::{self, ExtImageCopyCaptureCursorSessionV1},
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1, FailureReason},
    ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

use super::screencopy::{DMABUF_FORMATS, Image, SHM_FORMATS, buffer_fits, monotonic};
use crate::render::Elem;
use crate::state::State;

/// Ce qu'une source donne à voir
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Output(Output),
    /// Vue (identifiant)
    Toplevel(u64),
    /// Écran ou fenêtre déjà disparus
    Gone,
}

/// Formats dmabuf des captures : nœud de rendu, puis modificateurs de
/// chaque format que le moteur de rendu sait dessiner
pub type CaptureDmabuf = (u64, Vec<(Fourcc, Vec<u64>)>);

/// Session de capture
struct Session {
    id: u64,
    resource: ExtImageCopyCaptureSessionV1,
    source: Source,
    cursor: bool,
    /// Taille annoncée au client
    size: Size<i32, Physical>,
    image: Option<Image>,
    /// Image demandée (`capture`), en attente d'un changement
    pending: Option<ExtImageCopyCaptureFrameV1>,
    /// Image créée et pas encore rendue ou échouée
    current: Option<ExtImageCopyCaptureFrameV1>,
    stopped: bool,
}

#[derive(Default)]
pub struct ImageCapture {
    sessions: Vec<Session>,
    next: u64,
}

/// Données d'une image : sa session, le tampon et les zones que le client
/// dit périmées
pub struct FrameData {
    session: u64,
    inner: Mutex<FrameInner>,
}

#[derive(Default)]
struct FrameInner {
    buffer: Option<WlBuffer>,
    damage: Vec<Rectangle<i32, Physical>>,
    captured: bool,
}

/// Identifiant des sessions de curseur (toujours arrêtées)
const NO_SESSION: u64 = u64::MAX;

impl ImageCapture {
    pub fn new(dh: &DisplayHandle) -> ImageCapture {
        dh.create_global::<State, ExtOutputImageCaptureSourceManagerV1, _>(1, ());
        dh.create_global::<State, ExtForeignToplevelImageCaptureSourceManagerV1, _>(1, ());
        dh.create_global::<State, ExtImageCopyCaptureManagerV1, _>(1, ());
        ImageCapture::default()
    }
}

impl State {
    /// Vue d'une poignée ext-foreign-toplevel
    fn toplevel_view(&self, handle: &ForeignToplevelHandle) -> Option<u64> {
        self.protocols.wlr_toplevels.view_of(handle)
    }

    /// Taille d'une source en pixels physiques, et son échelle
    fn source_size(&self, source: &Source) -> Option<(Size<i32, Physical>, f64)> {
        match source {
            Source::Output(o) => {
                if !self.outputs.contains(o) {
                    return None;
                }
                let size = o.current_mode()?.size;
                Some((size, o.current_scale().fractional_scale()))
            }
            Source::Toplevel(id) => {
                let i = self.views.iter().position(|v| v.id == *id && v.mapped)?;
                let scale = self.view_scale(i);
                let size = self.views[i]
                    .window
                    .geometry()
                    .size
                    .to_f64()
                    .to_physical(scale)
                    .to_i32_round::<i32>();
                (size.w > 0 && size.h > 0).then_some((size, scale))
            }
            Source::Gone => None,
        }
    }

    /// Échelle de l'écran où se trouve une vue
    fn view_scale(&self, i: usize) -> f64 {
        let frame = self.frame_rect(i);
        let center = frame.loc.to_f64() + frame.size.to_f64().to_point().downscale(2.0);
        self.output_at(center)
            .map(|o| o.current_scale().fractional_scale())
            .unwrap_or(1.0)
    }
}

/// Annonce au client les tampons possibles pour cette taille
fn send_constraints(state: &State, session: &ExtImageCopyCaptureSessionV1, size: Size<i32, Physical>) {
    session.buffer_size(size.w as u32, size.h as u32);
    for f in SHM_FORMATS {
        session.shm_format(f);
    }
    if let Some((dev, formats)) = &state.capture_dmabuf {
        session.dmabuf_device(dev.to_ne_bytes().to_vec());
        for (code, modifiers) in formats {
            if DMABUF_FORMATS.contains(code) {
                let bytes = modifiers.iter().flat_map(|m| m.to_ne_bytes()).collect();
                session.dmabuf_format(*code as u32, bytes);
            }
        }
    }
    session.done();
}

// --- Sources --------------------------------------------------------------

impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for State {
    fn bind(
        _: &mut State,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for State {
    fn request(
        _: &mut State,
        _: &Client,
        _: &ExtOutputImageCaptureSourceManagerV1,
        request: output_sources::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        if let output_sources::Request::CreateSource { source, output } = request {
            let s = Output::from_resource(&output).map_or(Source::Gone, Source::Output);
            data_init.init(source, s);
        }
    }
}

impl GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for State {
    fn bind(
        _: &mut State,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        _: &ExtForeignToplevelImageCaptureSourceManagerV1,
        request: toplevel_sources::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        if let toplevel_sources::Request::CreateSource {
            source,
            toplevel_handle,
        } = request
        {
            let s = ForeignToplevelHandle::from_resource(&toplevel_handle)
                .and_then(|h| state.toplevel_view(&h))
                .map_or(Source::Gone, Source::Toplevel);
            data_init.init(source, s);
        }
    }
}

impl Dispatch<ExtImageCaptureSourceV1, Source> for State {
    fn request(
        _: &mut State,
        _: &Client,
        _: &ExtImageCaptureSourceV1,
        _: ext_image_capture_source_v1::Request,
        _: &Source,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
    }
}

// --- Sessions -------------------------------------------------------------

impl GlobalDispatch<ExtImageCopyCaptureManagerV1, ()> for State {
    fn bind(
        _: &mut State,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
        _: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _: &Client,
        resource: &ExtImageCopyCaptureManagerV1,
        request: ext_image_copy_capture_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use ext_image_copy_capture_manager_v1::{Options, Request};
        match request {
            Request::CreateSession {
                session,
                source,
                options,
            } => {
                let options = match options.into_result() {
                    Ok(o) => o,
                    Err(_) => {
                        resource.post_error(
                            ext_image_copy_capture_manager_v1::Error::InvalidOption,
                            "options inconnues",
                        );
                        return;
                    }
                };
                let source = source.data::<Source>().cloned().unwrap_or(Source::Gone);
                let ic = &mut state.protocols.image_capture;
                let id = ic.next;
                ic.next += 1;
                let session = data_init.init(session, id);
                // Pas de capture de l'écran verrouillé
                let size = state
                    .source_size(&source)
                    .filter(|_| !state.is_locked())
                    .map(|(s, _)| s);
                let Some(size) = size else {
                    session.stopped();
                    return;
                };
                send_constraints(state, &session, size);
                state.protocols.image_capture.sessions.push(Session {
                    id,
                    resource: session,
                    source,
                    cursor: options.contains(Options::PaintCursors),
                    size,
                    image: None,
                    pending: None,
                    current: None,
                    stopped: false,
                });
            }
            Request::CreatePointerCursorSession { session, .. } => {
                data_init.init(session, ());
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, ()> for State {
    fn request(
        _: &mut State,
        _: &Client,
        _: &ExtImageCopyCaptureCursorSessionV1,
        request: ext_image_copy_capture_cursor_session_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        if let ext_image_copy_capture_cursor_session_v1::Request::GetCaptureSession { session } = request {
            data_init.init(session, NO_SESSION).stopped();
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, u64> for State {
    fn request(
        state: &mut State,
        _: &Client,
        resource: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        id: &u64,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        if let ext_image_copy_capture_session_v1::Request::CreateFrame { frame } = request {
            let session = state
                .protocols
                .image_capture
                .sessions
                .iter_mut()
                .find(|s| s.id == *id);
            if session
                .as_ref()
                .is_some_and(|s| s.current.as_ref().is_some_and(|f| f.is_alive()))
            {
                resource.post_error(
                    ext_image_copy_capture_session_v1::Error::DuplicateFrame,
                    "une image est déjà en cours",
                );
                return;
            }
            let f = data_init.init(
                frame,
                FrameData {
                    session: *id,
                    inner: Mutex::default(),
                },
            );
            if let Some(s) = session {
                s.current = Some(f);
            }
        }
    }

    fn destroyed(
        state: &mut State,
        _: smithay::reexports::wayland_server::backend::ClientId,
        _: &ExtImageCopyCaptureSessionV1,
        id: &u64,
    ) {
        state.protocols.image_capture.sessions.retain(|s| s.id != *id);
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, FrameData> for State {
    fn request(
        state: &mut State,
        _: &Client,
        resource: &ExtImageCopyCaptureFrameV1,
        request: ext_image_copy_capture_frame_v1::Request,
        data: &FrameData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, State>,
    ) {
        use ext_image_copy_capture_frame_v1::{Error, Request};
        let mut inner = data.inner.lock().unwrap();
        match request {
            Request::AttachBuffer { buffer } => inner.buffer = Some(buffer),
            Request::DamageBuffer { x, y, width, height } => {
                if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    resource.post_error(Error::InvalidBufferDamage, "zone invalide");
                    return;
                }
                inner
                    .damage
                    .push(Rectangle::new((x, y).into(), (width, height).into()));
            }
            Request::Capture => {
                if inner.captured {
                    resource.post_error(Error::AlreadyCaptured, "image déjà demandée");
                    return;
                }
                if inner.buffer.is_none() {
                    resource.post_error(Error::NoBuffer, "pas de tampon");
                    return;
                }
                inner.captured = true;
                let session = state
                    .protocols
                    .image_capture
                    .sessions
                    .iter_mut()
                    .find(|s| s.id == data.session && !s.stopped);
                match session {
                    Some(s) => {
                        s.pending = Some(resource.clone());
                        drop(inner);
                        state.request_redraw();
                    }
                    None => resource.failed(FailureReason::Stopped),
                }
            }
            _ => {}
        }
    }
}

// --- Copie -----------------------------------------------------------------

/// Éléments d'une vue seule, à l'origine de sa géométrie : ses menus
/// surgissants puis ses surfaces
fn toplevel_elements(
    state: &State,
    renderer: &mut GlesRenderer,
    id: u64,
    scale: f64,
) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
    let Some(view) = state.views.iter().find(|v| v.id == id) else {
        return vec![];
    };
    let geo = view.window.geometry();
    let origin: Point<i32, Logical> = (-geo.loc.x, -geo.loc.y).into();
    let surface = view.surface();
    let mut out = vec![];
    for (popup, offset) in PopupManager::popups_for_surface(surface) {
        let loc = (origin + offset - popup.geometry().loc).to_physical_precise_round(scale);
        out.extend(render_elements_from_surface_tree(
            renderer,
            popup.wl_surface(),
            loc,
            scale,
            1.0,
            Kind::Unspecified,
        ));
    }
    out.extend(render_elements_from_surface_tree(
        renderer,
        surface,
        origin.to_physical_precise_round(scale),
        scale,
        1.0,
        Kind::Unspecified,
    ));
    out
}

/// Images demandées : celles des sessions de cet écran, et celles des
/// fenêtres. `elements` (l'écran) commence par les `cursor` éléments du
/// curseur.
pub fn process(
    state: &mut State,
    renderer: &mut GlesRenderer,
    output: &Output,
    elements: &[Elem],
    cursor: usize,
) {
    let locked = state.is_locked();
    let mut sessions = std::mem::take(&mut state.protocols.image_capture.sessions);
    sessions.retain(|s| s.resource.is_alive());
    for s in sessions.iter_mut().filter(|s| !s.stopped) {
        let source = state.source_size(&s.source).filter(|_| !locked);
        let Some((size, scale)) = source else {
            // Source disparue (ou écran verrouillé)
            s.stopped = true;
            s.resource.stopped();
            if let Some(f) = s.pending.take() {
                f.failed(FailureReason::Stopped);
            }
            continue;
        };
        if size != s.size {
            s.size = size;
            s.image = None;
            send_constraints(state, &s.resource, size);
            if let Some(f) = s.pending.take() {
                f.failed(FailureReason::BufferConstraints);
            }
            continue;
        }
        let Some(frame) = s.pending.clone() else {
            continue;
        };
        if !frame.is_alive() {
            s.pending = None;
            continue;
        }
        let Some(data) = frame.data::<FrameData>() else {
            continue;
        };
        let (buffer, client_damage) = {
            let inner = data.inner.lock().unwrap();
            (inner.buffer.clone(), inner.damage.clone())
        };
        let Some(buffer) = buffer.filter(|b| buffer_fits(b, size)) else {
            s.pending = None;
            frame.failed(FailureReason::BufferConstraints);
            continue;
        };
        if s.image.as_ref().is_none_or(|i| i.scale != scale) {
            match Image::new(renderer, size, scale) {
                Ok(i) => s.image = Some(i),
                Err(e) => {
                    warn!("capture: {e}");
                    s.pending = None;
                    frame.failed(FailureReason::Unknown);
                    continue;
                }
            }
        }
        let image = s.image.as_mut().unwrap();
        let region = Rectangle::from_size(size);
        let result = match &s.source {
            Source::Output(o) if o == output => {
                let elements = if s.cursor {
                    elements
                } else {
                    &elements[cursor.min(elements.len())..]
                };
                image.capture(renderer, elements, region, &buffer, true)
            }
            Source::Toplevel(id) => {
                let elements = toplevel_elements(state, renderer, *id, scale);
                image.capture(renderer, &elements, region, &buffer, true)
            }
            _ => continue,
        };
        match result {
            Ok(Some(damage)) => {
                s.pending = None;
                s.current = None;
                frame.transform(wl_output::Transform::Normal);
                for d in damage.iter().chain(&client_damage) {
                    frame.damage(d.loc.x, d.loc.y, d.size.w, d.size.h);
                }
                let (sec, nsec) = monotonic();
                frame.presentation_time((sec >> 32) as u32, sec as u32, nsec);
                frame.ready();
            }
            // Rien de changé : l'image attend
            Ok(None) => {}
            Err(e) => {
                warn!("capture: {e}");
                s.pending = None;
                frame.failed(FailureReason::Unknown);
            }
        }
    }
    sessions.append(&mut state.protocols.image_capture.sessions);
    state.protocols.image_capture.sessions = sessions;
}
