// SPDX-License-Identifier: MIT OR Apache-2.0
//! Entrées (cahier des charges § 5.3, § 5.4, § 8.6, § 8.7): raccourcis
//! clavier, liaisons de la souris, déplacement et redimensionnement.

use std::time::Instant;

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, GestureBeginEvent, GestureEndEvent,
    GesturePinchUpdateEvent as _, GestureSwipeUpdateEvent as _, InputBackend, InputEvent, KeyState,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::desktop::Window;
use smithay::input::keyboard::{FilterResult, Keycode, Keysym, keysyms};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, CursorIcon, CursorImageStatus, GestureHoldBeginEvent, GestureHoldEndEvent,
    GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab,
    PointerInnerHandle, RelativeMotionEvent,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Size};

use crate::actions::Action;
use crate::bindings::{Context, Mods, MouseButton, MouseEvent, ScrollDir, find_keybind};
use crate::config::Button;
use crate::deco::{Edges, Part};
use crate::state::{Press, State, Under};
use crate::view::{Snap, Tiling};
use crate::wm::{Origin, resist, snap_edge, snap_rect};

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

/// Distance à partir de laquelle un appui devient un glissement
const DRAG_THRESHOLD: f64 = 3.0;

/// Contextes de la souris d'un point, du plus précis au plus général, et la
/// vue concernée
fn contexts(under: &Under) -> (Vec<Context>, Option<usize>) {
    use Context as C;
    match under {
        Under::View(i, ..) => (vec![C::Client, C::Frame], Some(*i)),
        Under::Frame(i, Part::Title) => (vec![C::Title, C::TitleBar, C::Frame], Some(*i)),
        Under::Frame(i, Part::Button(b)) => {
            let c = match b {
                Button::Close => C::Close,
                Button::Maximize => C::Maximize,
                Button::Iconify => C::Iconify,
                Button::Shade => C::Shade,
                Button::Desk => C::AllDesktops,
            };
            (vec![c, C::TitleBar, C::Frame], Some(*i))
        }
        Under::Frame(i, Part::Border(e)) => {
            let specific = match (e.top, e.bottom, e.left, e.right) {
                (true, _, true, _) => C::TLCorner,
                (true, _, _, true) => C::TRCorner,
                (_, true, true, _) => C::BLCorner,
                (_, true, _, true) => C::BRCorner,
                (true, ..) => C::Top,
                (_, true, ..) => C::Bottom,
                (_, _, true, _) => C::Left,
                _ => C::Right,
            };
            (vec![specific, C::Border, C::Frame], Some(*i))
        }
        Under::Layer(_, _, true) | Under::Root => (vec![C::Root], None),
        Under::Layer(_, _, false) => (vec![], None),
    }
}

/// Résultat du filtre du clavier
enum KeyResult {
    Bind(Vec<Action>),
    /// Touche pour le menu ouvert, et le raccourci qu'elle déclencherait
    Menu(Keysym, Option<char>, Option<Vec<Action>>),
    CancelCycle,
    /// Ctrl+Alt+Fn: console n
    Vt(i32),
}

/// Curseur d'un redimensionnement par ces bords
pub fn resize_cursor(e: Edges) -> CursorIcon {
    match (e.top, e.bottom, e.left, e.right) {
        (true, _, true, _) => CursorIcon::NwResize,
        (true, _, _, true) => CursorIcon::NeResize,
        (_, true, true, _) => CursorIcon::SwResize,
        (_, true, _, true) => CursorIcon::SeResize,
        (true, ..) => CursorIcon::NResize,
        (_, true, ..) => CursorIcon::SResize,
        (_, _, true, _) => CursorIcon::WResize,
        _ => CursorIcon::EResize,
    }
}

fn is_modifier(sym: Keysym) -> bool {
    use smithay::input::keyboard::keysyms as k;
    matches!(
        sym.raw(),
        k::KEY_Shift_L
            | k::KEY_Shift_R
            | k::KEY_Control_L
            | k::KEY_Control_R
            | k::KEY_Alt_L
            | k::KEY_Alt_R
            | k::KEY_Super_L
            | k::KEY_Super_R
            | k::KEY_Meta_L
            | k::KEY_Meta_R
            | k::KEY_Hyper_L
            | k::KEY_Hyper_R
            | k::KEY_ISO_Level3_Shift
            | k::KEY_ISO_Level5_Shift
            | k::KEY_Caps_Lock
            | k::KEY_Num_Lock
    )
}

/// Touches de navigation des menus (prioritaires sur les raccourcis)
fn is_menu_key(sym: Keysym) -> bool {
    use smithay::input::keyboard::keysyms as k;
    matches!(
        sym.raw(),
        k::KEY_Escape
            | k::KEY_Up
            | k::KEY_Down
            | k::KEY_Left
            | k::KEY_Right
            | k::KEY_KP_Up
            | k::KEY_KP_Down
            | k::KEY_KP_Left
            | k::KEY_KP_Right
            | k::KEY_Return
            | k::KEY_KP_Enter
            | k::KEY_Home
            | k::KEY_End
    )
}

impl State {
    fn mods(&self) -> Mods {
        self.seat
            .get_keyboard()
            .map(|k| Mods::from_state(&k.modifier_state()))
            .unwrap_or_default()
    }

    /// Actions des liaisons de la souris correspondant à un événement
    fn mouse_actions(
        &self,
        ctxs: &[Context],
        event: MouseEvent,
        button: Option<MouseButton>,
        dir: Option<ScrollDir>,
        mods: Mods,
    ) -> Option<Vec<Action>> {
        let mut actions = vec![];
        let mut found = false;
        for ctx in ctxs {
            for b in &self.config.mousebinds {
                if b.context == *ctx
                    && b.event == event
                    && b.mods == mods
                    && b.button == button
                    && b.direction == dir
                {
                    found = true;
                    actions.extend(b.actions.iter().cloned());
                }
            }
        }
        found.then_some(actions)
    }

    /// Une liaison avec modificateurs existe pour ce bouton (quel que soit
    /// l'événement): l'appui ne va pas au client
    fn has_binding(&self, ctxs: &[Context], button: MouseButton, mods: Mods) -> bool {
        !mods.is_empty()
            && self
                .config
                .mousebinds
                .iter()
                .any(|b| ctxs.contains(&b.context) && b.button == Some(button) && b.mods == mods)
    }

    /// Événement d'un backend (winit)
    pub fn process_input<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event } => {
                let pressed = event.state() == KeyState::Pressed;
                self.key(event.key_code(), pressed, Event::time_msec(&event));
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let Some(output) = self.outputs.first().cloned() else {
                    return;
                };
                let geo = self.output_geometry(&output);
                let pos = event.position_transformed(geo.size) + geo.loc.to_f64();
                self.pointer_motion(pos, event.time_msec());
            }
            InputEvent::GestureSwipeBegin { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_swipe_begin(
                    self,
                    &GestureSwipeBeginEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                        fingers: event.fingers(),
                    },
                );
            }
            InputEvent::GestureSwipeUpdate { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_swipe_update(
                    self,
                    &GestureSwipeUpdateEvent {
                        time: event.time_msec(),
                        delta: event.delta(),
                    },
                );
            }
            InputEvent::GestureSwipeEnd { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_swipe_end(
                    self,
                    &GestureSwipeEndEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                        cancelled: event.cancelled(),
                    },
                );
            }
            InputEvent::GesturePinchBegin { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_pinch_begin(
                    self,
                    &GesturePinchBeginEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                        fingers: event.fingers(),
                    },
                );
            }
            InputEvent::GesturePinchUpdate { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_pinch_update(
                    self,
                    &GesturePinchUpdateEvent {
                        time: event.time_msec(),
                        delta: event.delta(),
                        scale: event.scale(),
                        rotation: event.rotation(),
                    },
                );
            }
            InputEvent::GesturePinchEnd { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_pinch_end(
                    self,
                    &GesturePinchEndEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                        cancelled: event.cancelled(),
                    },
                );
            }
            InputEvent::GestureHoldBegin { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_hold_begin(
                    self,
                    &GestureHoldBeginEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                        fingers: event.fingers(),
                    },
                );
            }
            InputEvent::GestureHoldEnd { event } => {
                let p = self.seat.get_pointer().unwrap();
                p.gesture_hold_end(
                    self,
                    &GestureHoldEndEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time: event.time_msec(),
                        cancelled: event.cancelled(),
                    },
                );
            }
            InputEvent::PointerMotion { event } => {
                self.pointer_relative(event.delta(), event.delta_unaccel(), event.time());
            }
            InputEvent::PointerButton { event } => {
                let pressed = event.state() == ButtonState::Pressed;
                self.pointer_button(event.button_code(), pressed, event.time_msec());
            }
            InputEvent::PointerAxis { event } => {
                let source = event.source();
                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                let mut steps = (0.0, 0.0);
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    let v120 = event.amount_v120(axis);
                    let amount = event
                        .amount(axis)
                        .unwrap_or_else(|| v120.unwrap_or(0.0) * 15.0 / 120.0);
                    let step = match v120 {
                        Some(d) => d / 120.0,
                        None => amount / 15.0,
                    };
                    if axis == Axis::Horizontal {
                        steps.0 = step;
                    } else {
                        steps.1 = step;
                    }
                    if amount != 0.0 {
                        frame = frame.value(axis, amount);
                        if let Some(d) = v120 {
                            frame = frame.v120(axis, d as i32);
                        }
                    }
                    if source == AxisSource::Finger && event.amount(axis) == Some(0.0) {
                        frame = frame.stop(axis);
                    }
                }
                self.pointer_axis(frame, steps);
            }
            _ => {}
        }
    }

    /// Touche: raccourci, sinon transmise au client
    pub fn key(&mut self, code: Keycode, pressed: bool, time: u32) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        self.activity();
        let serial = SERIAL_COUNTER.next_serial();
        let menu_open = self.menu.is_some();
        let cycling = self.cycle.is_some();
        let locked = self.is_locked();
        // Raccourcis inhibés par le client (machines virtuelles): seul
        // Super+Échap reste, pour lever l'inhibition
        let inhibitor = {
            use smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitorSeat;
            self.focused_surface()
                .and_then(|f| self.seat.keyboard_shortcuts_inhibitor_for_surface(&f))
                .filter(|i| i.is_active())
        };
        let result = keyboard.input(self, code, state, serial, time, |st, mods, handle| {
            if !pressed {
                if let Some(pos) = st.suppressed_keys.iter().position(|k| *k == code) {
                    st.suppressed_keys.remove(pos);
                    return FilterResult::Intercept(None);
                }
                return FilterResult::Forward;
            }
            let mut translated = handle.modified_syms();
            translated.extend(handle.raw_syms());
            let physical = st
                .us_keymap
                .as_ref()
                .and_then(|k| k.key_get_syms_by_level(code, 0, 0).first().copied());
            if let Some(inhibitor) = &inhibitor {
                let m = Mods::from_state(mods);
                let escape = handle.modified_sym() == Keysym::Escape;
                if escape && m.logo && !m.alt && !m.ctrl && !m.shift {
                    inhibitor.inactivate();
                    st.suppressed_keys.push(code);
                    return FilterResult::Intercept(None);
                }
                return FilterResult::Forward;
            }
            let raw = handle.modified_sym().raw();
            if (keysyms::KEY_XF86Switch_VT_1..=keysyms::KEY_XF86Switch_VT_12).contains(&raw) {
                st.suppressed_keys.push(code);
                let vt = (raw - keysyms::KEY_XF86Switch_VT_1 + 1) as i32;
                return FilterResult::Intercept(Some(KeyResult::Vt(vt)));
            }
            if cycling && handle.modified_sym() == Keysym::Escape {
                st.suppressed_keys.push(code);
                return FilterResult::Intercept(Some(KeyResult::CancelCycle));
            }
            // Verrouillé: seulement les raccourcis allowWhenLocked
            let bind = find_keybind(&st.config.keybinds, Mods::from_state(mods), &translated, physical)
                .filter(|b| !locked || b.allow_when_locked)
                .filter(|b| !st.keybinds_off || b.actions.contains(&Action::ToggleKeybinds))
                .map(|b| b.actions.clone());
            if menu_open && !locked {
                // Menu ouvert: les touches lui reviennent, sauf les
                // modificateurs; un raccourci ferme le menu
                let sym = handle.modified_sym();
                if is_modifier(sym) {
                    return FilterResult::Forward;
                }
                st.suppressed_keys.push(code);
                return FilterResult::Intercept(Some(KeyResult::Menu(sym, sym.key_char(), bind)));
            }
            match bind {
                Some(actions) => {
                    st.suppressed_keys.push(code);
                    FilterResult::Intercept(Some(KeyResult::Bind(actions)))
                }
                None => FilterResult::Forward,
            }
        });
        let actions = match result {
            Some(Some(KeyResult::Bind(actions))) => Some(actions),
            Some(Some(KeyResult::Vt(vt))) => {
                self.change_vt(vt);
                None
            }
            Some(Some(KeyResult::CancelCycle)) => {
                self.cancel_cycle();
                None
            }
            Some(Some(KeyResult::Menu(sym, ch, bind))) => {
                let nav = is_menu_key(sym);
                // Le raccourci du menu ouvert le referme
                let toggle = bind.as_deref().is_some_and(|b| {
                    matches!(b, [Action::ShowMenu { menu, .. }]
                        if self.open_menu_ids().first() == Some(menu))
                });
                if toggle && !nav {
                    self.close_menus();
                    None
                } else if nav || bind.is_none() {
                    self.menu_key(sym, ch);
                    None
                } else {
                    self.close_menus();
                    bind
                }
            }
            _ => None,
        };
        if let Some(actions) = actions {
            let target = self.focused_view().map(|i| self.views[i].window.clone());
            self.run_actions(
                &actions,
                target,
                Origin {
                    button: None,
                    location: self.pointer,
                },
            );
        }
        // Alt+Tab: fini quand plus aucun modificateur n'est enfoncé
        if self.cycle.is_some() && self.mods().is_empty() {
            self.end_cycle();
        }
    }

    pub fn pointer_motion(&mut self, pos: Point<f64, Logical>, time: u32) {
        self.activity();
        let Some(pos) = self.constrain(pos) else {
            return;
        };
        self.pointer = pos;
        if self.magnifier.is_some() {
            self.request_redraw();
        }
        let under = self.under(pos);
        // Survol des boutons du cadre
        let hovered = match &under {
            Under::Frame(i, Part::Button(b)) => Some((*i, *b)),
            _ => None,
        };
        for (i, view) in self.views.iter_mut().enumerate() {
            view.hovered = hovered.filter(|(j, _)| *j == i).map(|(_, b)| b);
        }
        // Focus qui suit la souris
        if self.config.follow_mouse
            && self.press.is_none()
            && self.menu.is_none()
            && let Under::View(i, ..) | Under::Frame(i, _) = under
            && self.focused_view() != Some(i)
        {
            let raise = self.config.raise_on_focus;
            self.focus_view(i, raise);
        }
        // Curseur du compositeur hors des clients: flèches de
        // redimensionnement sur les bords, flèche ailleurs (les clients
        // choisissent le leur en entrant)
        if !self.seat.get_pointer().is_some_and(|p| p.is_grabbed()) {
            match &under {
                Under::Frame(_, Part::Border(e)) => {
                    self.cursor_status = CursorImageStatus::Named(resize_cursor(*e));
                }
                Under::Frame(..) | Under::Root => {
                    self.cursor_status = CursorImageStatus::default_named();
                }
                _ => {}
            }
        }
        let focus = match self.under(pos) {
            Under::View(_, s, loc) | Under::Layer(s, loc, _) => Some((s, loc)),
            _ => None,
        };
        let pointer = self.seat.get_pointer().unwrap();
        pointer.motion(
            self,
            focus,
            &MotionEvent {
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
        // Glissement: liaisons Drag (Move, Resize…)
        let start_drag = self.press.as_ref().is_some_and(|p| {
            let d = pos - p.location;
            !p.dragged && d.x.hypot(d.y) > DRAG_THRESHOLD
        });
        if start_drag {
            let press = self.press.as_mut().unwrap();
            press.dragged = true;
            let press = press.clone();
            if let Some(actions) = self.mouse_actions(
                &press.contexts,
                MouseEvent::Drag,
                Some(press.button),
                None,
                press.mods,
            ) {
                let origin = Origin {
                    button: Some(button_code(press.button)),
                    location: press.location,
                };
                self.run_actions(&actions, press.window.clone(), origin);
                // Le geste commencé (déplacement, redimensionnement) reçoit
                // aussi le mouvement qui l'a déclenché
                let pointer = self.seat.get_pointer().unwrap();
                if pointer.is_grabbed() {
                    pointer.motion(
                        self,
                        None,
                        &MotionEvent {
                            location: pos,
                            serial: SERIAL_COUNTER.next_serial(),
                            time,
                        },
                    );
                    pointer.frame(self);
                }
            }
        }
    }

    pub fn pointer_button(&mut self, button: u32, pressed: bool, time: u32) {
        let pointer = self.seat.get_pointer().unwrap();
        let serial = SERIAL_COUNTER.next_serial();
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        let forward = |st: &mut State| {
            pointer.button(
                st,
                &ButtonEvent {
                    button,
                    state,
                    serial,
                    time,
                },
            );
            pointer.frame(st);
        };
        let Some(mbutton) = MouseButton::from_code(button) else {
            forward(self);
            return;
        };
        self.activity();
        // Verrouillé: tout va à la surface de verrouillage
        if self.is_locked() {
            forward(self);
            return;
        }
        if pressed {
            // Menu ouvert ou geste en cours: l'appui leur revient
            if pointer.is_grabbed() || self.press.is_some() {
                forward(self);
                return;
            }
            let mods = self.mods();
            let under = self.under(self.pointer);
            let (ctxs, view) = contexts(&under);
            if let Under::Frame(i, Part::Button(b)) = under
                && mbutton == MouseButton::Left
            {
                self.views[i].pressed = Some(b);
            }
            let window = view.map(|i| self.views[i].window.clone());
            let intercepted = self.has_binding(&ctxs, mbutton, mods);
            self.press = Some(Press {
                button: mbutton,
                mods,
                contexts: ctxs.clone(),
                window: window.clone(),
                location: self.pointer,
                dragged: false,
                intercepted,
            });
            if let Some(actions) = self.mouse_actions(&ctxs, MouseEvent::Press, Some(mbutton), None, mods) {
                self.run_actions(
                    &actions,
                    window,
                    Origin {
                        button: Some(button),
                        location: self.pointer,
                    },
                );
            }
            if !intercepted {
                // Le focus a pu changer: le client sous le pointeur reçoit l'appui
                let focus = match self.under(self.pointer) {
                    Under::View(_, s, loc) | Under::Layer(s, loc, _) => Some((s, loc)),
                    _ => None,
                };
                pointer.motion(
                    self,
                    focus,
                    &MotionEvent {
                        location: self.pointer,
                        serial,
                        time,
                    },
                );
                forward(self);
            }
            return;
        }
        // Relâche
        let press = self.press.take_if(|p| p.button == mbutton);
        for v in &mut self.views {
            v.pressed = None;
        }
        let Some(press) = press else {
            forward(self);
            return;
        };
        // Un geste (déplacement…) reçoit toujours la relâche
        if !press.intercepted || pointer.is_grabbed() {
            forward(self);
        }
        let origin = Origin {
            button: Some(button),
            location: self.pointer,
        };
        if let Some(actions) = self.mouse_actions(
            &press.contexts,
            MouseEvent::Release,
            Some(mbutton),
            None,
            press.mods,
        ) {
            self.run_actions(&actions, press.window.clone(), origin);
        }
        if press.dragged {
            return;
        }
        // Clic: relâché dans la même zone que l'appui
        let (now_ctxs, now_view) = contexts(&self.under(self.pointer));
        let same_view = now_view.map(|i| self.views[i].window.clone()) == press.window;
        if now_ctxs.first() != press.contexts.first() || !same_view {
            return;
        }
        if let Some(actions) = self.mouse_actions(
            &press.contexts,
            MouseEvent::Click,
            Some(mbutton),
            None,
            press.mods,
        ) {
            self.run_actions(&actions, press.window.clone(), origin);
        }
        let first = press.contexts.first().copied().unwrap_or(Context::Root);
        let now = Instant::now();
        let double = self.last_click.as_ref().is_some_and(|(b, c, w, t)| {
            *b == mbutton
                && *c == first
                && *w == press.window
                && now.duration_since(*t).as_millis() <= self.config.double_click_ms as u128
        });
        if double {
            self.last_click = None;
            if let Some(actions) = self.mouse_actions(
                &press.contexts,
                MouseEvent::DoubleClick,
                Some(mbutton),
                None,
                press.mods,
            ) {
                self.run_actions(&actions, press.window.clone(), origin);
            }
        } else {
            self.last_click = Some((mbutton, first, press.window.clone(), now));
        }
    }

    /// Molette: liaisons Scroll, sinon transmise au client. `steps`: crans
    /// horizontaux et verticaux (positifs vers la droite et le bas)
    pub fn pointer_axis(&mut self, frame: AxisFrame, steps: (f64, f64)) {
        self.activity();
        let pointer = self.seat.get_pointer().unwrap();
        if !pointer.is_grabbed() && !self.is_locked() {
            let (ctxs, view) = contexts(&self.under(self.pointer));
            let mods = self.mods();
            let any = [ScrollDir::Up, ScrollDir::Down, ScrollDir::Left, ScrollDir::Right]
                .iter()
                .any(|d| {
                    self.mouse_actions(&ctxs, MouseEvent::Scroll, None, Some(*d), mods)
                        .is_some()
                });
            if any {
                self.scroll_acc.0 += steps.0;
                self.scroll_acc.1 += steps.1;
                let mut dirs = vec![];
                while self.scroll_acc.1 >= 1.0 {
                    self.scroll_acc.1 -= 1.0;
                    dirs.push(ScrollDir::Down);
                }
                while self.scroll_acc.1 <= -1.0 {
                    self.scroll_acc.1 += 1.0;
                    dirs.push(ScrollDir::Up);
                }
                while self.scroll_acc.0 >= 1.0 {
                    self.scroll_acc.0 -= 1.0;
                    dirs.push(ScrollDir::Right);
                }
                while self.scroll_acc.0 <= -1.0 {
                    self.scroll_acc.0 += 1.0;
                    dirs.push(ScrollDir::Left);
                }
                let window = view.map(|i| self.views[i].window.clone());
                for d in dirs {
                    if let Some(actions) = self.mouse_actions(&ctxs, MouseEvent::Scroll, None, Some(d), mods)
                    {
                        let origin = Origin {
                            button: None,
                            location: self.pointer,
                        };
                        self.run_actions(&actions, window.clone(), origin);
                    }
                }
                return;
            }
            self.scroll_acc = (0.0, 0.0);
        }
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}

impl State {
    /// Mouvement relatif (souris, pavé tactile): le pointeur reste sur les
    /// écrans; le mouvement brut va aussi aux clients (relative-pointer)
    pub fn pointer_relative(&mut self, delta: Point<f64, Logical>, unaccel: Point<f64, Logical>, utime: u64) {
        let pointer = self.seat.get_pointer().unwrap();
        let focus = match self.under(self.pointer) {
            Under::View(_, s, loc) | Under::Layer(s, loc, _) => Some((s, loc)),
            _ => None,
        };
        pointer.relative_motion(
            self,
            focus,
            &RelativeMotionEvent {
                delta,
                delta_unaccel: unaccel,
                utime,
            },
        );
        let mut pos = self.pointer + delta;
        if !self
            .outputs
            .iter()
            .any(|o| self.output_geometry(o).to_f64().contains(pos))
            && let Some(o) = self.output_at(self.pointer)
        {
            let g = self.output_geometry(&o).to_f64();
            pos.x = pos.x.clamp(g.loc.x, g.loc.x + g.size.w - 1.0);
            pos.y = pos.y.clamp(g.loc.y, g.loc.y + g.size.h - 1.0);
        }
        self.pointer_motion(pos, (utime / 1000) as u32);
    }

    /// Contraintes du pointeur (pointer-constraints, jeux): `None` si le
    /// pointeur est bloqué; confiné, il reste sur la surface
    fn constrain(&mut self, pos: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
        use smithay::wayland::pointer_constraints::{PointerConstraint, with_pointer_constraint};
        let pointer = self.seat.get_pointer()?;
        let Some(surface) = pointer.current_focus() else {
            return Some(pos);
        };
        let mut kind = None;
        with_pointer_constraint(&surface, &pointer, |c| {
            if let Some(c) = c {
                if !c.is_active() {
                    c.activate();
                }
                kind = Some(matches!(&*c, PointerConstraint::Locked(_)));
            }
        });
        match kind {
            Some(true) => None,
            Some(false) => {
                let inside =
                    matches!(self.under(pos), Under::View(_, s, _) | Under::Layer(s, _, _) if s == surface);
                Some(if inside { pos } else { self.pointer })
            }
            None => Some(pos),
        }
    }
}

fn button_code(b: MouseButton) -> u32 {
    match b {
        MouseButton::Left => BTN_LEFT,
        MouseButton::Right => BTN_RIGHT,
        MouseButton::Middle => BTN_MIDDLE,
        MouseButton::Side => 0x113,
        MouseButton::Extra => 0x114,
    }
}

/// Déplacement d'une vue à la souris
pub struct MoveGrab {
    pub start: GrabStartData<State>,
    pub window: Window,
    pub initial_loc: Point<i32, Logical>,
    /// Lancé au clavier: fini au prochain clic
    pub keyboard: bool,
    /// Bord visé par le pointeur (collage au relâchement)
    pub snap: Option<Snap>,
}

/// Redimensionnement d'une vue à la souris
pub struct ResizeGrab {
    pub start: GrabStartData<State>,
    pub window: Window,
    pub edges: Edges,
    pub initial_size: Size<i32, Logical>,
    pub keyboard: bool,
    /// Taille voulue, appliquée à la fin si seul le contour est dessiné
    pub pending: Option<Size<i32, Logical>>,
}

impl ResizeGrab {
    /// Taille voulue pour un déplacement `d` du pointeur, dans les limites
    /// du client
    fn wanted(&self, d: Point<i32, Logical>) -> Size<i32, Logical> {
        let mut w = self.initial_size.w;
        let mut h = self.initial_size.h;
        if self.edges.left {
            w -= d.x;
        } else if self.edges.right {
            w += d.x;
        }
        if self.edges.top {
            h -= d.y;
        } else if self.edges.bottom {
            h += d.y;
        }
        let (min, max) = if let Some(x) = self.window.x11_surface() {
            (x.min_size().unwrap_or_default(), x.max_size().unwrap_or_default())
        } else if let Some(t) = self.window.toplevel() {
            smithay::wayland::compositor::with_states(t.wl_surface(), |s| {
                let mut cached = s
                    .cached_state
                    .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
                let c = cached.current();
                (c.min_size, c.max_size)
            })
        } else {
            (Size::default(), Size::default())
        };
        let clamp = |v: i32, lo: i32, hi: i32| {
            let v = v.max(lo.max(1));
            if hi > 0 { v.min(hi) } else { v }
        };
        Size::from((clamp(w, min.w, max.w), clamp(h, min.h, max.h)))
    }

    /// Envoie la taille au client (X11: taille et position tout de suite)
    fn apply(&self, data: &mut State, size: Size<i32, Logical>) {
        if let Some(x) = self.window.x11_surface() {
            if let Some(i) = data.window_index(&self.window) {
                let mut loc = data.views[i].loc;
                if self.edges.left {
                    loc.x += data.views[i].window.geometry().size.w - size.w;
                }
                if self.edges.top {
                    loc.y += data.views[i].window.geometry().size.h - size.h;
                }
                data.views[i].loc = loc;
                let _ = x.configure(smithay::utils::Rectangle::new(loc, size));
            }
            return;
        }
        let Some(toplevel) = self.window.toplevel() else {
            return;
        };
        toplevel.with_pending_state(|s| s.size = Some(size));
        if let Some(serial) = toplevel.send_pending_configure()
            && let Some(i) = data.window_index(&self.window)
            && let Some(r) = data.views[i].resize.as_mut()
        {
            r.last_serial = Some(serial);
        }
    }

    /// Cadre qu'aurait la vue à cette taille (le bord opposé reste fixe)
    fn frame_for(
        &self,
        data: &State,
        i: usize,
        size: Size<i32, Logical>,
    ) -> smithay::utils::Rectangle<i32, Logical> {
        let layout = data.layout(&data.views[i]);
        let deco = layout.size - data.views[i].client_size();
        let (loc0, size0) = data.views[i]
            .resize
            .map(|r| (r.initial_loc, r.initial_size))
            .unwrap_or((data.views[i].loc, self.initial_size));
        let mut loc = loc0;
        if self.edges.left {
            loc.x += size0.w - size.w;
        }
        if self.edges.top {
            loc.y += size0.h - size.h;
        }
        smithay::utils::Rectangle::new(loc - layout.client.loc, size + deco)
    }

    /// Taille affichée pendant le geste (§ 8.7, popupShow): en caractères
    /// pour un client X11 qui donne un pas de redimensionnement
    fn size_text(&self, data: &State, size: Size<i32, Logical>) -> Option<String> {
        use crate::config::PopupShow;
        // Les pas sont en pixels du client X (échelle de XWayland)
        let hints = self.window.x11_surface().and_then(|x| x.size_hints());
        let s = data.xwayland.scale;
        let client = Size::<i32, Logical>::from((
            (size.w as f64 * s).round() as i32,
            (size.h as f64 * s).round() as i32,
        ));
        let steps = hints.and_then(|h| {
            let (iw, ih) = h.size_increment.filter(|(a, b)| *a > 1 || *b > 1)?;
            let (bw, bh) = h.base_size.or(h.min_size).unwrap_or((0, 0));
            Some(((client.w - bw) / iw.max(1), (client.h - bh) / ih.max(1)))
        });
        match (data.config.resize_popup, steps) {
            (PopupShow::Never, _) | (PopupShow::Nonpixel, None) => None,
            (_, Some((c, r))) => Some(format!("{c} × {r}")),
            (PopupShow::Always, None) => Some(format!("{} × {}", size.w, size.h)),
        }
    }
}

/// Méthodes communes aux deux gestes: tout est transmis tel quel, sauf le
/// mouvement et les boutons
macro_rules! forward_grab {
    () => {
        fn relative_motion(
            &mut self,
            data: &mut State,
            handle: &mut PointerInnerHandle<'_, State>,
            focus: Option<(WlSurface, Point<f64, Logical>)>,
            event: &RelativeMotionEvent,
        ) {
            handle.relative_motion(data, focus, event);
        }
        fn axis(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, details: AxisFrame) {
            handle.axis(data, details)
        }
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
    };
}

/// Fin d'un geste: au relâchement du bouton, ou au clic s'il a été lancé au
/// clavier
fn grab_ends(
    keyboard: bool,
    start_button: u32,
    handle: &PointerInnerHandle<'_, State>,
    event: &ButtonEvent,
) -> bool {
    if keyboard {
        event.state == ButtonState::Pressed
    } else {
        !handle.current_pressed().contains(&start_button)
    }
}

impl PointerGrab<State> for MoveGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        data.pointer = event.location;
        let Some(i) = data.window_index(&self.window) else {
            return;
        };
        // Une vue imposée (maximisée, collée) reprend sa taille libre sous
        // le pointeur
        if data.views[i].tiling != Tiling::None {
            if data.views[i].tiling == Tiling::Fullscreen {
                return;
            }
            let before = data.frame_rect(i);
            let saved = data.views[i].saved;
            data.restore(i);
            // Sous le pointeur tout de suite, sans animation
            data.views[i].geom_anim = None;
            if let Some(saved) = saved {
                let layout = data.layout(&data.views[i]);
                let frame_w = saved.size.w + layout.size.w - layout.client.size.w;
                let ratio = (self.start.location.x - before.loc.x as f64) / before.size.w.max(1) as f64;
                let x = event.location.x - ratio * frame_w as f64;
                let y = event.location.y - (self.start.location.y - before.loc.y as f64);
                data.views[i].loc = Point::<f64, Logical>::from((x, y)).to_i32_round() + layout.client.loc;
            }
            self.initial_loc = data.views[i].loc;
            self.start.location = event.location;
            return;
        }
        let delta = event.location - self.start.location;
        let layout = data.layout(&data.views[i]);
        let mut frame = data.frame_rect(i);
        frame.loc = (self.initial_loc.to_f64() + delta).to_i32_round() - layout.client.loc;
        let zone = data.view_zone(i);
        let others: Vec<_> = data
            .stacking()
            .into_iter()
            .filter(|&j| j != i)
            .map(|j| data.frame_rect(j))
            .collect();
        let c = &data.config;
        let loc = resist(
            frame,
            zone,
            c.screen_edge_strength,
            &others,
            c.window_edge_strength,
        );
        data.views[i].loc = loc + layout.client.loc;
        // Collage: aperçu quand le pointeur approche d'un bord
        let zone_here = data
            .output_at(event.location)
            .map(|o| data.usable_area(&o))
            .unwrap_or(zone);
        self.snap = snap_edge(
            zone_here,
            event.location,
            data.config.snap_inner,
            data.config.snap_corner,
            data.config.top_maximize,
        );
        data.snap_preview = self.snap.map(|s| snap_rect(zone_here, s, data.config.gap));
        // Nouveau rectangle: son animation commence maintenant
        if data.snap_preview != data.snap_shown.map(|(r, _)| r) {
            let now = data.now();
            data.snap_shown = data.snap_preview.map(|r| {
                let anim = data.config.animations.start(crate::anim::Kind::SnapPreview, now);
                (r, anim)
            });
        }
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        // Lancé au clavier: le clic qui pose la vue n'est pas transmis
        if !self.keyboard {
            handle.button(data, event);
        }
        if grab_ends(self.keyboard, self.start.button, handle, event) {
            handle.unset_grab(self, data, event.serial, event.time, true);
            data.snap_preview = None;
            data.cursor_status = CursorImageStatus::default_named();
            if let Some(snap) = self.snap.take()
                && let Some(i) = data.window_index(&self.window)
            {
                data.snap(i, snap);
            }
        }
    }

    forward_grab!();

    fn unset(&mut self, data: &mut State) {
        data.snap_preview = None;
    }
}

impl PointerGrab<State> for ResizeGrab {
    fn motion(
        &mut self,
        data: &mut State,
        handle: &mut PointerInnerHandle<'_, State>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        handle.motion(data, None, event);
        data.pointer = event.location;
        let d = (event.location - self.start.location).to_i32_round::<i32>();
        let size = self.wanted(d);
        let Some(i) = data.window_index(&self.window) else {
            return;
        };
        let frame = self.frame_for(data, i, size);
        if data.config.resize_draw_contents {
            self.apply(data, size);
        } else {
            // drawContents=no: seulement le contour, taille appliquée à la fin
            self.pending = Some(size);
            data.resize_outline = Some(frame);
        }
        if let Some(text) = self.size_text(data, size) {
            let center = frame.loc + frame.size.to_point().downscale(2);
            data.show_size_osd(text, center);
        }
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        if !self.keyboard {
            handle.button(data, event);
        }
        if grab_ends(self.keyboard, self.start.button, handle, event) {
            handle.unset_grab(self, data, event.serial, event.time, true);
            data.cursor_status = CursorImageStatus::default_named();
            if let Some(size) = self.pending.take() {
                self.apply(data, size);
            }
            data.resize_outline = None;
            data.hide_size_osd();
            let serial = self.window.toplevel().and_then(|toplevel| {
                toplevel.with_pending_state(|s| s.states.unset(xdg_toplevel::State::Resizing));
                toplevel.send_pending_configure()
            });
            if let Some(i) = data.window_index(&self.window) {
                if self.window.is_x11() {
                    data.views[i].resize = None;
                } else if let Some(r) = data.views[i].resize.as_mut() {
                    r.ending = Some(serial.or(r.last_serial));
                }
            }
        }
    }

    forward_grab!();

    fn unset(&mut self, _data: &mut State) {}
}
