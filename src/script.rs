// SPDX-License-Identifier: MIT OR Apache-2.0
//! Scénario d'essai (option cachée `--script`): entrées simulées et
//! captures, pour vérifier le rendu et les gestes sans écran (§ 13).
//!
//! Une commande par ligne, `#` pour les commentaires:
//!
//! ```text
//! spawn kitty            lance une commande
//! wait 500               attend 500 ms
//! wait-views 2 5000      attend 2 fenêtres affichées (5 s au plus)
//! move 400 300           pointeur en (400, 300), coordonnées logiques
//! press left             appuie (left, right, middle)
//! release left
//! click left
//! key 30                 touche (code evdev), appui puis relâche
//! capture /tmp/a.png     capture de l'écran (backend headless)
//! quit
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use tracing::{info, warn};

use crate::backend::Backend;
use crate::input::{BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use crate::state::State;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Spawn(String),
    Wait(u64),
    WaitViews(usize, u64),
    Move(f64, f64),
    Button(u32, bool),
    Click(u32),
    Key(u32),
    Capture(PathBuf),
    Quit,
}

fn button(s: Option<&str>) -> Result<u32, String> {
    match s {
        Some("left") => Ok(BTN_LEFT),
        Some("right") => Ok(BTN_RIGHT),
        Some("middle") => Ok(BTN_MIDDLE),
        other => Err(format!("bouton inconnu: {other:?}")),
    }
}

pub fn parse(text: &str) -> Result<Vec<Step>, String> {
    let mut steps = vec![];
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let mut args = rest.split_whitespace();
        let num = |s: Option<&str>| -> Result<f64, String> {
            s.ok_or("argument manquant")?
                .parse::<f64>()
                .map_err(|e| e.to_string())
        };
        let step = (|| -> Result<Step, String> {
            Ok(match cmd {
                "spawn" => Step::Spawn(rest.to_string()),
                "wait" => Step::Wait(num(args.next())? as u64),
                "wait-views" => {
                    let n = num(args.next())? as usize;
                    let t = args.next().map(|s| s.parse().unwrap_or(5000)).unwrap_or(5000);
                    Step::WaitViews(n, t)
                }
                "move" => Step::Move(num(args.next())?, num(args.next())?),
                "press" => Step::Button(button(args.next())?, true),
                "release" => Step::Button(button(args.next())?, false),
                "click" => Step::Click(button(args.next())?),
                "key" => Step::Key(num(args.next())? as u32),
                "capture" => Step::Capture(PathBuf::from(rest)),
                "quit" => Step::Quit,
                _ => return Err(format!("commande inconnue: {cmd}")),
            })
        })()
        .map_err(|e| format!("ligne {}: {e}", n + 1))?;
        steps.push(step);
    }
    Ok(steps)
}

/// Exécute le scénario dans la boucle d'événements
pub fn run(handle: &LoopHandle<'static, State>, steps: Vec<Step>) {
    let mut steps = std::collections::VecDeque::from(steps);
    let mut waiting_since: Option<Instant> = None;
    let _ = handle.insert_source(
        Timer::from_duration(Duration::from_millis(100)),
        move |_, _, state| {
            let Some(step) = steps.front().cloned() else {
                return TimeoutAction::Drop;
            };
            let mut delay = 10;
            match step {
                Step::Spawn(cmd) => state.spawn(&cmd),
                Step::Wait(ms) => delay = ms,
                Step::WaitViews(n, timeout) => {
                    let start = *waiting_since.get_or_insert_with(Instant::now);
                    let mapped = state.views.iter().filter(|v| v.mapped).count();
                    if mapped < n && start.elapsed() < Duration::from_millis(timeout) {
                        return TimeoutAction::ToDuration(Duration::from_millis(50));
                    }
                    if mapped < n {
                        warn!("script: {mapped} fenêtres sur {n} après {timeout} ms");
                    }
                    waiting_since = None;
                }
                Step::Move(x, y) => {
                    state.pointer_motion((x, y).into(), state.start.elapsed().as_millis() as u32)
                }
                Step::Button(b, pressed) => {
                    state.pointer_button(b, pressed, state.start.elapsed().as_millis() as u32)
                }
                Step::Click(b) => {
                    let t = state.start.elapsed().as_millis() as u32;
                    state.pointer_button(b, true, t);
                    state.pointer_button(b, false, t + 1);
                }
                Step::Key(code) => {
                    let t = state.start.elapsed().as_millis() as u32;
                    state.key(Keycode::new(code + 8), true, t);
                    state.key(Keycode::new(code + 8), false, t + 1);
                }
                Step::Capture(path) => match state.backend.as_mut() {
                    Some(Backend::Headless(h)) => {
                        h.captures.push(path);
                        delay = 50;
                    }
                    _ => warn!("script: capture possible seulement avec --backend headless"),
                },
                Step::Quit => {
                    info!("script: fin");
                    state.signal.stop();
                }
            }
            steps.pop_front();
            TimeoutAction::ToDuration(Duration::from_millis(delay))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_script() {
        let s = parse("# essai\nspawn kitty -e sh\nwait 200\nwait-views 1\nmove 10 20.5\nclick left\ncapture /tmp/x.png\nquit\n").unwrap();
        assert_eq!(s[0], Step::Spawn("kitty -e sh".into()));
        assert_eq!(s[2], Step::WaitViews(1, 5000));
        assert_eq!(s[3], Step::Move(10.0, 20.5));
        assert_eq!(s[4], Step::Click(BTN_LEFT));
        assert_eq!(s[6], Step::Quit);
        assert!(parse("vole").is_err());
    }
}
