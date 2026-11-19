// SPDX-License-Identifier: MIT OR Apache-2.0
//! packarch-partage : sélecteur du partage d'écran (cahier des charges
//! § 9.3, décision 30), lancé par xdg-desktop-portal-wlr
//! (`chooser_type=simple`). Propose les écrans et les fenêtres dans un menu
//! (rofi) et écrit le choix sur la sortie standard : `Monitor: NOM` ou
//! `Window: IDENTIFIANT` (identifiant ext-foreign-toplevel-list). Rien si
//! l'utilisateur annule.
//!
//! ```text
//! packarch-partage            menu rofi
//! packarch-partage --liste    écrit les lignes du menu, sans menu
//! PACKARCH_PARTAGE_MENU="wofi --dmenu" packarch-partage   autre menu (dmenu)
//! ```

use std::io::Write;
use std::process::{Command, Stdio};

use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{Connection, Dispatch, QueueHandle, event_created_child};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};

#[derive(Default)]
struct Output {
    name: String,
    description: String,
}

#[derive(Default)]
struct Toplevel {
    identifier: String,
    title: String,
    app_id: String,
    closed: bool,
}

#[derive(Default)]
struct Client {
    outputs: Vec<(wl_output::WlOutput, Output)>,
    list: Option<ExtForeignToplevelListV1>,
    toplevels: Vec<(ExtForeignToplevelHandleV1, Toplevel)>,
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
                // Version 4 : nom et description des écrans
                "wl_output" if version >= 4 => {
                    let o = registry.bind(name, 4, qh, ());
                    state.outputs.push((o, Output::default()));
                }
                "ext_foreign_toplevel_list_v1" => state.list = Some(registry.bind(name, 1, qh, ())),
                _ => {}
            }
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
        let Some((_, o)) = state.outputs.iter_mut().find(|(w, _)| w == output) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => o.name = name,
            wl_output::Event::Description { description } => o.description = description,
            _ => {}
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
            state.toplevels.push((toplevel, Toplevel::default()));
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
        let Some((_, t)) = state.toplevels.iter_mut().find(|(h, _)| h == handle) else {
            return;
        };
        match event {
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => t.identifier = identifier,
            ext_foreign_toplevel_handle_v1::Event::Title { title } => t.title = title,
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => t.app_id = app_id,
            ext_foreign_toplevel_handle_v1::Event::Closed => t.closed = true,
            _ => {}
        }
    }
}

/// Une ligne du menu et la réponse au portail
struct Choice {
    label: String,
    answer: String,
}

/// Écrans puis fenêtres du compositeur
fn choices() -> Result<Vec<Choice>, String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut client = Client::default();
    // Globales, puis écrans et fenêtres, puis leurs propriétés
    for _ in 0..3 {
        queue.roundtrip(&mut client).map_err(|e| e.to_string())?;
    }
    let mut out = vec![];
    for (_, o) in client.outputs.iter().filter(|(_, o)| !o.name.is_empty()) {
        // La description finit souvent par le nom (« iiyama - PL2773H - HDMI-A-2 »)
        let description = o
            .description
            .strip_suffix(&o.name)
            .map(|d| d.trim_end_matches([' ', '-', '(']))
            .unwrap_or(&o.description);
        let label = if description.is_empty() {
            format!("Écran  {}", o.name)
        } else {
            format!("Écran  {} — {}", o.name, description)
        };
        out.push(Choice {
            label,
            answer: format!("Monitor: {}", o.name),
        });
    }
    if client.list.is_none() {
        eprintln!("packarch-partage: ext_foreign_toplevel_list_v1 absent, écrans seulement");
    }
    for (_, t) in client
        .toplevels
        .iter()
        .filter(|(_, t)| !t.closed && !t.identifier.is_empty())
    {
        let name = match (t.title.is_empty(), t.app_id.is_empty()) {
            (false, false) => format!("{} ({})", t.title, t.app_id),
            (false, true) => t.title.clone(),
            (true, false) => t.app_id.clone(),
            (true, true) => "(sans titre)".into(),
        };
        out.push(Choice {
            // Une ligne par fenêtre : pas de retour à la ligne dans le titre
            label: format!("Fenêtre  {}", name.replace(['\n', '\r'], " ")),
            answer: format!("Window: {}", t.identifier),
        });
    }
    Ok(out)
}

/// Ouvre le menu ; rend l'indice de la ligne choisie
fn menu(choices: &[Choice]) -> Result<Option<usize>, String> {
    let custom = std::env::var("PACKARCH_PARTAGE_MENU")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let mut cmd = match &custom {
        Some(c) => {
            let mut cmd = Command::new("sh");
            cmd.arg("-c").arg(c);
            cmd
        }
        None => {
            let mut cmd = Command::new("rofi");
            cmd.args(["-dmenu", "-i", "-no-custom", "-format", "i", "-p", "Partager"]);
            cmd
        }
    };
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("menu: {e}"))?;
    {
        let mut stdin = child.stdin.take().ok_or("menu: pas d'entrée")?;
        for c in choices {
            writeln!(stdin, "{}", c.label).map_err(|e| format!("menu: {e}"))?;
        }
    }
    let out = child.wait_with_output().map_err(|e| format!("menu: {e}"))?;
    let picked = String::from_utf8_lossy(&out.stdout)
        .trim_end_matches('\n')
        .to_string();
    if picked.is_empty() {
        return Ok(None);
    }
    Ok(match custom {
        // Autre menu : la ligne choisie
        Some(_) => choices.iter().position(|c| c.label == picked),
        // rofi -format i : l'indice
        None => picked.trim().parse().ok().filter(|i| *i < choices.len()),
    })
}

fn main() {
    let choices = match choices() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("packarch-partage: {e}");
            std::process::exit(1);
        }
    };
    if std::env::args().nth(1).as_deref() == Some("--liste") {
        for c in &choices {
            println!("{}\t{}", c.label, c.answer);
        }
        return;
    }
    if choices.is_empty() {
        eprintln!("packarch-partage: aucun écran ni fenêtre");
        return;
    }
    match menu(&choices) {
        Ok(Some(i)) => println!("{}", choices[i].answer),
        // Annulé : rien sur la sortie, le portail refuse le partage
        Ok(None) => {}
        Err(e) => {
            eprintln!("packarch-partage: {e}");
            std::process::exit(1);
        }
    }
}
