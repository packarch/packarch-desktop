// SPDX-License-Identifier: MIT OR Apache-2.0
//! Capture d'écran (cahier des charges § 9.2): wlr-screencopy v3, pour grim,
//! hyprpicker, wf-recorder et xdg-desktop-portal-wlr. Tampons partagés (shm,
//! relus par le processeur) ou dmabuf (copiés par le GPU). La copie est
//! faite au rendu suivant de l'écran demandé.
//!
//! Chaque client a, par écran et selon qu'il veut le curseur, une session :
//! texture et suivi des dégâts gardés d'une copie à l'autre. Seules les
//! zones changées sont redessinées, et une copie « avec dégâts »
//! (enregistrement, partage d'écran) attend que l'image ait changé, avec
//! les rectangles changés : sans cela un enregistreur recevait une image
//! entière à chaque requête, même écran immobile.

use std::sync::Mutex;

use smithay::backend::allocator::{Buffer, Fourcc};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, Blit, ExportMem, Offscreen, TextureFilter};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm};
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Physical, Point, Rectangle, Size, Transform};
use tracing::warn;
use wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1};
use wayland_protocols_wlr::screencopy::v1::server::zwlr_screencopy_manager_v1::{
    self, ZwlrScreencopyManagerV1,
};

use crate::render::{CLEAR, Elem};
use crate::state::State;

const VERSION: u32 = 3;

/// Données d'une image demandée
pub struct FrameData {
    manager: ZwlrScreencopyManagerV1,
    output: Output,
    /// Zone de l'écran, pixels physiques
    region: Rectangle<i32, Physical>,
    cursor: bool,
    used: Mutex<bool>,
}

struct Request {
    frame: ZwlrScreencopyFrameV1,
    manager: ZwlrScreencopyManagerV1,
    output: Output,
    region: Rectangle<i32, Physical>,
    cursor: bool,
    buffer: WlBuffer,
    damage: bool,
}

/// Image d'un écran gardée pour un client : redessinée seulement là où
/// elle a changé depuis sa dernière copie
struct Session {
    manager: ZwlrScreencopyManagerV1,
    output: Output,
    cursor: bool,
    size: Size<i32, Physical>,
    scale: f64,
    texture: GlesTexture,
    tracker: OutputDamageTracker,
    /// Rien encore dessiné : la première image est entière
    fresh: bool,
}

pub struct Screencopy {
    pending: Vec<Request>,
    sessions: Vec<Session>,
}

impl Screencopy {
    pub fn new(dh: &DisplayHandle) -> Screencopy {
        dh.create_global::<State, ZwlrScreencopyManagerV1, _>(VERSION, ());
        Screencopy {
            pending: vec![],
            sessions: vec![],
        }
    }
}

/// Formats des tampons acceptés : shm et dmabuf
const SHM_FORMATS: [wl_shm::Format; 2] = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888];
const DMABUF_FORMATS: [Fourcc; 2] = [Fourcc::Xrgb8888, Fourcc::Argb8888];

/// Taille de l'écran en pixels physiques
fn output_size(output: &Output) -> Size<i32, Physical> {
    output.current_mode().map(|m| m.size).unwrap_or_default()
}

fn init_frame(
    data_init: &mut DataInit<'_, State>,
    manager: &ZwlrScreencopyManagerV1,
    frame: New<ZwlrScreencopyFrameV1>,
    output: Option<Output>,
    region: Option<Rectangle<i32, smithay::utils::Logical>>,
    cursor: bool,
) {
    let Some(output) = output else {
        let f = data_init.init(
            frame,
            FrameData {
                manager: manager.clone(),
                cursor,
                output: Output::new(
                    "absent".into(),
                    smithay::output::PhysicalProperties {
                        size: (0, 0).into(),
                        subpixel: smithay::output::Subpixel::Unknown,
                        make: String::new(),
                        model: String::new(),
                    },
                ),
                region: Rectangle::default(),
                used: Mutex::new(true),
            },
        );
        f.failed();
        return;
    };
    let full = Rectangle::from_size(output_size(&output));
    let scale = output.current_scale().fractional_scale();
    let region = match region {
        Some(r) => r.to_f64().to_physical(scale).to_i32_round().intersection(full),
        None => Some(full),
    };
    let ok = region.is_some_and(|r| !r.is_empty());
    let region = region.unwrap_or_default();
    let f = data_init.init(
        frame,
        FrameData {
            manager: manager.clone(),
            output,
            region,
            cursor,
            used: Mutex::new(!ok),
        },
    );
    if !ok {
        f.failed();
        return;
    }
    f.buffer(
        wl_shm::Format::Xrgb8888,
        region.size.w as u32,
        region.size.h as u32,
        region.size.w as u32 * 4,
    );
    if f.version() >= 3 {
        f.linux_dmabuf(
            Fourcc::Xrgb8888 as u32,
            region.size.w as u32,
            region.size.h as u32,
        );
        f.buffer_done();
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn bind(
        _state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _data: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_screencopy_manager_v1::Request as R;
        // Pas de capture de l'écran verrouillé
        let allowed = !state.is_locked();
        match request {
            R::CaptureOutput {
                frame,
                output,
                overlay_cursor,
            } => {
                let o = Output::from_resource(&output).filter(|_| allowed);
                init_frame(data_init, resource, frame, o, None, overlay_cursor != 0);
            }
            R::CaptureOutputRegion {
                frame,
                output,
                x,
                y,
                width,
                height,
                overlay_cursor,
            } => {
                let o = Output::from_resource(&output).filter(|_| allowed);
                let r = Rectangle::new(Point::from((x, y)), Size::from((width, height)));
                init_frame(data_init, resource, frame, o, Some(r), overlay_cursor != 0);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_screencopy_frame_v1::Request as R;
        let (buffer, damage) = match request {
            R::Copy { buffer } => (buffer, false),
            R::CopyWithDamage { buffer } => (buffer, true),
            _ => return,
        };
        {
            let mut used = data.used.lock().unwrap();
            if *used {
                resource.post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "image déjà copiée");
                return;
            }
            *used = true;
        }
        let size = data.region.size;
        let valid = match smithay::wayland::dmabuf::get_dmabuf(&buffer) {
            Ok(d) => {
                let s = d.size();
                (s.w, s.h) == (size.w, size.h) && DMABUF_FORMATS.contains(&d.format().code)
            }
            Err(_) => smithay::wayland::shm::with_buffer_contents(&buffer, |_, _, b| {
                b.width == size.w
                    && b.height == size.h
                    && b.stride >= size.w * 4
                    && SHM_FORMATS.contains(&b.format)
            })
            .unwrap_or(false),
        };
        if !valid {
            resource.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "tampon invalide");
            return;
        }
        state.protocols.screencopy.pending.push(Request {
            frame: resource.clone(),
            manager: data.manager.clone(),
            output: data.output.clone(),
            region: data.region,
            cursor: data.cursor,
            buffer,
            damage,
        });
    }
}

/// Instant présent, horloge monotone: (secondes, nanosecondes)
fn monotonic() -> (u64, u32) {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // Sûr: simple appel système sur une structure locale
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    (ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// Copies demandées pour cet écran : met à jour l'image de chaque session
/// et l'écrit dans le tampon du client. `elements` commence par les
/// `cursor` éléments du curseur.
pub fn process(
    state: &mut State,
    renderer: &mut GlesRenderer,
    output: &Output,
    elements: &[Elem],
    cursor: usize,
) {
    let outputs = &state.outputs;
    let sc = &mut state.protocols.screencopy;
    sc.sessions
        .retain(|s| s.manager.is_alive() && outputs.contains(&s.output));
    let (mine, rest): (Vec<Request>, Vec<Request>) = std::mem::take(&mut sc.pending)
        .into_iter()
        .filter(|r| r.frame.is_alive())
        .partition(|r| r.output == *output);
    sc.pending = rest;
    let size = output_size(output);
    let scale = output.current_scale().fractional_scale();
    for r in mine {
        let i = match sc
            .sessions
            .iter()
            .position(|s| s.manager == r.manager && s.output == *output && s.cursor == r.cursor)
        {
            Some(i) if sc.sessions[i].size == size && sc.sessions[i].scale == scale => i,
            found => {
                let texture = match renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into()) {
                    Ok(t) => t,
                    Err(e) => {
                        warn!("capture d'écran: {e}");
                        r.frame.failed();
                        continue;
                    }
                };
                let session = Session {
                    manager: r.manager.clone(),
                    output: output.clone(),
                    cursor: r.cursor,
                    size,
                    scale,
                    texture,
                    tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
                    fresh: true,
                };
                match found {
                    Some(i) => {
                        sc.sessions[i] = session;
                        i
                    }
                    None => {
                        sc.sessions.push(session);
                        sc.sessions.len() - 1
                    }
                }
            }
        };
        let session = &mut sc.sessions[i];
        let elements = if r.cursor {
            elements
        } else {
            &elements[cursor.min(elements.len())..]
        };
        match copy(renderer, session, elements, &r) {
            Ok(Some(damage)) => {
                r.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
                if r.damage {
                    for d in damage {
                        r.frame
                            .damage(d.loc.x as u32, d.loc.y as u32, d.size.w as u32, d.size.h as u32);
                    }
                }
                let (sec, nsec) = monotonic();
                r.frame.ready((sec >> 32) as u32, sec as u32, nsec);
            }
            // Rien de changé : la copie avec dégâts attend une autre image
            Ok(None) => sc.pending.push(r),
            Err(e) => {
                warn!("capture d'écran: {e}");
                r.frame.failed();
            }
        }
    }
}

/// Met à jour l'image de la session puis la copie dans le tampon du client.
/// Rend les zones changées dans la région (coordonnées de la région), ou
/// `None` pour une copie avec dégâts quand rien n'a changé.
fn copy(
    renderer: &mut GlesRenderer,
    session: &mut Session,
    elements: &[Elem],
    r: &Request,
) -> anyhow::Result<Option<Vec<Rectangle<i32, Physical>>>> {
    let region = r.region;
    let full = Rectangle::from_size(session.size);
    let mut fb = renderer.bind(&mut session.texture)?;
    let age = if session.fresh { 0 } else { 1 };
    let result = session
        .tracker
        .render_output(renderer, &mut fb, age, elements, CLEAR.premultiplied())
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let changed = if session.fresh {
        vec![full]
    } else {
        result.damage.cloned().unwrap_or_default()
    };
    session.fresh = false;
    let damage: Vec<Rectangle<i32, Physical>> = changed
        .iter()
        .filter_map(|d| d.intersection(region))
        .map(|d| Rectangle::new(d.loc - region.loc, d.size))
        .collect();
    if r.damage && damage.is_empty() {
        return Ok(None);
    }
    if let Ok(dmabuf) = smithay::wayland::dmabuf::get_dmabuf(&r.buffer) {
        // Copie par le GPU, terminée avant que le client ne lise le tampon
        let mut dmabuf = dmabuf.clone();
        let mut dst = renderer.bind(&mut dmabuf)?;
        renderer.blit(
            &fb,
            &mut dst,
            region,
            Rectangle::from_size(region.size),
            TextureFilter::Nearest,
        )?;
        renderer.with_context(|gl| unsafe { gl.Finish() })?;
        return Ok(Some(damage));
    }
    let area = Rectangle::new(
        (region.loc.x, region.loc.y).into(),
        (region.size.w, region.size.h).into(),
    );
    let mapping = renderer.copy_framebuffer(&fb, area, Fourcc::Abgr8888)?;
    let pixels = renderer.map_texture(&mapping)?;
    let written = smithay::wayland::shm::with_buffer_contents_mut(&r.buffer, |ptr, len, b| {
        let w = region.size.w as usize;
        let need = b.offset as usize + b.stride as usize * (region.size.h as usize - 1) + w * 4;
        if need > len || pixels.len() < w * 4 * region.size.h as usize {
            return false;
        }
        for y in 0..region.size.h as usize {
            let src = &pixels[y * w * 4..(y + 1) * w * 4];
            // Sûr: la taille du tampon a été vérifiée ci-dessus
            let dst = unsafe {
                std::slice::from_raw_parts_mut(ptr.add(b.offset as usize + y * b.stride as usize), w * 4)
            };
            // RVBA → BVRX (xrgb8888 petit-boutiste)
            for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
                *d = [s[2], s[1], s[0], 255];
            }
        }
        true
    });
    match written {
        Ok(true) => Ok(Some(damage)),
        _ => anyhow::bail!("tampon shm trop petit"),
    }
}
