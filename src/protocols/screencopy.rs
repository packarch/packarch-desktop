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

use smithay::backend::allocator::dmabuf::{DmabufMappingMode, DmabufSyncFlags};
use smithay::backend::allocator::{Buffer, Fourcc, Modifier};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::RenderElement;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTarget, GlesTexture};
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

/// Image gardée d'une copie à l'autre (wlr-screencopy, ext-image-copy-capture) :
/// redessinée seulement là où elle a changé
pub struct Image {
    pub size: Size<i32, Physical>,
    pub scale: f64,
    texture: GlesTexture,
    tracker: OutputDamageTracker,
    /// Rien encore dessiné : la première image est entière
    fresh: bool,
}

impl Image {
    pub fn new(renderer: &mut GlesRenderer, size: Size<i32, Physical>, scale: f64) -> anyhow::Result<Image> {
        Ok(Image {
            size,
            scale,
            texture: renderer.create_buffer(Fourcc::Abgr8888, (size.w, size.h).into())?,
            tracker: OutputDamageTracker::new(size, scale, Transform::Normal),
            fresh: true,
        })
    }

    /// Redessine l'image puis copie `region` dans le tampon du client. Rend
    /// les zones changées dans la région (coordonnées de la région), ou
    /// `None` si `need_change` et que rien n'a changé.
    pub fn capture<E: RenderElement<GlesRenderer>>(
        &mut self,
        renderer: &mut GlesRenderer,
        elements: &[E],
        region: Rectangle<i32, Physical>,
        buffer: &WlBuffer,
        need_change: bool,
    ) -> anyhow::Result<Option<Vec<Rectangle<i32, Physical>>>> {
        let full = Rectangle::from_size(self.size);
        let mut fb = renderer.bind(&mut self.texture)?;
        let age = if self.fresh { 0 } else { 1 };
        let result = self
            .tracker
            .render_output(renderer, &mut fb, age, elements, CLEAR.premultiplied())
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let changed = if self.fresh {
            vec![full]
        } else {
            result.damage.cloned().unwrap_or_default()
        };
        self.fresh = false;
        let damage: Vec<Rectangle<i32, Physical>> = changed
            .iter()
            .filter_map(|d| d.intersection(region))
            .map(|d| Rectangle::new(d.loc - region.loc, d.size))
            .collect();
        if need_change && damage.is_empty() {
            return Ok(None);
        }
        write_buffer(renderer, &fb, region, buffer)?;
        Ok(Some(damage))
    }
}

/// Le tampon du client a la taille voulue et un format accepté
pub fn buffer_fits(buffer: &WlBuffer, size: Size<i32, Physical>) -> bool {
    match smithay::wayland::dmabuf::get_dmabuf(buffer) {
        Ok(d) => {
            let s = d.size();
            (s.w, s.h) == (size.w, size.h) && DMABUF_FORMATS.contains(&d.format().code)
        }
        Err(_) => smithay::wayland::shm::with_buffer_contents(buffer, |_, _, b| {
            b.width == size.w
                && b.height == size.h
                && b.stride >= size.w * 4
                && SHM_FORMATS.contains(&b.format)
        })
        .unwrap_or(false),
    }
}

/// Copie `region` de l'image liée `fb` dans le tampon du client : par le GPU
/// pour un dmabuf, par le processeur pour un tampon shm
fn write_buffer(
    renderer: &mut GlesRenderer,
    fb: &GlesTarget<'_>,
    region: Rectangle<i32, Physical>,
    buffer: &WlBuffer,
) -> anyhow::Result<()> {
    if let Ok(dmabuf) = smithay::wayland::dmabuf::get_dmabuf(buffer) {
        let renderable = renderer
            .egl_context()
            .dmabuf_render_formats()
            .contains(&dmabuf.format());
        if renderable {
            // Copie terminée avant que le client ne lise le tampon
            let mut dmabuf = dmabuf.clone();
            let mut dst = renderer.bind(&mut dmabuf)?;
            renderer.blit(
                fb,
                &mut dst,
                region,
                Rectangle::from_size(region.size),
                TextureFilter::Nearest,
            )?;
            renderer.with_context(|gl| unsafe { gl.Finish() })?;
            return Ok(());
        }
        // Tampon que le moteur ne sait pas dessiner : sur NVIDIA, un dmabuf
        // linéaire n'est accepté qu'en lecture (wf-recorder en alloue un).
        // Copie par le processeur, le tampon mappé en mémoire
        if dmabuf.format().modifier != Modifier::Linear || dmabuf.num_planes() != 1 {
            anyhow::bail!("dmabuf {:?} : format non dessinable", dmabuf.format());
        }
        let pixels = read_region(renderer, fb, region)?;
        let stride = dmabuf.strides().next().unwrap_or(0) as usize;
        // La projection commence à l'offset du plan
        let map = dmabuf.map_plane(0, DmabufMappingMode::WRITE)?;
        dmabuf.sync_plane(0, DmabufSyncFlags::START | DmabufSyncFlags::WRITE)?;
        // Sûr : projection valide jusqu'à sa destruction, à la fin de la fonction
        let dst = unsafe { std::slice::from_raw_parts_mut(map.ptr().cast::<u8>(), map.length()) };
        let ok = copy_pixels(&pixels, region.size, dst, 0, stride);
        dmabuf.sync_plane(0, DmabufSyncFlags::END | DmabufSyncFlags::WRITE)?;
        if !ok {
            anyhow::bail!("dmabuf trop petit");
        }
        return Ok(());
    }
    let pixels = read_region(renderer, fb, region)?;
    let written = smithay::wayland::shm::with_buffer_contents_mut(buffer, |ptr, len, b| {
        // Sûr : zone du tampon donnée par Smithay
        let dst = unsafe { std::slice::from_raw_parts_mut(ptr, len) };
        copy_pixels(&pixels, region.size, dst, b.offset as usize, b.stride as usize)
    });
    match written {
        Ok(true) => Ok(()),
        _ => anyhow::bail!("tampon shm trop petit"),
    }
}

/// Pixels de `region` dans l'image liée `fb`, en RVBA
fn read_region(
    renderer: &mut GlesRenderer,
    fb: &GlesTarget<'_>,
    region: Rectangle<i32, Physical>,
) -> anyhow::Result<Vec<u8>> {
    let area = Rectangle::new(
        (region.loc.x, region.loc.y).into(),
        (region.size.w, region.size.h).into(),
    );
    let mapping = renderer.copy_framebuffer(fb, area, Fourcc::Abgr8888)?;
    Ok(renderer.map_texture(&mapping)?.to_vec())
}

/// Écrit les pixels RVBA dans `dst` en xrgb8888, ligne après ligne à partir
/// de `offset` ; faux si `dst` est trop petit
fn copy_pixels(
    pixels: &[u8],
    size: Size<i32, Physical>,
    dst: &mut [u8],
    offset: usize,
    stride: usize,
) -> bool {
    let (w, h) = (size.w as usize, size.h as usize);
    if h == 0 {
        return true;
    }
    let need = offset + stride * (h - 1) + w * 4;
    if stride < w * 4 || need > dst.len() || pixels.len() < w * 4 * h {
        return false;
    }
    for y in 0..h {
        let src = &pixels[y * w * 4..(y + 1) * w * 4];
        let line = &mut dst[offset + y * stride..offset + y * stride + w * 4];
        // RVBA → BVRX (xrgb8888 petit-boutiste)
        for (d, s) in line.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
            *d = [s[2], s[1], s[0], 255];
        }
    }
    true
}

/// Copie d'un écran demandée par un client
struct Session {
    manager: ZwlrScreencopyManagerV1,
    output: Output,
    cursor: bool,
    image: Image,
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
pub const SHM_FORMATS: [wl_shm::Format; 2] = [wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888];
pub const DMABUF_FORMATS: [Fourcc; 2] = [Fourcc::Xrgb8888, Fourcc::Argb8888];

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
        let valid = buffer_fits(&buffer, size);
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
pub fn monotonic() -> (u64, u32) {
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
            Some(i) if sc.sessions[i].image.size == size && sc.sessions[i].image.scale == scale => i,
            found => {
                let image = match Image::new(renderer, size, scale) {
                    Ok(image) => image,
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
                    image,
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
        match session
            .image
            .capture(renderer, elements, r.region, &r.buffer, r.damage)
        {
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
