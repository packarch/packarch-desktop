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
            ("XDG_CURRENT_DESKTOP".to_string(), "packarch:wlroots".to_string()),
            ("XDG_SESSION_DESKTOP".to_string(), "packarch".to_string()),
            ("XDG_SESSION_TYPE".to_string(), "wayland".to_string()),
        ];
        if let Some(n) = self.xwayland.display {
            vars.push(("DISPLAY".to_string(), format!(":{n}")));
        }
        if let Some((address, _)) = &self.private_bus {
            vars.push(("DBUS_SESSION_BUS_ADDRESS".to_string(), address.clone()));
        }
        vars.extend(self.env_vars.iter().cloned());
        self.child_env = vars;
    }

    /// Envoie l'environnement de la session à systemd et à D-Bus (portails,
    /// services), seulement en session réelle: imbriqué, il détournerait
    /// celui de la session hôte (§ 5.6)
    pub fn export_session_env(&mut self) {
        if !matches!(self.backend, Some(crate::backend::Backend::Udev(_))) {
            return;
        }
        // Une autre session graphique du même utilisateur est active (essai
        // sur une autre console): son environnement est gardé, sinon ses
        // programmes lancés par D-Bus s'ouvriraient ici
        if let Some(other) = other_session_display(&self.socket_name.to_string_lossy()) {
            if !self.env_exported {
                warn!("autre session active ({other}): environnement non exporté vers systemd et D-Bus");
            }
            return;
        }
        self.env_exported = true;
        let names = [
            "WAYLAND_DISPLAY",
            "DISPLAY",
            "XDG_CURRENT_DESKTOP",
            "XDG_SESSION_DESKTOP",
            "XDG_SESSION_TYPE",
            "PACKARCH_PID",
        ];
        let present: Vec<&str> = names
            .into_iter()
            .filter(|n| self.child_env.iter().any(|(k, _)| k == n))
            .collect();
        self.spawn(&format!(
            "dbus-update-activation-environment --systemd {}",
            present.join(" ")
        ));
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
            let layout = if x.layout.is_empty() { "us" } else { &x.layout };
            if x.variant.is_empty() {
                info!("clavier: {layout}");
            } else {
                info!("clavier: {layout} {}", x.variant);
            }
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
        // Session réelle à côté d'une autre (essai sur une autre console):
        // bus D-Bus à part, pour que les applications à instance unique
        // (Thunar…) ne s'ouvrent pas dans l'autre session
        if matches!(self.backend, Some(crate::backend::Backend::Udev(_)))
            && let Some(other) = other_session_display(&display)
        {
            match start_private_bus() {
                Ok(bus) => {
                    info!("autre session active ({other}): bus D-Bus à part ({})", bus.0);
                    self.private_bus = Some(bus);
                    self.apply_session_env();
                }
                Err(e) => warn!("bus D-Bus à part: {e}"),
            }
        }
        if let Some(path) = pid_file(&display)
            && let Err(e) = std::fs::write(&path, format!("{}\n", std::process::id()))
        {
            warn!("{}: {e}", path.display());
        }
        self.export_session_env();
        // autostart après XWayland (DISPLAY défini), 3 s au plus
        self.start_xwayland();
        let _ = self.handle.insert_source(
            smithay::reexports::calloop::timer::Timer::from_duration(std::time::Duration::from_secs(3)),
            |_, _, state| {
                state.run_autostart();
                smithay::reexports::calloop::timer::TimeoutAction::Drop
            },
        );
    }

    /// Script `autostart` (une seule fois)
    pub fn run_autostart(&mut self) {
        if std::mem::replace(&mut self.autostarted, true) {
            return;
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
        if let Some((_, pid)) = self.private_bus {
            // Sûr: simple appel système vers notre propre bus
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
        }
        // Les variables exportées ne doivent pas survivre à la session
        if self.env_exported {
            let _ = std::process::Command::new("systemctl")
                .args([
                    "--user",
                    "unset-environment",
                    "WAYLAND_DISPLAY",
                    "DISPLAY",
                    "XDG_CURRENT_DESKTOP",
                    "XDG_SESSION_DESKTOP",
                    "PACKARCH_PID",
                ])
                .status();
        }
    }
}

/// Lance un bus D-Bus de session pour cette session seulement: adresse et
/// numéro de processus
fn start_private_bus() -> Result<(String, i32), String> {
    let out = std::process::Command::new("dbus-daemon")
        .args(["--session", "--fork", "--print-address=1", "--print-pid=1"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let address = lines.next().map(str::trim).filter(|a| !a.is_empty());
    let pid = lines.next().and_then(|p| p.trim().parse::<i32>().ok());
    match (address, pid) {
        (Some(a), Some(p)) => Ok((a.to_string(), p)),
        _ => Err(format!("réponse inattendue de dbus-daemon: {text:?}")),
    }
}

/// `WAYLAND_DISPLAY` de l'environnement de systemd s'il désigne la socket
/// encore présente d'une autre session que la nôtre (`ours`)
fn other_session_display(ours: &str) -> Option<String> {
    let out = std::process::Command::new("systemctl")
        .args(["--user", "show-environment"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let display = text
        .lines()
        .find_map(|l| l.strip_prefix("WAYLAND_DISPLAY="))?
        .trim()
        .to_string();
    if display.is_empty() || display == ours {
        return None;
    }
    let path = if display.starts_with('/') {
        PathBuf::from(&display)
    } else {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(&display)
    };
    // Socket présente et quelqu'un l'écoute
    std::os::unix::net::UnixStream::connect(&path).ok()?;
    Some(display)
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
