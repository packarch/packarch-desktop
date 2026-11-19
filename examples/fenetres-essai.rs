// SPDX-License-Identifier: MIT OR Apache-2.0
//! Client de test des scénarios (§ 13) : identifiant ext-foreign-toplevel
//! d'une fenêtre, pour `grim -T`.
//!
//! ```text
//! fenetres-essai TITRE   affiche l'identifiant de la fenêtre de ce titre
//! ```

use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop, event_created_child};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};

#[derive(Default)]
struct Client {
    list: Option<ExtForeignToplevelListV1>,
    /// (poignée, identifiant, titre)
    toplevels: Vec<(ExtForeignToplevelHandleV1, String, String)>,
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
            && interface == "ext_foreign_toplevel_list_v1"
        {
            state.list = Some(registry.bind(name, 1, qh, ()));
        }
    }
}

impl Dispatch<ExtForeignToplevelListV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state.toplevels.push((toplevel, String::new(), String::new()));
        }
    }

    event_created_child!(Client, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for Client {
    fn event(
        state: &mut Self,
        handle: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(t) = state.toplevels.iter_mut().find(|t| t.0 == *handle) else {
            return;
        };
        match event {
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => t.1 = identifier,
            ext_foreign_toplevel_handle_v1::Event::Title { title } => t.2 = title,
            _ => {}
        }
    }
}

delegate_noop!(Client: ignore wayland_client::protocol::wl_callback::WlCallback);

fn main() {
    let Some(title) = std::env::args().nth(1) else {
        eprintln!("fenetres-essai TITRE");
        std::process::exit(2);
    };
    let conn = Connection::connect_to_env().unwrap_or_else(|e| {
        eprintln!("fenetres-essai: {e}");
        std::process::exit(1);
    });
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut client = Client::default();
    for _ in 0..3 {
        if let Err(e) = queue.roundtrip(&mut client) {
            eprintln!("fenetres-essai: {e}");
            std::process::exit(1);
        }
    }
    if client.list.is_none() {
        eprintln!("fenetres-essai: ext_foreign_toplevel_list_v1 absent");
        std::process::exit(1);
    }
    match client.toplevels.iter().find(|t| t.2 == title) {
        Some(t) => println!("{}", t.1),
        None => {
            eprintln!("fenetres-essai: pas de fenêtre « {title} »");
            std::process::exit(1);
        }
    }
}
