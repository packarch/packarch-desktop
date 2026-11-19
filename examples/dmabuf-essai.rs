// SPDX-License-Identifier: MIT OR Apache-2.0
//! Client de test (§ 13) : capture du premier écran par wlr-screencopy dans
//! un dmabuf linéaire XR24 alloué par GBM, comme wf-recorder, écrite en PNG.
//! À comparer à `grim` (tampon shm).
//!
//! ```text
//! dmabuf-essai NŒUD SORTIE.png   par exemple /dev/dri/renderD128
//! ```

use std::fs::File;
use std::os::fd::AsFd;

use smithay::reexports::gbm::{self, BufferObjectFlags, Format};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");

#[derive(Default)]
struct Client {
    output: Option<WlOutput>,
    manager: Option<ZwlrScreencopyManagerV1>,
    dmabuf: Option<ZwpLinuxDmabufV1>,
    /// Taille proposée pour un dmabuf XR24
    size: Option<(u32, u32)>,
    buffer_done: bool,
    /// Vrai : image copiée ; faux : échec
    result: Option<bool>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, .. } = event {
            match interface.as_str() {
                "wl_output" if state.output.is_none() => state.output = Some(registry.bind(name, 1, qh, ())),
                "zwlr_screencopy_manager_v1" => state.manager = Some(registry.bind(name, 3, qh, ())),
                "zwp_linux_dmabuf_v1" => state.dmabuf = Some(registry.bind(name, 3, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_screencopy_frame_v1::Event::LinuxDmabuf {
                format,
                width,
                height,
            } if format == XRGB8888 => state.size = Some((width, height)),
            zwlr_screencopy_frame_v1::Event::BufferDone => state.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Ready { .. } => state.result = Some(true),
            zwlr_screencopy_frame_v1::Event::Failed => state.result = Some(false),
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_buffer_params_v1::Event::Failed = event {
            state.result = Some(false);
        }
    }
}

delegate_noop!(Client: ignore WlOutput);
delegate_noop!(Client: ignore WlBuffer);
delegate_noop!(Client: ignore ZwlrScreencopyManagerV1);
delegate_noop!(Client: ignore ZwpLinuxDmabufV1);

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("dmabuf-essai: {msg}");
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, node, out] = args.as_slice() else {
        eprintln!("dmabuf-essai NŒUD SORTIE.png");
        std::process::exit(2);
    };
    let conn = Connection::connect_to_env().unwrap_or_else(|e| fail(e));
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut client = Client::default();
    queue.roundtrip(&mut client).unwrap_or_else(|e| fail(e));
    let (Some(output), Some(manager), Some(dmabuf)) = (
        client.output.clone(),
        client.manager.clone(),
        client.dmabuf.clone(),
    ) else {
        fail("wl_output, zwlr_screencopy_manager_v1 ou zwp_linux_dmabuf_v1 absent");
    };

    let frame = manager.capture_output(0, &output, &qh, ());
    while !client.buffer_done {
        queue.blocking_dispatch(&mut client).unwrap_or_else(|e| fail(e));
    }
    let Some((w, h)) = client.size else {
        fail("pas de dmabuf XR24 proposé");
    };

    let file = File::options()
        .read(true)
        .write(true)
        .open(node)
        .unwrap_or_else(|e| fail(format!("{node} : {e}")));
    let device = gbm::Device::new(file).unwrap_or_else(|e| fail(format!("GBM : {e}")));
    // Tampon linéaire, comme wf-recorder ; NVIDIA refuse LINEAR | RENDERING
    let bo = device
        .create_buffer_object::<()>(w, h, Format::Xrgb8888, BufferObjectFlags::LINEAR)
        .unwrap_or_else(|e| fail(format!("allocation : {e}")));
    let fd = bo.fd().unwrap_or_else(|e| fail(format!("export : {e}")));
    let stride = bo.stride();
    let modifier = u64::from(bo.modifier());
    let params = dmabuf.create_params(&qh, ());
    params.add(
        fd.as_fd(),
        0,
        bo.offset(0),
        stride,
        (modifier >> 32) as u32,
        modifier as u32,
    );
    let buffer = params.create_immed(
        w as i32,
        h as i32,
        XRGB8888,
        zwp_linux_buffer_params_v1::Flags::empty(),
        &qh,
        (),
    );

    frame.copy(&buffer);
    while client.result.is_none() {
        queue.blocking_dispatch(&mut client).unwrap_or_else(|e| fail(e));
    }
    if client.result != Some(true) {
        fail("copie refusée (failed)");
    }

    let len = stride as usize * h as usize;
    // Sûr : projection en lecture seule du tampon, libérée plus bas
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            std::os::fd::AsRawFd::as_raw_fd(&fd),
            0,
        )
    };
    if ptr == libc::MAP_FAILED {
        fail(format!("mmap : {}", std::io::Error::last_os_error()));
    }
    let data = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h as usize {
        for px in data[y * stride as usize..][..w as usize * 4].as_chunks::<4>().0 {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
    }
    unsafe { libc::munmap(ptr, len) };

    let file = File::create(out).unwrap_or_else(|e| fail(e));
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&rgb))
        .unwrap_or_else(|e| fail(e));
    println!("{w}×{h}, modificateur {modifier:#x}, pas {stride}");
}
