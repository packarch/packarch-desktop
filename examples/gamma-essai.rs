// SPDX-License-Identifier: MIT OR Apache-2.0
//! Client de test de wlr-gamma-control (§ 9.2, scénario `gamma`) : donne
//! une table gamma à chaque écran, comme gammastep, la garde un moment puis
//! quitte ; le compositeur doit alors rétablir la table d'origine.
//!
//! ```text
//! gamma-essai [--duration MS] [--short yes]
//! ```
//!
//! Table : rouge linéaire, vert à moitié, bleu nul. `--short yes` envoie
//! une table trop courte : le compositeur doit couper la connexion
//! (erreur `invalid_gamma`), et le client sort avec le code 3.

use std::io::{Seek, Write};
use std::os::fd::AsFd;

use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
    zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
};

#[derive(Default)]
struct Client {
    manager: Option<ZwlrGammaControlManagerV1>,
    outputs: Vec<wl_output::WlOutput>,
    short: bool,
    failed: bool,
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
                "zwlr_gamma_control_manager_v1" => state.manager = Some(registry.bind(name, 1, qh, ())),
                "wl_output" => state.outputs.push(registry.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, ()> for Client {
    fn event(
        state: &mut Self,
        control: &ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => {
                let n = size as usize;
                let ramp = |f: &dyn Fn(usize) -> usize| -> Vec<u16> {
                    (0..n).map(|i| f(i * 0xffff / (n - 1).max(1)) as u16).collect()
                };
                let mut table = ramp(&|v| v);
                table.extend(ramp(&|v| v / 2));
                if !state.short {
                    table.extend(ramp(&|_| 0));
                }
                let bytes: Vec<u8> = table.iter().flat_map(|v| v.to_ne_bytes()).collect();
                let mut file = tempfile();
                file.write_all(&bytes).expect("écriture de la table");
                file.rewind().expect("retour au début");
                control.set_gamma(file.as_fd());
            }
            zwlr_gamma_control_v1::Event::Failed => state.failed = true,
            _ => {}
        }
    }
}

/// Fichier anonyme en mémoire
fn tempfile() -> std::fs::File {
    use std::os::fd::{FromRawFd, OwnedFd};
    // SAFETY: appel système sans pointeur ; le descripteur rendu est neuf
    let fd = unsafe { libc::memfd_create(c"gamma-essai".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        eprintln!("gamma-essai: memfd_create: {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }
    // SAFETY: fd vient d'être créé et n'appartient qu'à nous
    std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) })
}

delegate_noop!(Client: ignore wl_output::WlOutput);
delegate_noop!(Client: ignore ZwlrGammaControlManagerV1);

fn usage() -> ! {
    eprintln!("gamma-essai [--duration MS] [--short yes]");
    std::process::exit(2);
}

fn main() {
    let mut duration = 1000u64;
    let mut client = Client::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--duration" => duration = value.parse().unwrap_or_else(|_| usage()),
            "--short" => client.short = value == "yes",
            _ => usage(),
        }
    }
    let conn = Connection::connect_to_env().unwrap_or_else(|e| {
        eprintln!("gamma-essai: {e}");
        std::process::exit(1);
    });
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let fail = |e: &dyn std::fmt::Display| -> ! {
        eprintln!("gamma-essai: {e}");
        std::process::exit(3);
    };
    if let Err(e) = queue.roundtrip(&mut client) {
        fail(&e);
    }
    let Some(manager) = client.manager.clone() else {
        eprintln!("gamma-essai: zwlr_gamma_control_manager_v1 absent");
        std::process::exit(1);
    };
    let _controls: Vec<ZwlrGammaControlV1> = client
        .outputs
        .iter()
        .map(|o| manager.get_gamma_control(o, &qh, ()))
        .collect();
    // Taille des tables, puis réponse à set_gamma
    for _ in 0..2 {
        if let Err(e) = queue.roundtrip(&mut client) {
            fail(&e);
        }
    }
    if client.failed {
        eprintln!("gamma-essai: réglage refusé");
        std::process::exit(4);
    }
    std::thread::sleep(std::time::Duration::from_millis(duration));
}
