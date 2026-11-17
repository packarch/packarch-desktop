// SPDX-License-Identifier: MIT OR Apache-2.0
//! Session (cahier des charges § 5.6, § 5.7, § 12): environnement, clavier,
//! `autostart`, signaux, et commandes `--reconfigure` / `--exit`.

use std::path::PathBuf;

use smithay::input::keyboard::XkbConfig;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::signals::{Signal, Signals};
use tracing::{info, warn};

use crate::env;
use crate::state::State;

/// Fichier du numéro de processus d'une instance
pub fn pid_file(display: &str) -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join(format!("packarch-{display}.pid")))
}

/// Envoie un signal à l'instance en cours (`--reconfigure`, `--exit`):
/// `PACKARCH_PID`, sinon le fichier de `WAYLAND_DISPLAY`
pub fn signal_instance(sig: libc::c_int) -> Result<(), String> {
    let pid = std::env::var("PACKARCH_PID")
        .ok()
        .and_then(|p| p.parse::<i32>().ok())
        .or_else(|| {
            let display = std::env::var("WAYLAND_DISPLAY").ok()?;
            std::fs::read_to_string(pid_file(&display)?)
                .ok()?
                .trim()
                .parse()
                .ok()
        })
        .ok_or("aucune instance de packarch trouvée (ni PACKARCH_PID, ni fichier du numéro de processus)")?;
    // Sûr: simple appel système
    if unsafe { libc::kill(pid, sig) } != 0 {
        return Err(format!(
            "signal au processus {pid}: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

impl State {
    /// Environnement des programmes lancés: variables de la session et du
    /// fichier `environment`
    pub fn apply_session_env(&mut self) {
        self.env_vars = env::read_environment(&self.config.dir);
        let socket = self.socket_name.to_string_lossy().to_string();
        let mut vars = vec![
            ("WAYLAND_DISPLAY".to_string(), socket),
            ("PACKARCH_PID".to_string(), std::process::id().to_string()),
        ];
        vars.extend(self.env_vars.iter().cloned());
        self.child_env = vars;
    }

    /// Disposition, répétition, verrouillage numérique (§ 5.2, § 5.6)
    pub fn apply_keyboard(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let x = env::xkb(&self.env_vars);
        let cfg = XkbConfig {
            rules: &x.rules,
            model: &x.model,
            layout: &x.layout,
            variant: &x.variant,
            options: x.options.clone(),
        };
        if let Err(e) = keyboard.set_xkb_config(self, cfg) {
            warn!("disposition du clavier {:?}: {e:?}", x.layout);
        } else {
            info!(
                "clavier: {} {}",
                if x.layout.is_empty() { "us" } else { &x.layout },
                x.variant
            );
        }
        keyboard.change_repeat_info(self.config.repeat_rate, self.config.repeat_delay);
        if let Some(on) = self.config.numlock {
            let mut mods = keyboard.modifier_state();
            mods.num_lock = on;
            keyboard.set_modifier_state(mods);
        }
    }

    /// Démarrage de la session, après la mise en place de l'affichage
    pub fn start_session(&mut self) {
        let display = self.socket_name.to_string_lossy().to_string();
        if let Some(path) = pid_file(&display)
            && let Err(e) = std::fs::write(&path, format!("{}\n", std::process::id()))
        {
            warn!("{}: {e}", path.display());
        }
        let autostart = self.config.dir.join("autostart");
        if autostart.is_file() {
            info!("autostart: {}", autostart.display());
            let cmd = format!("sh '{}'", autostart.display().to_string().replace('\'', "'\\''"));
            self.spawn(&cmd);
        }
    }

    /// Fin de la session: fichier du numéro de processus
    pub fn end_session(&self) {
        if let Some(path) = pid_file(&self.socket_name.to_string_lossy()) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// SIGHUP recharge, SIGTERM et SIGINT quittent proprement
pub fn watch_signals(handle: &LoopHandle<'static, State>) {
    match Signals::new(&[Signal::SIGHUP, Signal::SIGTERM, Signal::SIGINT]) {
        Ok(signals) => {
            let r = handle.insert_source(signals, |event, _, state| match event.signal() {
                Signal::SIGHUP => state.reconfigure(),
                _ => state.exit(),
            });
            if let Err(e) = r {
                warn!("signaux: {e}");
            }
        }
        Err(e) => warn!("signaux: {e}"),
    }
}
