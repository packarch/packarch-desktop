// SPDX-License-Identifier: MIT OR Apache-2.0
//! Menus ouverts (§ 5.5, § 10.4): position, survol, sous-menus (ouverts au
//! survol après 100 ms), clavier, pipemenus asynchrones, menus fournis par
//! le compositeur (menu de fenêtre, liste des fenêtres).

use std::io::Read;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::desktop::Window;
use smithay::input::keyboard::{Keysym, keysyms};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
    RelativeMotionEvent,
};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER, Size, Transform};
use tracing::{debug, info, warn};

use super::draw::{self, MenuLayout};
use super::{CLIENT, CLIENT_LIST, Entry, Label, Menu, Menus};
use crate::actions::{Action, DesktopTarget};
use crate::anim::{Anim, Kind};
use crate::state::State;
use crate::view::{Axis, Stack, Tiling};
use crate::wm::Origin;

/// Délai d'ouverture d'un sous-menu au survol
const HOVER_DELAY: Duration = Duration::from_millis(100);
/// Délai limite d'un pipemenu
const PIPE_TIMEOUT: Duration = Duration::from_secs(5);
/// Glissement des menus à l'ouverture et à la fermeture (px logiques)
pub const SLIDE: f64 = 4.0;

/// Image d'un menu, refaite quand la sélection ou l'échelle change
pub struct MenuImage {
    pub buffer: MemoryRenderBuffer,
    /// Taille en pixels physiques
    pub px: Size<i32, Logical>,
    selected: Option<usize>,
    scale: u64,
}

/// Un menu ouvert (le menu racine, puis ses sous-menus)
pub struct Level {
    pub id: String,
    pub entries: Vec<Entry>,
    pub layout: MenuLayout,
    /// Coin haut gauche, coordonnées globales
    pub loc: Point<i32, Logical>,
    pub selected: Option<usize>,
    /// Entrée du menu parent qui a ouvert celui-ci
    pub parent_item: Option<usize>,
    pub image: Option<MenuImage>,
    pub anim: Option<Anim>,
}

impl Level {
    pub fn rect(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.loc, self.layout.size)
    }
}

/// Menu qui disparaît (fondu)
pub struct MenuGhost {
    pub image: MenuImage,
    pub rect: Rectangle<i32, Logical>,
    pub anim: Anim,
}

/// Pipemenu en cours d'exécution
struct Pending {
    token: u64,
    id: String,
    /// Niveau à ouvrir (0: le menu racine)
    level: usize,
    /// Entrée du niveau parent
    item: Option<usize>,
    at: Point<i32, Logical>,
    select_first: bool,
}

pub struct MenuState {
    pub levels: Vec<Level>,
    /// Menus de `menu.xml`, menus générés et contenus des pipemenus
    pub store: Menus,
    /// Vue visée par les actions (menu de fenêtre)
    pub target: Option<Window>,
    opened_at: Point<f64, Logical>,
    /// Le pointeur a bougé depuis l'ouverture
    moved: bool,
    /// Ouvert par l'appui d'un bouton: cet appui et sa relâche (sans
    /// mouvement) ne choisissent rien
    skip_press: bool,
    skip_release: bool,
    hover_gen: u64,
    hover_target: Option<(usize, usize)>,
    pending: Option<Pending>,
}

/// Résultat d'un clic dans un menu
enum Click {
    Nothing,
    Close,
    Run(Vec<Action>, Option<Window>),
}

fn empty_entry() -> Entry {
    Entry::Item {
        label: Label::plain("(vide)"),
        icon: None,
        actions: vec![],
        target: None,
        checked: false,
        enabled: false,
    }
}

fn check(label: &str, checked: bool, action: Action) -> Entry {
    Entry::Item {
        label: Label::parse(label),
        icon: None,
        actions: vec![action],
        target: None,
        checked,
        enabled: true,
    }
}

/// Exécute la commande d'un pipemenu, 5 s au plus
fn run_pipe(mut cmd: std::process::Command) -> Option<String> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            warn!("pipemenu: {e}");
            return None;
        }
    };
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = vec![];
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    match rx.recv_timeout(PIPE_TIMEOUT) {
        Ok(buf) => {
            let _ = child.wait();
            Some(String::from_utf8_lossy(&buf).into_owned())
        }
        Err(_) => {
            warn!("pipemenu: délai de 5 s dépassé");
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

impl State {
    /// Ouvre un menu (action ShowMenu); `target`: vue visée
    pub fn show_menu(&mut self, id: &str, at_cursor: bool, target: Option<usize>) {
        self.close_menus();
        let mut store = self.menus.clone();
        if id == CLIENT && !store.map.contains_key(CLIENT) {
            let Some(i) = target else { return };
            for m in self.client_menu(i) {
                store.map.insert(m.id.clone(), m);
            }
        }
        if id == CLIENT_LIST && !store.map.contains_key(CLIENT_LIST) {
            let m = self.client_list_menu();
            store.map.insert(m.id.clone(), m);
        }
        let Some(menu) = store.get(id) else {
            warn!("menu inconnu: {id}");
            return;
        };
        let execute = menu.execute.clone();
        // atCursor="no": sous la barre de titre de la vue
        let at = match target.filter(|_| !at_cursor) {
            Some(i) => {
                let layout = self.layout(&self.views[i]);
                let frame = self.frame_rect(i);
                frame.loc + Point::from((0, layout.border + layout.title_height))
            }
            None => self.pointer.to_i32_round(),
        };
        let held = self.press.as_ref().is_some_and(|p| !p.intercepted && !p.dragged);
        self.menu = Some(MenuState {
            levels: vec![],
            store,
            target: target.map(|i| self.views[i].window.clone()),
            opened_at: self.pointer,
            moved: false,
            skip_press: held,
            skip_release: held,
            hover_gen: 0,
            hover_target: None,
            pending: None,
        });
        debug!("menu « {id} »");
        if let Some(pointer) = self.seat.get_pointer() {
            let grab = MenuGrab {
                start: GrabStartData {
                    focus: None,
                    button: 0,
                    location: self.pointer,
                },
            };
            pointer.set_grab(
                self,
                grab,
                SERIAL_COUNTER.next_serial(),
                smithay::input::pointer::Focus::Clear,
            );
        }
        match execute {
            Some(cmd) => self.start_pipe(id, 0, None, at, cmd, false),
            None => self.open_level(id, None, at),
        }
    }

    /// Ferme tous les menus et rend le pointeur aux clients
    pub fn close_menus(&mut self) {
        if self.menu.is_none() {
            return;
        }
        self.menu_dismiss();
        if let Some(pointer) = self.seat.get_pointer()
            && pointer.is_grabbed()
        {
            let time = self.start.elapsed().as_millis() as u32;
            pointer.unset_grab(self, SERIAL_COUNTER.next_serial(), time);
        }
    }

    /// Ferme les menus sans toucher au pointeur (depuis le geste lui-même)
    fn menu_dismiss(&mut self) {
        if let Some(ms) = self.menu.take() {
            let levels: Vec<Level> = ms.levels;
            for l in levels.into_iter().rev() {
                self.menu_ghost(l);
            }
        }
    }

    fn menu_ghost(&mut self, level: Level) {
        let now = self.now();
        let rect = level.rect();
        if let (Some(image), Some(anim)) = (level.image, self.config.animations.start(Kind::Menu, now)) {
            // Fermé pendant l'ouverture: le fondu part de l'état affiché
            let shown = level.anim.map(|a| a.value(now)).unwrap_or(1.0);
            self.menu_ghosts.push(MenuGhost {
                image,
                rect,
                anim: anim.resumed_from(1.0 - shown),
            });
        }
    }

    /// Ferme les niveaux à partir de `from`
    fn close_levels(&mut self, from: usize) {
        let Some(ms) = self.menu.as_mut() else { return };
        if from >= ms.levels.len() {
            return;
        }
        let removed: Vec<Level> = ms.levels.drain(from..).collect();
        for l in removed.into_iter().rev() {
            self.menu_ghost(l);
        }
    }

    /// Affiche un menu de la réserve: le menu racine à `at`, ou un
    /// sous-menu de l'entrée `parent` du dernier niveau
    fn open_level(&mut self, id: &str, parent: Option<usize>, at: Point<i32, Logical>) {
        let screen = self
            .output_at(at.to_f64())
            .map(|o| self.output_geometry(&o))
            .unwrap_or_else(|| Rectangle::from_size((1, 1).into()));
        let now = self.now();
        let anim = self.config.animations.start(Kind::Menu, now);
        let Some(ms) = self.menu.as_mut() else { return };
        let Some(menu) = ms.store.get(id) else { return };
        let mut entries = menu.entries.clone();
        if entries.is_empty() {
            entries.push(empty_entry());
        }
        let layout = draw::layout(&entries, &ms.store, &self.config, &self.theme, &mut self.text);
        let t = &self.theme.menu;
        let (w, h) = (layout.size.w, layout.size.h);
        let right = screen.loc.x + screen.size.w;
        let bottom = screen.loc.y + screen.size.h;
        let loc = match (parent, ms.levels.last()) {
            (Some(item), Some(p)) => {
                let row = &p.layout.rows[item];
                let mut x = p.loc.x + p.layout.size.w - t.overlap_x;
                if x + w > right {
                    x = p.loc.x - w + t.overlap_x;
                }
                let y = p.loc.y + row.y - t.border_width + t.overlap_y;
                let y = y.min(bottom - h);
                Point::from((x.max(screen.loc.x), y.max(screen.loc.y)))
            }
            _ => Point::from((
                at.x.min(right - w).max(screen.loc.x),
                at.y.min(bottom - h).max(screen.loc.y),
            )),
        };
        ms.levels.push(Level {
            id: id.to_string(),
            entries,
            layout,
            loc,
            selected: None,
            parent_item: parent,
            image: None,
            anim,
        });
    }

    /// Lance un pipemenu; le menu s'ouvre à l'arrivée du résultat
    fn start_pipe(
        &mut self,
        id: &str,
        level: usize,
        item: Option<usize>,
        at: Point<i32, Logical>,
        cmd: String,
        select_first: bool,
    ) {
        self.pipe_counter += 1;
        let token = self.pipe_counter;
        let Some(ms) = self.menu.as_mut() else { return };
        ms.pending = Some(Pending {
            token,
            id: id.to_string(),
            level,
            item,
            at,
            select_first,
        });
        debug!("pipemenu « {id} »: {cmd}");
        let command = self.command(&cmd);
        let tx = self.pipe_tx.clone();
        std::thread::spawn(move || {
            let out = run_pipe(command);
            let _ = tx.send((token, out));
        });
    }

    /// Résultat d'un pipemenu
    pub fn pipe_done(&mut self, token: u64, out: Option<String>) {
        let Some(ms) = self.menu.as_mut() else { return };
        let Some(p) = ms.pending.take_if(|p| p.token == token) else {
            return;
        };
        let prefix = format!("{}/", p.id);
        ms.store.map.retain(|k, _| !k.starts_with(&prefix));
        let mut warnings = vec![];
        let entries = match out {
            Some(xml) => match ms.store.parse(&xml, &prefix, &mut warnings) {
                Ok(e) => e,
                Err(e) => {
                    warn!("pipemenu « {} »: {e}", p.id);
                    vec![]
                }
            },
            None => vec![],
        };
        for w in warnings {
            debug!("pipemenu « {} », {w}", p.id);
        }
        if let Some(m) = ms.store.map.get_mut(&p.id) {
            m.entries = entries;
        }
        // Toujours voulu: le parent montre encore cette entrée
        let wanted = match p.item {
            None => ms.levels.is_empty(),
            Some(item) => ms.levels.len() == p.level && ms.levels[p.level - 1].selected == Some(item),
        };
        if wanted {
            self.open_level(&p.id, p.item, p.at);
            if p.select_first {
                self.menu_select_first();
            }
        }
    }

    /// Ouvre le sous-menu de l'entrée `item` du niveau `level`
    fn open_submenu(&mut self, level: usize, item: usize, select_first: bool) {
        let Some(ms) = self.menu.as_ref() else { return };
        let Some(Entry::Menu(id)) = ms.levels.get(level).and_then(|l| l.entries.get(item)) else {
            return;
        };
        let id = id.clone();
        let Some(menu) = ms.store.get(&id) else { return };
        let execute = menu.execute.clone();
        self.close_levels(level + 1);
        match execute {
            Some(cmd) => {
                let at = self.menu.as_ref().unwrap().levels[level].loc;
                self.start_pipe(&id, level + 1, Some(item), at, cmd, select_first);
            }
            None => {
                self.open_level(&id, Some(item), Point::default());
                if select_first {
                    self.menu_select_first();
                }
            }
        }
    }

    fn menu_select_first(&mut self) {
        let Some(ms) = self.menu.as_mut() else { return };
        let store = &ms.store;
        if let Some(l) = ms.levels.last_mut() {
            l.selected = l.entries.iter().position(|e| e.is_selectable(store));
        }
    }

    /// Le pointeur survole les menus
    fn menu_hover(&mut self, p: Point<f64, Logical>) {
        let Some(ms) = self.menu.as_mut() else { return };
        let d = p - ms.opened_at;
        if d.x.hypot(d.y) > 3.0 {
            ms.moved = true;
        }
        let hit = ms.levels.iter().enumerate().rev().find_map(|(l, lv)| {
            let rel = p - lv.loc.to_f64();
            lv.rect()
                .to_f64()
                .contains(p)
                .then(|| (l, lv.layout.row_at(rel.x, rel.y)))
        });
        let Some((l, Some(j))) = hit else { return };
        if !ms.levels[l].entries[j].is_selectable(&ms.store) {
            return;
        }
        ms.levels[l].selected = Some(j);
        if ms.hover_target == Some((l, j)) {
            return;
        }
        ms.hover_target = Some((l, j));
        ms.hover_gen += 1;
        // Rien à changer: le sous-menu de cette entrée est déjà ouvert, ou
        // c'est une entrée simple du dernier niveau
        let is_menu = matches!(ms.levels[l].entries[j], Entry::Menu(_));
        let consistent = match ms.levels.get(l + 1) {
            Some(next) => next.parent_item == Some(j),
            None => !is_menu && ms.pending.is_none(),
        };
        if consistent {
            return;
        }
        let generation = ms.hover_gen;
        let _ = self
            .handle
            .insert_source(Timer::from_duration(HOVER_DELAY), move |_, _, state| {
                state.menu_hover_commit(generation, l, j);
                TimeoutAction::Drop
            });
    }

    fn menu_hover_commit(&mut self, generation: u64, l: usize, j: usize) {
        let Some(ms) = self.menu.as_mut() else { return };
        if ms.hover_gen != generation || ms.levels.get(l).is_none_or(|lv| lv.selected != Some(j)) {
            return;
        }
        if ms.levels.get(l + 1).is_some_and(|n| n.parent_item == Some(j)) {
            return;
        }
        ms.pending = None;
        self.close_levels(l + 1);
        self.open_submenu(l, j, false);
    }

    /// Clic dans le geste des menus
    fn menu_button(&mut self, pressed: bool) -> Click {
        let Some(ms) = self.menu.as_mut() else {
            return Click::Close;
        };
        let p = self.pointer;
        let hit = ms.levels.iter().enumerate().rev().find_map(|(l, lv)| {
            let rel = p - lv.loc.to_f64();
            lv.rect()
                .to_f64()
                .contains(p)
                .then(|| (l, lv.layout.row_at(rel.x, rel.y)))
        });
        if pressed {
            if std::mem::take(&mut ms.skip_press) {
                return Click::Nothing;
            }
            ms.skip_release = false;
            // Hors des menus: fermés, le clic n'est pas transmis
            return if hit.is_none() {
                Click::Close
            } else {
                Click::Nothing
            };
        }
        if std::mem::take(&mut ms.skip_release) && !ms.moved {
            return Click::Nothing;
        }
        let Some((l, Some(j))) = hit else {
            return Click::Nothing;
        };
        self.menu_activate(l, j)
    }

    /// Choisit l'entrée `j` du niveau `l`
    fn menu_activate(&mut self, l: usize, j: usize) -> Click {
        let Some(ms) = self.menu.as_mut() else {
            return Click::Nothing;
        };
        let Some(entry) = ms.levels.get(l).and_then(|lv| lv.entries.get(j)).cloned() else {
            return Click::Nothing;
        };
        match entry {
            Entry::Item {
                actions,
                target,
                enabled: true,
                ..
            } => Click::Run(actions, target.or_else(|| ms.target.clone())),
            Entry::Menu(_) => {
                ms.levels[l].selected = Some(j);
                ms.hover_gen += 1;
                ms.hover_target = Some((l, j));
                if !ms.levels.get(l + 1).is_some_and(|n| n.parent_item == Some(j)) {
                    self.open_submenu(l, j, false);
                }
                Click::Nothing
            }
            _ => Click::Nothing,
        }
    }

    /// Touche pendant qu'un menu est ouvert; `false`: pas pour le menu
    pub fn menu_key(&mut self, sym: Keysym, ch: Option<char>) -> bool {
        enum Do {
            Nothing,
            Close,
            Select(usize),
            Back,
            Open(usize),
            Choose(usize),
        }
        let Some(ms) = self.menu.as_ref() else {
            return false;
        };
        let raw = sym.raw();
        let Some(last) = ms.levels.len().checked_sub(1) else {
            if raw == keysyms::KEY_Escape {
                self.close_menus();
            }
            return true;
        };
        let level = &ms.levels[last];
        let selectable: Vec<usize> = (0..level.entries.len())
            .filter(|&i| level.entries[i].is_selectable(&ms.store))
            .collect();
        let pos = level
            .selected
            .and_then(|s| selectable.iter().position(|&i| i == s));
        let is_menu = |j: usize| matches!(level.entries[j], Entry::Menu(_));
        let action = match raw {
            keysyms::KEY_Escape => Do::Close,
            keysyms::KEY_Down | keysyms::KEY_KP_Down | keysyms::KEY_Up | keysyms::KEY_KP_Up => {
                let n = selectable.len();
                let down = matches!(raw, keysyms::KEY_Down | keysyms::KEY_KP_Down);
                match (n, pos, down) {
                    (0, ..) => Do::Nothing,
                    (_, None, true) => Do::Select(selectable[0]),
                    (_, None, false) => Do::Select(selectable[n - 1]),
                    (_, Some(p), true) => Do::Select(selectable[(p + 1) % n]),
                    (_, Some(p), false) => Do::Select(selectable[(p + n - 1) % n]),
                }
            }
            keysyms::KEY_Home => selectable.first().map_or(Do::Nothing, |&j| Do::Select(j)),
            keysyms::KEY_End => selectable.last().map_or(Do::Nothing, |&j| Do::Select(j)),
            keysyms::KEY_Left | keysyms::KEY_KP_Left => {
                if last > 0 {
                    Do::Back
                } else {
                    Do::Nothing
                }
            }
            keysyms::KEY_Right
            | keysyms::KEY_KP_Right
            | keysyms::KEY_Return
            | keysyms::KEY_KP_Enter
            | keysyms::KEY_space => {
                let right = matches!(raw, keysyms::KEY_Right | keysyms::KEY_KP_Right);
                match level.selected {
                    Some(j) if is_menu(j) => Do::Open(j),
                    Some(j) if !right => Do::Choose(j),
                    _ => Do::Nothing,
                }
            }
            _ => {
                // Lettre d'accélérateur: une seule entrée, choisie; plusieurs,
                // sélection de la suivante
                let c = ch.and_then(|c| c.to_lowercase().next());
                let matching: Vec<usize> = selectable
                    .iter()
                    .copied()
                    .filter(|&i| {
                        let label = match &level.entries[i] {
                            Entry::Item { label, .. } => Some(label),
                            Entry::Menu(id) => ms.store.get(id).map(|m| &m.label),
                            Entry::Separator(_) => None,
                        };
                        c.is_some() && label.and_then(|l| l.accel()) == c
                    })
                    .collect();
                match matching.len() {
                    0 => Do::Nothing,
                    1 if is_menu(matching[0]) => Do::Open(matching[0]),
                    1 => Do::Choose(matching[0]),
                    _ => Do::Select(
                        matching
                            .iter()
                            .copied()
                            .find(|&i| level.selected.is_none_or(|s| i > s))
                            .unwrap_or(matching[0]),
                    ),
                }
            }
        };
        match action {
            Do::Nothing => {}
            Do::Close => self.close_menus(),
            Do::Select(j) => {
                let ms = self.menu.as_mut().unwrap();
                ms.levels[last].selected = Some(j);
                ms.hover_target = None;
                ms.hover_gen += 1;
            }
            Do::Back => {
                let ms = self.menu.as_mut().unwrap();
                ms.pending = None;
                ms.hover_target = None;
                ms.hover_gen += 1;
                self.close_levels(last);
            }
            Do::Open(j) => {
                let ms = self.menu.as_mut().unwrap();
                ms.levels[last].selected = Some(j);
                ms.hover_target = Some((last, j));
                ms.hover_gen += 1;
                self.open_submenu(last, j, true);
            }
            Do::Choose(j) => {
                self.menu.as_mut().unwrap().levels[last].selected = Some(j);
                self.menu_choose(last, j);
            }
        }
        true
    }

    /// Entrée choisie au clavier: menus fermés, puis ses actions
    fn menu_choose(&mut self, l: usize, j: usize) {
        if let Click::Run(actions, target) = self.menu_activate(l, j) {
            self.close_menus();
            let origin = Origin {
                button: None,
                location: self.pointer,
            };
            self.run_actions(&actions, target, origin);
        }
    }

    /// Images des menus ouverts, à l'échelle de l'écran
    pub fn menu_images(&mut self, scale: f64) {
        let State {
            menu,
            config,
            theme,
            text,
            icons,
            ..
        } = self;
        let Some(ms) = menu.as_mut() else { return };
        for level in &mut ms.levels {
            if level
                .image
                .as_ref()
                .is_some_and(|img| img.selected == level.selected && img.scale == scale.to_bits())
            {
                continue;
            }
            let d = draw::Draw {
                entries: &level.entries,
                menus: &ms.store,
                layout: &level.layout,
                selected: level.selected,
                scale,
            };
            let Some(pm) = draw::draw(&d, config, theme, text, icons) else {
                continue;
            };
            let (w, h) = (pm.width() as i32, pm.height() as i32);
            let buffer = MemoryRenderBuffer::from_slice(
                pm.data(),
                Fourcc::Abgr8888,
                (w, h),
                1,
                Transform::Normal,
                None,
            );
            level.image = Some(MenuImage {
                buffer,
                px: Size::from((w, h)),
                selected: level.selected,
                scale: scale.to_bits(),
            });
        }
    }

    /// Menu de fenêtre (§ 5.5) et son sous-menu des bureaux
    fn client_menu(&self, i: usize) -> Vec<Menu> {
        let v = &self.views[i];
        let mut desks: Vec<Entry> = self
            .config
            .desktop_names
            .iter()
            .enumerate()
            .map(|(d, name)| {
                let mut e = check(
                    name,
                    !v.omnipresent && v.desktop == d,
                    Action::SendToDesktop {
                        to: DesktopTarget::Index(d),
                        follow: false,
                        wrap: false,
                    },
                );
                if let Entry::Item { label, .. } = &mut e {
                    *label = Label::plain(name);
                }
                e
            })
            .collect();
        desks.push(Entry::Separator(None));
        desks.push(check(
            "_Tous les bureaux",
            v.omnipresent,
            Action::ToggleOmnipresent,
        ));
        let maximized = matches!(v.tiling, Tiling::Maximized(Axis::Both));
        let entries = vec![
            Entry::Menu(format!("{CLIENT}/desktops")),
            Entry::Separator(None),
            Entry::item("_Réduire", vec![Action::Iconify]),
            check("_Maximiser", maximized, Action::ToggleMaximize(Axis::Both)),
            check(
                "_Plein écran",
                v.tiling == Tiling::Fullscreen,
                Action::ToggleFullscreen,
            ),
            check("_Enrouler", v.shaded, Action::ToggleShade),
            check("Déc_orations", v.decorated, Action::ToggleDecorations),
            check(
                "Toujours au-dess_us",
                v.stack == Stack::Top,
                Action::ToggleAlwaysOnTop,
            ),
            check(
                "Toujours en dessou_s",
                v.stack == Stack::Bottom,
                Action::ToggleAlwaysOnBottom,
            ),
            Entry::Separator(None),
            Entry::item("_Déplacer", vec![Action::Move]),
            Entry::item("Redime_nsionner", vec![Action::Resize]),
            Entry::Separator(None),
            Entry::item("_Fermer", vec![Action::Close]),
        ];
        vec![
            Menu {
                id: CLIENT.into(),
                label: Label::plain("Fenêtre"),
                icon: None,
                execute: None,
                entries,
            },
            Menu {
                id: format!("{CLIENT}/desktops"),
                label: Label::parse("_Bureaux"),
                icon: None,
                execute: None,
                entries: desks,
            },
        ]
    }

    /// Toutes les fenêtres, par bureau (§ 5.5)
    fn client_list_menu(&mut self) -> Menu {
        let mut entries = vec![];
        let names = self.config.desktop_names.clone();
        for (d, name) in names.iter().enumerate() {
            entries.push(Entry::Separator(Some(name.clone())));
            let mut any = false;
            let ids: Vec<usize> = (0..self.views.len())
                .filter(|&i| self.views[i].mapped && self.views[i].desktop == d)
                .collect();
            for i in ids {
                let icon = self.apps.get(&self.views[i].app_id()).and_then(|a| a.icon);
                let v = &self.views[i];
                let title = v.title();
                let title = if title.is_empty() { v.app_id() } else { title };
                let title = if v.minimized { format!("({title})") } else { title };
                let app_id = v.app_id();
                entries.push(Entry::Item {
                    label: Label::plain(&title),
                    icon: icon.or_else(|| (!app_id.is_empty()).then_some(app_id)),
                    actions: vec![Action::Focus, Action::Raise],
                    target: Some(v.window.clone()),
                    checked: false,
                    enabled: true,
                });
                any = true;
            }
            if !any {
                entries.push(Entry::Item {
                    label: Label::plain("—"),
                    icon: None,
                    actions: vec![],
                    target: None,
                    checked: false,
                    enabled: false,
                });
            }
        }
        Menu {
            id: CLIENT_LIST.into(),
            label: Label::plain("Fenêtres"),
            icon: None,
            execute: None,
            entries,
        }
    }

    /// Recharge `menu.xml` (§ 5.7); une erreur garde les anciens menus
    pub fn reload_menus(&mut self) {
        match Menus::load(&self.config.dir) {
            Ok(m) => self.menus = m,
            Err(e) => warn!("menus gardés: {e}"),
        }
        self.icons.set_theme(&self.config.icon_theme);
        self.icons.clear();
        info!("{} menus", self.menus.map.len());
    }

    // --- Pour les scénarios d'essai -----------------------------------

    /// Identifiants des menus ouverts, du racine au plus profond
    pub fn open_menu_ids(&self) -> Vec<String> {
        self.menu
            .as_ref()
            .map(|m| m.levels.iter().map(|l| l.id.clone()).collect())
            .unwrap_or_default()
    }

    /// Centre de l'entrée de libellé `label` dans les menus ouverts
    pub fn menu_item_center(&self, label: &str) -> Option<Point<f64, Logical>> {
        let ms = self.menu.as_ref()?;
        for lv in ms.levels.iter().rev() {
            for (e, row) in lv.entries.iter().zip(&lv.layout.rows) {
                if draw::entry_label(e, &ms.store) == label {
                    return Some(Point::from((
                        lv.loc.x as f64 + lv.layout.size.w as f64 / 2.0,
                        (lv.loc.y + row.y) as f64 + row.h as f64 / 2.0,
                    )));
                }
            }
        }
        None
    }
}

/// Geste du pointeur pendant qu'un menu est ouvert: rien n'est transmis aux
/// clients
pub struct MenuGrab {
    start: GrabStartData<State>,
}

impl PointerGrab<State> for MenuGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        data.pointer = event.location;
        data.menu_hover(event.location);
    }

    fn relative_motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, None, event);
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        let pressed = event.state == smithay::backend::input::ButtonState::Pressed;
        match data.menu_button(pressed) {
            Click::Nothing => {}
            Click::Close => {
                data.menu_dismiss();
                handle.unset_grab(self, data, event.serial, event.time, true);
            }
            Click::Run(actions, target) => {
                data.menu_dismiss();
                handle.unset_grab(self, data, event.serial, event.time, true);
                let origin = Origin {
                    button: None,
                    location: data.pointer,
                };
                // Après le geste: les actions peuvent en commencer un autre
                data.handle.insert_idle(move |state| {
                    state.run_actions(&actions, target, origin);
                });
            }
        }
    }

    fn axis(&mut self, _data: &mut State, _handle: &mut PointerInnerHandle<'_, State>, _details: AxisFrame) {}

    fn frame(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        d: &mut State,
        h: &mut PointerInnerHandle<'_, State>,
        e: &GestureSwipeBeginEvent,
    ) {
        h.gesture_swipe_begin(d, e)
    }
    fn gesture_swipe_update(
        &mut self,
        d: &mut State,
        h: &mut PointerInnerHandle<'_, State>,
        e: &GestureSwipeUpdateEvent,
    ) {
        h.gesture_swipe_update(d, e)
    }
    fn gesture_swipe_end(
        &mut self,
        d: &mut State,
        h: &mut PointerInnerHandle<'_, State>,
        e: &GestureSwipeEndEvent,
    ) {
        h.gesture_swipe_end(d, e)
    }
    fn gesture_pinch_begin(
        &mut self,
        d: &mut State,
        h: &mut PointerInnerHandle<'_, State>,
        e: &GesturePinchBeginEvent,
    ) {
        h.gesture_pinch_begin(d, e)
    }
    fn gesture_pinch_update(
        &mut self,
        d: &mut State,
        h: &mut PointerInnerHandle<'_, State>,
        e: &GesturePinchUpdateEvent,
    ) {
        h.gesture_pinch_update(d, e)
    }
    fn gesture_pinch_end(
        &mut self,
        d: &mut State,
        h: &mut PointerInnerHandle<'_, State>,
        e: &GesturePinchEndEvent,
    ) {
        h.gesture_pinch_end(d, e)
    }
    fn gesture_hold_begin(
        &mut self,
        d: &mut State,
        h: &mut PointerInnerHandle<'_, State>,
        e: &GestureHoldBeginEvent,
    ) {
        h.gesture_hold_begin(d, e)
    }
    fn gesture_hold_end(
        &mut self,
        d: &mut State,
        h: &mut PointerInnerHandle<'_, State>,
        e: &GestureHoldEndEvent,
    ) {
        h.gesture_hold_end(d, e)
    }

    fn start_data(&self) -> &GrabStartData<State> {
        &self.start
    }

    fn unset(&mut self, data: &mut State) {
        data.menu_dismiss();
    }
}
