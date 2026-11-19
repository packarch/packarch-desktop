// SPDX-License-Identifier: MIT OR Apache-2.0
//! Client de test des scénarios (§ 13) : pointeur virtuel
//! (wlr-virtual-pointer), pour vérifier le protocole de bout en bout.
//!
//! ```text
//! pointeur-essai ÉTAPE…
//!   abs X Y L H     mouvement absolu en (X, Y) sur une étendue L × H
//!   rel DX DY       mouvement relatif
//!   press | release bouton gauche
//!   rpress | rrelease bouton droit
//!   scroll N        N crans de molette verticale (négatif : vers le haut)
//! ```
//!
//! Chaque étape est suivie de `frame` ; le client attend que le compositeur
//! les ait toutes reçues avant de quitter.

use wayland_client::protocol::{wl_pointer, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

#[derive(Default)]
struct Client {
    manager: Option<ZwlrVirtualPointerManagerV1>,
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
        if let wl_registry::Event::Global { name, interface, .. } = event
            && interface == "zwlr_virtual_pointer_manager_v1"
        {
            state.manager = Some(registry.bind(name, 1, qh, ()));
        }
    }
}

delegate_noop!(Client: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(Client: ignore ZwlrVirtualPointerV1);

fn usage() -> ! {
    eprintln!("pointeur-essai [abs X Y L H | rel DX DY | press | release | scroll N]…");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let conn = Connection::connect_to_env().unwrap_or_else(|e| {
        eprintln!("pointeur-essai: {e}");
        std::process::exit(1);
    });
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut client = Client::default();
    if let Err(e) = queue.roundtrip(&mut client) {
        eprintln!("pointeur-essai: {e}");
        std::process::exit(1);
    }
    let Some(manager) = client.manager.clone() else {
        eprintln!("pointeur-essai: zwlr_virtual_pointer_manager_v1 absent");
        std::process::exit(1);
    };
    let pointer = manager.create_virtual_pointer(None, &qh, ());
    let num = |s: Option<&String>| -> f64 { s.and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()) };
    let mut it = args.iter();
    let mut time = 1;
    while let Some(step) = it.next() {
        match step.as_str() {
            "abs" => {
                let (x, y, w, h) = (num(it.next()), num(it.next()), num(it.next()), num(it.next()));
                pointer.motion_absolute(time, x as u32, y as u32, w as u32, h as u32);
            }
            "rel" => pointer.motion(time, num(it.next()), num(it.next())),
            "press" => pointer.button(time, BTN_LEFT, wl_pointer::ButtonState::Pressed),
            "release" => pointer.button(time, BTN_LEFT, wl_pointer::ButtonState::Released),
            "rpress" => pointer.button(time, BTN_RIGHT, wl_pointer::ButtonState::Pressed),
            "rrelease" => pointer.button(time, BTN_RIGHT, wl_pointer::ButtonState::Released),
            "scroll" => {
                let n = num(it.next()) as i32;
                pointer.axis_source(wl_pointer::AxisSource::Wheel);
                pointer.axis_discrete(time, wl_pointer::Axis::VerticalScroll, n as f64 * 15.0, n);
            }
            _ => usage(),
        }
        pointer.frame();
        time += 10;
    }
    pointer.destroy();
    if let Err(e) = queue.roundtrip(&mut client) {
        eprintln!("pointeur-essai: {e}");
        std::process::exit(1);
    }
}
