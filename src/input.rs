// SPDX-License-Identifier: MIT OR Apache-2.0
//! Entrées: pointeur, clavier, gestes de déplacement et de
//! redimensionnement (§ 8.6, § 8.7). Prototype: pas encore de raccourcis.

use std::time::{Duration, Instant};

use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent, KeyState,
    KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::desktop::Window;
use smithay::input::keyboard::{FilterResult, Keycode};
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, GrabStartData, MotionEvent, PointerGrab, PointerInnerHandle,
    RelativeMotionEvent,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, SERIAL_COUNTER, Size};
use tracing::info;

use crate::config::Button;
use crate::deco::{Edges, Part};
use crate::state::{State, Under};

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

const DOUBLE_CLICK: Duration = Duration::from_millis(400);

impl State {
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
            InputEvent::PointerButton { event } => {
                let pressed = event.state() == ButtonState::Pressed;
                self.pointer_button(event.button_code(), pressed, event.time_msec());
            }
            InputEvent::PointerAxis { event } => {
                let source = event.source();
                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                for axis in [Axis::Horizontal, Axis::Vertical] {
                    let v120 = event.amount_v120(axis);
                    let amount = event
                        .amount(axis)
                        .unwrap_or_else(|| v120.unwrap_or(0.0) * 15.0 / 120.0);
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
                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }

    pub fn key(&mut self, code: Keycode, pressed: bool, time: u32) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        keyboard.input::<(), _>(
            self,
            code,
            state,
            SERIAL_COUNTER.next_serial(),
            time,
            |_, _, _| FilterResult::Forward,
        );
    }

    pub fn pointer_motion(&mut self, pos: Point<f64, Logical>, time: u32) {
        self.pointer = pos;
        let under = self.under(pos);
        // Survol des boutons du cadre
        let hovered = match &under {
            Under::Frame(i, Part::Button(b)) => Some((*i, *b)),
            _ => None,
        };
        for (i, view) in self.views.iter_mut().enumerate() {
            let h = hovered.filter(|(j, _)| *j == i).map(|(_, b)| b);
            if view.hovered != h {
                view.hovered = h;
                if h.is_none() {
                    view.pressed = None;
                }
            }
        }
        let focus = match under {
            Under::Surface(s, loc, _) => Some((s, loc)),
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
    }

    pub fn pointer_button(&mut self, button: u32, pressed: bool, time: u32) {
        let pointer = self.seat.get_pointer().unwrap();
        let serial = SERIAL_COUNTER.next_serial();
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        // Relâche d'un bouton du cadre: l'appui a posé une saisie implicite
        // du pointeur, on ne passe donc pas par la recherche ci-dessous
        if !pressed && let Some(i) = self.views.iter().position(|v| v.pressed.is_some()) {
            let b = self.views[i].pressed.take().unwrap();
            let over =
                matches!(self.under(self.pointer), Under::Frame(j, Part::Button(b2)) if j == i && b2 == b);
            pointer.button(
                self,
                &ButtonEvent {
                    button,
                    state,
                    serial,
                    time,
                },
            );
            pointer.frame(self);
            if over && button == BTN_LEFT {
                self.button_action(i, b);
            }
            return;
        }
        if !pointer.is_grabbed() {
            match self.under(self.pointer) {
                Under::Surface(surface, _, view) => {
                    if pressed {
                        match view {
                            Some(i) => self.focus_view(i),
                            None => self.focus_layer_on_click(&surface),
                        }
                    }
                }
                Under::Frame(i, part) => {
                    pointer.button(
                        self,
                        &ButtonEvent {
                            button,
                            state,
                            serial,
                            time,
                        },
                    );
                    pointer.frame(self);
                    self.frame_button(i, part, button, pressed, serial);
                    return;
                }
                Under::Root => {}
            }
        }
        pointer.button(
            self,
            &ButtonEvent {
                button,
                state,
                serial,
                time,
            },
        );
        pointer.frame(self);
    }

    /// Couche « à la demande » (barre, notifications): le clavier au clic
    fn focus_layer_on_click(&mut self, surface: &WlSurface) {
        use smithay::wayland::shell::wlr_layer::KeyboardInteractivity;
        for output in self.outputs.clone() {
            let map = smithay::desktop::layer_map_for_output(&output);
            let layer = map
                .layer_for_surface(surface, smithay::desktop::WindowSurfaceType::ALL)
                .cloned();
            drop(map);
            if let Some(layer) = layer
                && layer.cached_state().keyboard_interactivity != KeyboardInteractivity::None
            {
                self.set_keyboard_focus(Some(layer.wl_surface().clone()));
            }
        }
    }

    /// Clic sur le cadre d'une vue
    fn frame_button(
        &mut self,
        i: usize,
        part: Part,
        button: u32,
        pressed: bool,
        serial: smithay::utils::Serial,
    ) {
        let window = self.views[i].window.clone();
        if pressed {
            self.focus_view(i);
        }
        let Some(i) = self.views.iter().position(|v| v.window == window) else {
            return;
        };
        let start = || GrabStartData {
            focus: None,
            button,
            location: self.pointer,
        };
        match (part, button, pressed) {
            (Part::Button(b), BTN_LEFT, true) => self.views[i].pressed = Some(b),
            (Part::Title, BTN_LEFT, true) => {
                let now = Instant::now();
                let double = self
                    .last_title_click
                    .as_ref()
                    .is_some_and(|(w, t)| *w == window && now.duration_since(*t) < DOUBLE_CLICK);
                if double {
                    self.last_title_click = None;
                    self.toggle_maximize(i);
                } else {
                    self.last_title_click = Some((window, now));
                    let s = start();
                    self.start_move(i, s, serial);
                }
            }
            (Part::Border(edges), BTN_LEFT, true) => {
                let s = start();
                self.start_resize(i, edges, s, serial);
            }
            (Part::Title, BTN_MIDDLE, true) => {
                // Clic du milieu sur le titre: à l'arrière-plan, comme Openbox
                let view = self.views.remove(i);
                self.views.insert(0, view);
                self.focus_top();
            }
            _ => {}
        }
    }

    fn button_action(&mut self, i: usize, button: Button) {
        match button {
            Button::Close => {
                tracing::debug!("fermeture: « {} »", self.views[i].title());
                self.views[i].window.toplevel().unwrap().send_close()
            }
            Button::Maximize => self.toggle_maximize(i),
            Button::Iconify => info!("réduction: prévue avec la barre des tâches (0.4)"),
            Button::Shade | Button::Desk => info!("{button:?}: prévu en 0.2"),
        }
    }
}

/// Déplacement d'une vue à la souris
pub struct MoveGrab {
    pub start: GrabStartData<State>,
    pub window: Window,
    pub initial_loc: Point<i32, Logical>,
}

/// Redimensionnement d'une vue à la souris
pub struct ResizeGrab {
    pub start: GrabStartData<State>,
    pub window: Window,
    pub edges: Edges,
    pub initial_size: Size<i32, Logical>,
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
        let delta = event.location - self.start.location;
        if let Some(view) = data.views.iter_mut().find(|v| v.window == self.window) {
            if view.maximized.is_some() {
                return;
            }
            view.loc = (self.initial_loc.to_f64() + delta).to_i32_round();
        }
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    forward_grab!();

    fn unset(&mut self, _data: &mut State) {}
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
        let toplevel = self.window.toplevel().unwrap();
        let (min, max) = smithay::wayland::compositor::with_states(toplevel.wl_surface(), |s| {
            let mut cached = s
                .cached_state
                .get::<smithay::wayland::shell::xdg::SurfaceCachedState>();
            let c = cached.current();
            (c.min_size, c.max_size)
        });
        let clamp = |v: i32, lo: i32, hi: i32| {
            let v = v.max(lo.max(1));
            if hi > 0 { v.min(hi) } else { v }
        };
        let size = Size::from((clamp(w, min.w, max.w), clamp(h, min.h, max.h)));
        toplevel.with_pending_state(|s| s.size = Some(size));
        toplevel.send_pending_configure();
    }

    fn button(&mut self, data: &mut State, handle: &mut PointerInnerHandle<'_, State>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
            let toplevel = self.window.toplevel().unwrap();
            toplevel.with_pending_state(|s| s.states.unset(xdg_toplevel::State::Resizing));
            toplevel.send_pending_configure();
            if let Some(view) = data.views.iter_mut().find(|v| v.window == self.window)
                && let Some(r) = view.resize.as_mut()
            {
                r.ending = true;
            }
        }
    }

    forward_grab!();

    fn unset(&mut self, _data: &mut State) {}
}
