// SPDX-License-Identifier: MIT OR Apache-2.0
//! Client de test des scénarios (§ 13) : une fenêtre xdg-shell d'une couleur
//! unie, qui demande le cadre du compositeur (xdg-decoration). Son image ne
//! dépend ni des polices ni d'une horloge : les captures se comparent
//! exactement à des images de référence.
//!
//! ```text
//! client-essai [--title TITRE] [--size LxH] [--color RRGGBB] [--app-id ID]
//!              [--fullscreen ÉCRAN] [--window-menu yes]
//! ```
//!
//! `--fullscreen HEADLESS-2` demande le plein écran sur cet écran avant
//! d'afficher la fenêtre. `--window-menu yes` : un clic droit demande le
//! menu de fenêtre au compositeur (xdg_toplevel.show_window_menu), comme une
//! application qui dessine sa barre de titre.
//!
//! La taille proposée par le compositeur (maximisation, collage…) remplace
//! celle de `--size` ; 0 × 0 la rétablit. Le client quitte quand on ferme la
//! fenêtre.

use std::os::fd::{AsFd, FromRawFd, OwnedFd};

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::{self, ZxdgToplevelDecorationV1},
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Default)]
struct Client {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    decoration: Option<ZxdgDecorationManagerV1>,
    surface: Option<wl_surface::WlSurface>,
    /// Taille demandée par `--size`
    size: (i32, i32),
    /// Taille proposée par le dernier configure (0: libre)
    pending: (i32, i32),
    color: u32,
    closed: bool,
    /// Écrans et leurs noms (`--fullscreen`)
    outputs: Vec<(wl_output::WlOutput, String)>,
    seat: Option<wl_seat::WlSeat>,
    toplevel: Option<xdg_toplevel::XdgToplevel>,
    /// `--window-menu` : le clic droit demande le menu de fenêtre
    window_menu: bool,
    /// Position du pointeur sur la surface
    pointer: (f64, f64),
}

impl Client {
    /// Nouveau tampon de la couleur unie, à la taille voulue
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let (w, h) = match self.pending {
            (w, h) if w > 0 && h > 0 => (w, h),
            _ => self.size,
        };
        let (Some(shm), Some(surface)) = (&self.shm, &self.surface) else {
            return;
        };
        let stride = w * 4;
        let len = (stride * h) as usize;
        // SAFETY: appel système sans pointeur ; le descripteur rendu est neuf
        let fd = unsafe { libc::memfd_create(c"client-essai".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            eprintln!("client-essai: memfd_create: {}", std::io::Error::last_os_error());
            std::process::exit(1);
        }
        // SAFETY: fd vient d'être créé et n'appartient qu'à nous
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let pixel = self.color.to_le_bytes();
        let data: Vec<u8> = pixel.iter().copied().cycle().take(len).collect();
        let mut file = std::fs::File::from(fd);
        if let Err(e) = std::io::Write::write_all(&mut file, &data) {
            eprintln!("client-essai: {e}");
            std::process::exit(1);
        }
        let pool = shm.create_pool(file.as_fd(), len as i32, qh, ());
        let buffer = pool.create_buffer(0, w, h, stride, wl_shm::Format::Xrgb8888, qh, ());
        pool.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
    }
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
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(registry.bind(name, version.min(4), qh, ())),
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
                "zxdg_decoration_manager_v1" => state.decoration = Some(registry.bind(name, 1, qh, ())),
                "wl_output" => {
                    let output = registry.bind(name, 4, qh, ());
                    state.outputs.push((output, String::new()));
                }
                "wl_seat" if state.window_menu => {
                    let seat: wl_seat::WlSeat = registry.bind(name, 5, qh, ());
                    seat.get_pointer(qh, ());
                    state.seat = Some(seat);
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        state: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            state.draw(qh);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => state.pending = (width, height),
            xdg_toplevel::Event::Close => state.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgToplevelDecorationV1, ()> for Client {
    fn event(
        _: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        _: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_buffer::WlBuffer, ()> for Client {
    fn event(
        _: &mut Self,
        buffer: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            buffer.destroy();
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Client {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(o) = state.outputs.iter_mut().find(|(o, _)| o == output)
        {
            o.1 = name;
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface_x, surface_y, ..
            }
            | wl_pointer::Event::Motion {
                surface_x, surface_y, ..
            } => state.pointer = (surface_x, surface_y),
            // BTN_RIGHT
            wl_pointer::Event::Button {
                serial,
                button: 0x111,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => {
                if let (Some(toplevel), Some(seat)) = (&state.toplevel, &state.seat) {
                    toplevel.show_window_menu(seat, serial, state.pointer.0 as i32, state.pointer.1 as i32);
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(Client: ignore wl_seat::WlSeat);
delegate_noop!(Client: ignore wl_compositor::WlCompositor);
delegate_noop!(Client: ignore wl_surface::WlSurface);
delegate_noop!(Client: ignore wl_shm::WlShm);
delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Client: ignore ZxdgDecorationManagerV1);

fn usage() -> ! {
    eprintln!(
        "client-essai [--title TITRE] [--size LxH] [--color RRGGBB] [--app-id ID] [--fullscreen ÉCRAN] \
         [--window-menu yes]"
    );
    std::process::exit(2);
}

fn main() {
    let mut title = "essai".to_string();
    let mut app_id = "client-essai".to_string();
    let mut fullscreen = None;
    let mut client = Client {
        size: (400, 300),
        color: 0x3a6ea5,
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--title" => title = value,
            "--fullscreen" => fullscreen = Some(value),
            "--window-menu" => client.window_menu = value == "yes",
            "--app-id" => app_id = value,
            "--size" => {
                let (w, h) = value.split_once('x').unwrap_or_else(|| usage());
                client.size = match (w.parse(), h.parse()) {
                    (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
                    _ => usage(),
                };
            }
            "--color" => {
                client.color =
                    u32::from_str_radix(value.trim_start_matches('#'), 16).unwrap_or_else(|_| usage())
            }
            _ => usage(),
        }
    }

    let conn = Connection::connect_to_env().unwrap_or_else(|e| {
        eprintln!("client-essai: {e}");
        std::process::exit(1);
    });
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    if let Err(e) = queue.roundtrip(&mut client) {
        eprintln!("client-essai: {e}");
        std::process::exit(1);
    }
    let (Some(compositor), Some(wm_base)) = (client.compositor.clone(), client.wm_base.clone()) else {
        eprintln!("client-essai: wl_compositor ou xdg_wm_base absent");
        std::process::exit(1);
    };
    if client.shm.is_none() {
        eprintln!("client-essai: wl_shm absent");
        std::process::exit(1);
    }
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_title(title);
    toplevel.set_app_id(app_id);
    if let Some(manager) = &client.decoration {
        let deco = manager.get_toplevel_decoration(&toplevel, &qh, ());
        deco.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
    }
    if let Some(name) = fullscreen {
        // Second aller-retour : noms des écrans
        if let Err(e) = queue.roundtrip(&mut client) {
            eprintln!("client-essai: {e}");
            std::process::exit(1);
        }
        let Some((output, _)) = client.outputs.iter().find(|(_, n)| *n == name) else {
            eprintln!("client-essai: pas d'écran « {name} »");
            std::process::exit(1);
        };
        toplevel.set_fullscreen(Some(output));
    }
    client.toplevel = Some(toplevel.clone());
    client.surface = Some(surface.clone());
    surface.commit();
    while !client.closed {
        if let Err(e) = queue.blocking_dispatch(&mut client) {
            eprintln!("client-essai: {e}");
            std::process::exit(1);
        }
    }
}
