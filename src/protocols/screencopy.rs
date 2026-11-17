// SPDX-License-Identifier: MIT OR Apache-2.0
//! Capture d'écran (cahier des charges § 9.2): wlr-screencopy v3, tampons
//! partagés (shm), pour grim, hyprpicker, wf-recorder. La copie est faite
//! au rendu suivant de l'écran demandé.

use std::sync::Mutex;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
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
    output: Output,
    /// Zone de l'écran, pixels physiques
    region: Rectangle<i32, Physical>,
    used: Mutex<bool>,
}

struct Request {
    frame: ZwlrScreencopyFrameV1,
    output: Output,
    region: Rectangle<i32, Physical>,
    buffer: WlBuffer,
    damage: bool,
}

pub struct Screencopy {
    pending: Vec<Request>,
}

impl Screencopy {
    pub fn new(dh: &DisplayHandle) -> Screencopy {
        dh.create_global::<State, ZwlrScreencopyManagerV1, _>(VERSION, ());
        Screencopy { pending: vec![] }
    }
}

/// Taille de l'écran en pixels physiques
fn output_size(output: &Output) -> Size<i32, Physical> {
    output.current_mode().map(|m| m.size).unwrap_or_default()
}

fn init_frame(
    data_init: &mut DataInit<'_, State>,
    frame: New<ZwlrScreencopyFrameV1>,
    output: Option<Output>,
    region: Option<Rectangle<i32, smithay::utils::Logical>>,
) {
    let Some(output) = output else {
        let f = data_init.init(
            frame,
            FrameData {
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
            output,
            region,
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
        _resource: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_screencopy_manager_v1::Request as R;
        // Pas de capture de l'écran verrouillé
        let allowed = !state.is_locked();
        match request {
            R::CaptureOutput { frame, output, .. } => {
                let o = Output::from_resource(&output).filter(|_| allowed);
                init_frame(data_init, frame, o, None);
            }
            R::CaptureOutputRegion {
                frame,
                output,
                x,
                y,
                width,
                height,
                ..
            } => {
                let o = Output::from_resource(&output).filter(|_| allowed);
                let r = Rectangle::new(Point::from((x, y)), Size::from((width, height)));
                init_frame(data_init, frame, o, Some(r));
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
        let valid = smithay::wayland::shm::with_buffer_contents(&buffer, |_, _, b| {
            b.width == size.w
                && b.height == size.h
                && b.stride >= size.w * 4
                && matches!(b.format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
        })
        .unwrap_or(false);
        if !valid {
            resource.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "tampon invalide");
            return;
        }
        state.protocols.screencopy.pending.push(Request {
            frame: resource.clone(),
            output: data.output.clone(),
            region: data.region,
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

/// Copie demandée pour cet écran: rend ses éléments hors écran et les
/// écrit dans le tampon du client
pub fn process(state: &mut State, renderer: &mut GlesRenderer, output: &Output, elements: &[Elem]) {
    let (mine, rest): (Vec<Request>, Vec<Request>) = std::mem::take(&mut state.protocols.screencopy.pending)
        .into_iter()
        .partition(|r| r.output == *output);
    state.protocols.screencopy.pending = rest;
    if mine.is_empty() {
        return;
    }
    let size = output_size(output);
    let scale = output.current_scale().fractional_scale();
    let pixels = (|| -> anyhow::Result<Vec<u8>> {
        let mut tex: GlesTexture = renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?;
        let mut damage = OutputDamageTracker::new(size, scale, Transform::Normal);
        let mut fb = renderer.bind(&mut tex)?;
        damage
            .render_output(renderer, &mut fb, 0, elements, CLEAR.premultiplied())
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let mapping = renderer.copy_framebuffer(
            &fb,
            Rectangle::from_size((size.w, size.h).into()),
            Fourcc::Abgr8888,
        )?;
        Ok(renderer.map_texture(&mapping)?.to_vec())
    })();
    let (sec, nsec) = monotonic();
    for r in mine {
        if !r.frame.is_alive() {
            continue;
        }
        let Ok(pixels) = &pixels else {
            if let Err(e) = &pixels {
                warn!("capture d'écran: {e}");
            }
            r.frame.failed();
            continue;
        };
        let written = smithay::wayland::shm::with_buffer_contents_mut(&r.buffer, |ptr, len, b| {
            let region = r.region;
            let need = b.offset as usize + (b.stride * (region.size.h - 1) + region.size.w * 4) as usize;
            if need > len {
                return false;
            }
            for y in 0..region.size.h {
                let src_row = ((region.loc.y + y) * size.w + region.loc.x) as usize * 4;
                let dst_row = b.offset as usize + (y * b.stride) as usize;
                for x in 0..region.size.w as usize {
                    let s = &pixels[src_row + x * 4..src_row + x * 4 + 4];
                    // RVBA → BVRX (xrgb8888 petit-boutiste)
                    // Sûr: la taille du tampon a été vérifiée ci-dessus
                    unsafe {
                        let d = ptr.add(dst_row + x * 4);
                        *d = s[2];
                        *d.add(1) = s[1];
                        *d.add(2) = s[0];
                        *d.add(3) = 255;
                    }
                }
            }
            true
        });
        if !matches!(written, Ok(true)) {
            r.frame.failed();
            continue;
        }
        r.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
        if r.damage {
            r.frame
                .damage(0, 0, r.region.size.w as u32, r.region.size.h as u32);
        }
        r.frame.ready((sec >> 32) as u32, sec as u32, nsec);
    }
}
