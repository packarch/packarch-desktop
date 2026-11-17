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
//! keydown 125            touche enfoncée (modificateur…), keyup pour la relâcher
//! combo 125 42 3         touches enfoncées dans l'ordre puis relâchées à l'envers
//! scroll down            un cran de molette (up, down, left, right)
//! reconfigure            comme SIGHUP
//! point un title         pointeur sur une partie de la vue de titre « un »:
//!                        title, client, close, max, iconify, br (coin bas
//!                        droit), left, right, top, bottom (milieu d'un bord)
//! expect-desktop 2       vérifie le bureau affiché (numéro à partir de 1)
//! expect-focus un        vérifie la vue qui a le clavier
//! expect un 10 38 640 724  vérifie le cadre d'une vue (x y largeur hauteur)
//! move-by 50 -20         déplace le pointeur
//! mark un                retient le cadre de la vue « un »
//! expect-moved un 50 -20 vérifie son déplacement depuis `mark`
//! expect-grown un 40 0   vérifie son changement de taille depuis `mark`
//! expect-shaded un yes   vérifie l'enroulement (yes/no)
//! expect-visible un no   vérifie l'affichage (réduite, autre bureau…)
//! expect-menu root-menu prefs   vérifie les menus ouverts (none: aucun)
//! wait-menu 2 5000       attend 2 menus ouverts (pipemenus, 5 s au plus)
//! point-menu Terminal    pointeur sur l'entrée de ce libellé
//! expect-file a.png 2000 attend qu'un fichier existe (2 s au plus)
//! expect-locked yes      vérifie le verrouillage de la session
//! expect-views 2         vérifie le nombre de vues affichées
//! clock-freeze           arrête l'horloge des animations
//! clock-advance 50       l'avance de 50 ms
//! clock-run              la remet en marche
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
    KeyState(u32, bool),
    Combo(Vec<u32>),
    Scroll(f64, f64),
    Reconfigure,
    Point(String, String),
    ExpectDesktop(usize),
    ExpectFocus(String),
    Expect(String, [i32; 4]),
    MoveBy(f64, f64),
    Mark(String),
    ExpectMoved(String, i32, i32),
    ExpectGrown(String, i32, i32),
    ExpectShaded(String, bool),
    ExpectVisible(String, bool),
    ExpectMenu(Vec<String>),
    WaitMenu(usize, u64),
    PointMenu(String),
    ExpectFile(PathBuf, u64),
    ExpectLocked(bool),
    ExpectViews(usize),
    ClockFreeze,
    ClockAdvance(u64),
    ClockRun,
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
                "keydown" => Step::KeyState(num(args.next())? as u32, true),
                "keyup" => Step::KeyState(num(args.next())? as u32, false),
                "combo" => Step::Combo(
                    args.map(|a| a.parse::<u32>().map_err(|e| e.to_string()))
                        .collect::<Result<_, _>>()?,
                ),
                "scroll" => match args.next() {
                    Some("up") => Step::Scroll(0.0, -1.0),
                    Some("down") => Step::Scroll(0.0, 1.0),
                    Some("left") => Step::Scroll(-1.0, 0.0),
                    Some("right") => Step::Scroll(1.0, 0.0),
                    d => return Err(format!("sens inconnu: {d:?}")),
                },
                "reconfigure" => Step::Reconfigure,
                "point" => {
                    let (title, part) = rest.rsplit_once(' ').ok_or("point TITRE PARTIE")?;
                    Step::Point(title.trim().to_string(), part.to_string())
                }
                "move-by" => Step::MoveBy(num(args.next())?, num(args.next())?),
                "mark" => Step::Mark(rest.to_string()),
                "expect-moved" | "expect-grown" | "expect-shaded" | "expect-visible" => {
                    let v: Vec<&str> = rest.split_whitespace().collect();
                    let n = v.len();
                    if cmd == "expect-moved" || cmd == "expect-grown" {
                        if n < 3 {
                            return Err(format!("{cmd} TITRE dx dy"));
                        }
                        let dx = v[n - 2].parse().map_err(|_| "dx")?;
                        let dy = v[n - 1].parse().map_err(|_| "dy")?;
                        let t = v[..n - 2].join(" ");
                        if cmd == "expect-moved" {
                            Step::ExpectMoved(t, dx, dy)
                        } else {
                            Step::ExpectGrown(t, dx, dy)
                        }
                    } else {
                        if n < 2 {
                            return Err(format!("{cmd} TITRE yes|no"));
                        }
                        let b = crate::config::parse_bool(v[n - 1]).ok_or("yes ou no")?;
                        let t = v[..n - 1].join(" ");
                        if cmd == "expect-shaded" {
                            Step::ExpectShaded(t, b)
                        } else {
                            Step::ExpectVisible(t, b)
                        }
                    }
                }
                "expect-desktop" => Step::ExpectDesktop(num(args.next())? as usize),
                "expect-focus" => Step::ExpectFocus(rest.to_string()),
                "expect" => {
                    let v: Vec<&str> = rest.split_whitespace().collect();
                    if v.len() < 5 {
                        return Err("expect TITRE x y largeur hauteur".into());
                    }
                    let n = v.len();
                    let nums: Vec<i32> = v[n - 4..]
                        .iter()
                        .map(|x| x.parse().map_err(|_| format!("nombre: {x}")))
                        .collect::<Result<_, _>>()?;
                    Step::Expect(v[..n - 4].join(" "), [nums[0], nums[1], nums[2], nums[3]])
                }
                "expect-menu" => Step::ExpectMenu(match rest {
                    "none" | "" => vec![],
                    _ => rest.split_whitespace().map(str::to_string).collect(),
                }),
                "wait-menu" => {
                    let n = num(args.next())? as usize;
                    let t = args.next().map(|s| s.parse().unwrap_or(5000)).unwrap_or(5000);
                    Step::WaitMenu(n, t)
                }
                "point-menu" => Step::PointMenu(rest.to_string()),
                "expect-file" => {
                    let path = args.next().ok_or("expect-file FICHIER [ms]")?;
                    let t = args.next().map(|s| s.parse().unwrap_or(3000)).unwrap_or(3000);
                    Step::ExpectFile(PathBuf::from(path), t)
                }
                "expect-views" => Step::ExpectViews(num(args.next())? as usize),
                "expect-locked" => Step::ExpectLocked(
                    crate::config::parse_bool(args.next().unwrap_or("")).ok_or("yes ou no")?,
                ),
                "clock-freeze" => Step::ClockFreeze,
                "clock-advance" => Step::ClockAdvance(num(args.next())? as u64),
                "clock-run" => Step::ClockRun,
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

/// Échec d'une vérification: message, et code de sortie 2 à la fin
fn fail(state: &mut State, msg: String) {
    warn!("script: ÉCHEC: {msg}");
    FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = state;
}

/// Une vérification a échoué
pub static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Position d'une partie d'une vue
fn point(
    state: &State,
    title: &str,
    part: &str,
) -> Option<smithay::utils::Point<f64, smithay::utils::Logical>> {
    use crate::config::Button;
    let i = state.views.iter().position(|v| v.title() == title)?;
    let layout = state.layout(&state.views[i]);
    let frame = state.frame_rect(i).loc.to_f64();
    let center = |r: smithay::utils::Rectangle<i32, smithay::utils::Logical>| {
        frame + r.loc.to_f64() + r.size.to_f64().to_point().downscale(2.0)
    };
    let button = |b: Button| {
        layout
            .buttons
            .iter()
            .find(|(x, _)| *x == b)
            .map(|(_, r)| center(*r))
    };
    match part {
        "title" => Some(center(layout.title)),
        "client" => Some(center(layout.client)),
        "close" => button(Button::Close),
        "max" => button(Button::Maximize),
        "iconify" => button(Button::Iconify),
        "br" => Some(frame + layout.size.to_f64().to_point() + smithay::utils::Point::from((2.0, 2.0))),
        "left" | "right" | "top" | "bottom" => {
            let (w, h) = (layout.size.w as f64, layout.size.h as f64);
            let p = match part {
                "left" => (-2.0, h / 2.0),
                "right" => (w + 1.0, h / 2.0),
                "top" => (w / 2.0, -2.0),
                _ => (w / 2.0, h + 1.0),
            };
            Some(frame + smithay::utils::Point::from(p))
        }
        _ => None,
    }
}

/// Exécute le scénario dans la boucle d'événements
pub fn run(handle: &LoopHandle<'static, State>, steps: Vec<Step>) {
    let mut steps = std::collections::VecDeque::from(steps);
    let mut waiting_since: Option<Instant> = None;
    let mut marks: Vec<(String, smithay::utils::Rectangle<i32, smithay::utils::Logical>)> = vec![];
    let _ = handle.insert_source(
        Timer::from_duration(Duration::from_millis(100)),
        move |_, _, state| {
            let Some(step) = steps.front().cloned() else {
                return TimeoutAction::Drop;
            };
            let mut delay = 10;
            match step.clone() {
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
                Step::KeyState(code, pressed) => {
                    let t = state.start.elapsed().as_millis() as u32;
                    state.key(Keycode::new(code + 8), pressed, t);
                }
                Step::Combo(codes) => {
                    let t = state.start.elapsed().as_millis() as u32;
                    for c in &codes {
                        state.key(Keycode::new(c + 8), true, t);
                    }
                    for c in codes.iter().rev() {
                        state.key(Keycode::new(c + 8), false, t + 1);
                    }
                }
                Step::Scroll(x, y) => {
                    use smithay::backend::input::{Axis, AxisSource};
                    let t = state.start.elapsed().as_millis() as u32;
                    let mut frame = smithay::input::pointer::AxisFrame::new(t).source(AxisSource::Wheel);
                    for (axis, v) in [(Axis::Horizontal, x), (Axis::Vertical, y)] {
                        if v != 0.0 {
                            frame = frame.value(axis, v * 15.0).v120(axis, (v * 120.0) as i32);
                        }
                    }
                    state.pointer_axis(frame, (x, y));
                }
                Step::Reconfigure => state.reconfigure(),
                Step::Point(title, part) => match point(state, &title, &part) {
                    Some(p) => state.pointer_motion(p, state.start.elapsed().as_millis() as u32),
                    None => fail(
                        state,
                        format!("point: pas de vue « {title} » ou partie « {part} »"),
                    ),
                },
                Step::MoveBy(dx, dy) => {
                    let p = state.pointer + smithay::utils::Point::from((dx, dy));
                    state.pointer_motion(p, state.start.elapsed().as_millis() as u32);
                }
                Step::Mark(title) => match state.views.iter().position(|v| v.title() == title) {
                    Some(i) => {
                        let f = state.frame_rect(i);
                        marks.retain(|(t, _)| *t != title);
                        marks.push((title, f));
                    }
                    None => fail(state, format!("pas de vue « {title} »")),
                },
                Step::ExpectMoved(title, dx, dy) => {
                    let i = state.views.iter().position(|v| v.title() == title);
                    let m = marks.iter().find(|(t, _)| *t == title).map(|(_, p)| p.loc);
                    match (i, m) {
                        (Some(i), Some(m)) => {
                            let d = state.frame_rect(i).loc - m;
                            if (d.x, d.y) != (dx, dy) {
                                fail(
                                    state,
                                    format!(
                                        "« {title} » déplacée de ({}, {}) au lieu de ({dx}, {dy})",
                                        d.x, d.y
                                    ),
                                );
                            }
                        }
                        _ => fail(state, format!("expect-moved: « {title} » sans mark")),
                    }
                }
                Step::ExpectGrown(title, dw, dh) => {
                    let i = state.views.iter().position(|v| v.title() == title);
                    let m = marks.iter().find(|(t, _)| *t == title).map(|(_, r)| r.size);
                    match (i, m) {
                        (Some(i), Some(m)) => {
                            let s = state.frame_rect(i).size;
                            if (s.w - m.w, s.h - m.h) != (dw, dh) {
                                fail(
                                    state,
                                    format!(
                                        "« {title} » agrandie de ({}, {}) au lieu de ({dw}, {dh})",
                                        s.w - m.w,
                                        s.h - m.h
                                    ),
                                );
                            }
                        }
                        _ => fail(state, format!("expect-grown: « {title} » sans mark")),
                    }
                }
                Step::ExpectShaded(title, b) | Step::ExpectVisible(title, b) => {
                    let shaded = matches!(step, Step::ExpectShaded(..));
                    match state.views.iter().position(|v| v.title() == title) {
                        Some(i) => {
                            let got = if shaded {
                                state.views[i].shaded
                            } else {
                                state.visible(&state.views[i])
                            };
                            if got != b {
                                let what = if shaded { "enroulée" } else { "affichée" };
                                fail(state, format!("« {title} » {what}: {got} au lieu de {b}"));
                            }
                        }
                        None => fail(state, format!("pas de vue « {title} »")),
                    }
                }
                Step::ExpectDesktop(d) => {
                    if state.desktop + 1 != d {
                        fail(state, format!("bureau {} au lieu de {d}", state.desktop + 1));
                    }
                }
                Step::ExpectFocus(title) => {
                    let got = state
                        .focused_view()
                        .map(|i| state.views[i].title())
                        .unwrap_or_default();
                    if got != title {
                        fail(state, format!("focus sur « {got} » au lieu de « {title} »"));
                    }
                }
                Step::Expect(title, [x, y, w, h]) => {
                    match state.views.iter().position(|v| v.title() == title) {
                        Some(i) => {
                            let f = state.frame_rect(i);
                            if [f.loc.x, f.loc.y, f.size.w, f.size.h] != [x, y, w, h] {
                                fail(
                                    state,
                                    format!("« {title} »: cadre {:?} au lieu de {:?}", f, [x, y, w, h]),
                                );
                            }
                        }
                        None => fail(state, format!("pas de vue « {title} »")),
                    }
                }
                Step::ExpectMenu(ids) => {
                    let got = state.open_menu_ids();
                    if got != ids {
                        fail(state, format!("menus ouverts {got:?} au lieu de {ids:?}"));
                    }
                }
                Step::WaitMenu(n, timeout) => {
                    let start = *waiting_since.get_or_insert_with(Instant::now);
                    let open = state.open_menu_ids().len();
                    if open < n && start.elapsed() < Duration::from_millis(timeout) {
                        return TimeoutAction::ToDuration(Duration::from_millis(20));
                    }
                    if open < n {
                        fail(state, format!("{open} menus ouverts sur {n} après {timeout} ms"));
                    }
                    waiting_since = None;
                }
                Step::PointMenu(label) => match state.menu_item_center(&label) {
                    Some(p) => state.pointer_motion(p, state.start.elapsed().as_millis() as u32),
                    None => fail(state, format!("pas d'entrée « {label} » dans les menus ouverts")),
                },
                Step::ExpectFile(path, timeout) => {
                    let start = *waiting_since.get_or_insert_with(Instant::now);
                    let ok = std::fs::metadata(&path).is_ok();
                    if !ok && start.elapsed() < Duration::from_millis(timeout) {
                        return TimeoutAction::ToDuration(Duration::from_millis(50));
                    }
                    if !ok {
                        fail(state, format!("fichier absent: {}", path.display()));
                    }
                    waiting_since = None;
                }
                Step::ExpectViews(n) => {
                    let got = state.views.iter().filter(|v| v.mapped).count();
                    if got != n {
                        fail(state, format!("{got} vues affichées au lieu de {n}"));
                    }
                }
                Step::ExpectLocked(b) => {
                    if state.is_locked() != b {
                        fail(
                            state,
                            format!("verrouillée: {} au lieu de {b}", state.is_locked()),
                        );
                    }
                }
                Step::ClockFreeze => state.clock.frozen = Some(state.now()),
                Step::ClockAdvance(ms) => {
                    let now = state.now();
                    state.clock.frozen = Some(now + Duration::from_millis(ms));
                }
                Step::ClockRun => {
                    let start = state.start;
                    state.clock.run(start);
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
