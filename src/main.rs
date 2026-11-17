// SPDX-License-Identifier: MIT OR Apache-2.0
//! packarch: compositeur Wayland à fenêtres flottantes de Packarch

#![allow(
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::large_enum_variant
)]

mod actions;
mod backend;
mod bindings;
mod config;
mod deco;
mod env;
mod handlers;
mod input;
mod render;
mod script;
mod session;
mod state;
mod theme;
mod view;
mod wm;

use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use tracing::{error, info};

use crate::config::Config;
use crate::state::State;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BackendKind {
    Winit,
    Headless,
}

/// Compositeur Wayland à fenêtres flottantes de Packarch
#[derive(Debug, Parser)]
#[command(name = "packarch", disable_version_flag = true)]
struct Cli {
    /// Dossier de configuration
    #[arg(short = 'C', long, value_name = "DOSSIER")]
    config_dir: Option<PathBuf>,
    /// Commande lancée au démarrage
    #[arg(short, long, value_name = "COMMANDE")]
    startup: Vec<String>,
    /// Demande à l'instance en cours de recharger sa configuration
    #[arg(short, long)]
    reconfigure: bool,
    /// Demande à l'instance en cours de quitter
    #[arg(short, long)]
    exit: bool,
    /// Journal détaillé
    #[arg(short, long)]
    debug: bool,
    /// Backend (udev: version 0.4)
    #[arg(long, value_enum, default_value = "winit")]
    backend: BackendKind,
    /// Taille de l'écran headless, LARGEURxHAUTEUR
    #[arg(long, hide = true, default_value = "1280x800")]
    size: String,
    /// Échelle de l'écran
    #[arg(long, hide = true, default_value_t = 1.0)]
    scale: f64,
    /// Scénario d'essai (voir src/script.rs)
    #[arg(long, hide = true, value_name = "FICHIER")]
    script: Option<PathBuf>,
}

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn main() {
    // -v, --version (§ 12)
    if std::env::args_os().skip(1).any(|a| a == "-v" || a == "--version") {
        println!("packarch {}", version());
        return;
    }
    let cli = Cli::parse();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(if cli.debug {
            "debug,smithay=info"
        } else {
            "info,smithay=warn"
        })
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
    if cli.reconfigure || cli.exit {
        let sig = if cli.exit { libc::SIGTERM } else { libc::SIGHUP };
        if let Err(e) = session::signal_instance(sig) {
            eprintln!("packarch: {e}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(e) = run(cli) {
        error!("{e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    info!("packarch {}", version());
    let dir = config::config_dir(cli.config_dir.as_deref());
    info!("configuration: {}", dir.display());
    let config = Config::load(&dir).unwrap_or_else(|e| {
        tracing::warn!("{e}; configuration par défaut");
        Config::fallback(&dir)
    });

    let mut event_loop: EventLoop<'static, State> = EventLoop::try_new()?;
    let display: Display<State> = Display::new()?;
    let mut state = State::new(display, event_loop.handle(), event_loop.get_signal(), config);

    match cli.backend {
        BackendKind::Winit => backend::winit::init(&mut event_loop, &mut state, cli.scale)?,
        BackendKind::Headless => {
            let (w, h) = cli
                .size
                .split_once('x')
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                .ok_or_else(|| anyhow::anyhow!("--size: LARGEURxHAUTEUR attendu"))?;
            backend::headless::init(&mut event_loop, &mut state, (w, h), cli.scale)?
        }
    }

    state.apply_session_env();
    state.apply_keyboard();
    session::watch_signals(&event_loop.handle());
    state.start_session();
    for cmd in &cli.startup {
        state.spawn(cmd);
    }
    if let Some(path) = &cli.script {
        let text = std::fs::read_to_string(path)?;
        let steps = script::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        script::run(&event_loop.handle(), steps);
    }

    let result = event_loop.run(None, &mut state, |_| {});
    state.end_session();
    result?;
    if script::FAILED.load(std::sync::atomic::Ordering::Relaxed) {
        anyhow::bail!("scénario d'essai: au moins une vérification a échoué");
    }
    Ok(())
}
