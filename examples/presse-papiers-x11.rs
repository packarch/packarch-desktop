// SPDX-License-Identifier: MIT OR Apache-2.0
//! Client de test des scénarios (§ 13) : lit le presse-papiers (CLIPBOARD)
//! comme un client X11 sans fenêtre visible, à la manière de spice-vdagent
//! ou d'un gestionnaire de presse-papiers X11, et l'écrit sur la sortie
//! standard. Code de sortie 1 si la lecture est refusée ou n'aboutit pas
//! dans les 3 secondes.
//!
//! ```text
//! presse-papiers-x11
//! ```

use std::time::{Duration, Instant};

use x11rb::NONE;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, CreateWindowAux, WindowClass};

fn main() {
    match lire() {
        Ok(texte) => print!("{texte}"),
        Err(e) => {
            eprintln!("presse-papiers-x11 : {e}");
            std::process::exit(1);
        }
    }
}

fn lire() -> Result<String, Box<dyn std::error::Error>> {
    let (conn, ecran) = x11rb::connect(None)?;
    let racine = conn.setup().roots[ecran].root;
    // Fenêtre jamais affichée : seulement destinataire de la sélection
    let fenetre = conn.generate_id()?;
    conn.create_window(
        0,
        fenetre,
        racine,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new(),
    )?;
    let atome = |nom: &str| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, nom.as_bytes())?.reply()?.atom)
    };
    let (clipboard, utf8, propriete) = (
        atome("CLIPBOARD")?,
        atome("UTF8_STRING")?,
        atome("PACKARCH_ESSAI")?,
    );
    conn.convert_selection(fenetre, clipboard, utf8, propriete, x11rb::CURRENT_TIME)?;
    conn.flush()?;
    let fin = Instant::now() + Duration::from_secs(3);
    while Instant::now() < fin {
        match conn.poll_for_event()? {
            Some(Event::SelectionNotify(n)) => {
                if n.property == NONE {
                    return Err("lecture refusée par le propriétaire de la sélection".into());
                }
                let r = conn
                    .get_property(true, fenetre, propriete, AtomEnum::ANY, 0, u32::MAX / 4)?
                    .reply()?;
                return Ok(String::from_utf8_lossy(&r.value).into_owned());
            }
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    Err("pas de réponse en 3 secondes".into())
}
