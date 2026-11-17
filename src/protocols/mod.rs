// SPDX-License-Identifier: MIT OR Apache-2.0
//! Protocoles de l'usage quotidien (cahier des charges § 9.1, § 9.2) fournis
//! par Smithay: verrouillage, inactivité, activation, presse-papiers des
//! outils, méthodes de saisie, contraintes du pointeur, etc. Ceux que
//! Smithay ne fournit pas sont dans les sous-modules.

pub mod foreign_toplevel;
pub mod outputs;
pub mod screencopy;
pub mod workspace;

use std::sync::Arc;

use smithay::desktop::PopupKind;
use smithay::input::pointer::PointerHandle;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::{wl_output::WlOutput, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::alpha_modifier::AlphaModifierState;
use smithay::wayland::content_type::ContentTypeState;
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::foreign_toplevel_list::{ForeignToplevelListHandler, ForeignToplevelListState};
use smithay::wayland::idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState};
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};
use smithay::wayland::input_method::{InputMethodHandler, InputMethodManagerState, PopupSurface as ImPopup};
use smithay::wayland::keyboard_shortcuts_inhibit::{
    KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
};
use smithay::wayland::pointer_constraints::{
    PointerConstraintsHandler, PointerConstraintsState, with_pointer_constraint,
};
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::security_context::{
    SecurityContext, SecurityContextHandler, SecurityContextListenerSource, SecurityContextState,
};
use smithay::wayland::selection::ext_data_control::{
    DataControlHandler as ExtDataControlHandler, DataControlState as ExtDataControlState,
};
use smithay::wayland::selection::wlr_data_control::{DataControlHandler, DataControlState};
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
};
use smithay::wayland::shell::xdg::dialog::{XdgDialogHandler, XdgDialogState};
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::virtual_keyboard::VirtualKeyboardManagerState;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
};
use smithay::{
    delegate_alpha_modifier, delegate_content_type, delegate_cursor_shape, delegate_data_control,
    delegate_ext_data_control, delegate_foreign_toplevel_list, delegate_idle_inhibit, delegate_idle_notify,
    delegate_input_method_manager, delegate_keyboard_shortcuts_inhibit, delegate_pointer_constraints,
    delegate_pointer_gestures, delegate_presentation, delegate_relative_pointer, delegate_security_context,
    delegate_session_lock, delegate_single_pixel_buffer, delegate_text_input_manager,
    delegate_virtual_keyboard_manager, delegate_xdg_activation, delegate_xdg_dialog,
};
use tracing::{info, warn};

use crate::state::{ClientState, State};

/// Client de confiance: pas lancé dans un bac à sable (Flatpak…), il peut
/// voir les protocoles privilégiés (verrouillage, capture, presse-papiers)
pub fn trusted(client: &Client) -> bool {
    client
        .get_data::<ClientState>()
        .is_none_or(|d| d.security_context.is_none())
}

/// États des protocoles (ils vivent aussi longtemps que leurs globales)
#[allow(dead_code)]
pub struct Protocols {
    pub session_lock: SessionLockManagerState,
    pub idle_notifier: IdleNotifierState<State>,
    pub idle_inhibit: IdleInhibitManagerState,
    pub activation: XdgActivationState,
    pub shortcuts_inhibit: KeyboardShortcutsInhibitState,
    pub foreign_list: ForeignToplevelListState,
    pub data_control: DataControlState,
    pub ext_data_control: ExtDataControlState,
    pub security_context: SecurityContextState,
    pub input_method: InputMethodManagerState,
    pub text_input: TextInputManagerState,
    pub virtual_keyboard: VirtualKeyboardManagerState,
    pub pointer_constraints: PointerConstraintsState,
    pub relative_pointer: RelativePointerManagerState,
    pub pointer_gestures: PointerGesturesState,
    pub single_pixel: SinglePixelBufferState,
    pub alpha_modifier: AlphaModifierState,
    pub content_type: ContentTypeState,
    pub cursor_shape: CursorShapeManagerState,
    pub presentation: PresentationState,
    pub dialog: XdgDialogState,
    pub wlr_toplevels: foreign_toplevel::WlrToplevels,
    pub workspaces: workspace::Workspaces,
    pub screencopy: screencopy::Screencopy,
    pub outputs: outputs::OutputTools,
}

impl Protocols {
    pub fn new(
        dh: &DisplayHandle,
        handle: &smithay::reexports::calloop::LoopHandle<'static, State>,
        primary: &smithay::wayland::selection::primary_selection::PrimarySelectionState,
    ) -> Protocols {
        Protocols {
            session_lock: SessionLockManagerState::new::<State, _>(dh, trusted),
            idle_notifier: IdleNotifierState::new(dh, handle.clone()),
            idle_inhibit: IdleInhibitManagerState::new::<State>(dh),
            activation: XdgActivationState::new::<State>(dh),
            shortcuts_inhibit: KeyboardShortcutsInhibitState::new::<State>(dh),
            foreign_list: ForeignToplevelListState::new_with_filter::<State>(dh, trusted),
            data_control: DataControlState::new::<State, _>(dh, Some(primary), trusted),
            ext_data_control: ExtDataControlState::new::<State, _>(dh, Some(primary), trusted),
            security_context: SecurityContextState::new::<State, _>(dh, trusted),
            input_method: InputMethodManagerState::new::<State, _>(dh, trusted),
            text_input: TextInputManagerState::new::<State>(dh),
            virtual_keyboard: VirtualKeyboardManagerState::new::<State, _>(dh, trusted),
            pointer_constraints: PointerConstraintsState::new::<State>(dh),
            relative_pointer: RelativePointerManagerState::new::<State>(dh),
            pointer_gestures: PointerGesturesState::new::<State>(dh),
            single_pixel: SinglePixelBufferState::new::<State>(dh),
            alpha_modifier: AlphaModifierState::new::<State>(dh),
            content_type: ContentTypeState::new::<State>(dh),
            cursor_shape: CursorShapeManagerState::new::<State>(dh),
            presentation: PresentationState::new::<State>(dh, libc::CLOCK_MONOTONIC as u32),
            dialog: XdgDialogState::new::<State>(dh),
            wlr_toplevels: foreign_toplevel::WlrToplevels::new(dh),
            workspaces: workspace::Workspaces::new(dh),
            screencopy: screencopy::Screencopy::new(dh),
            outputs: outputs::OutputTools::new(dh),
        }
    }
}

// --- Verrouillage (ext-session-lock, § 9.2) -------------------------------

/// État du verrouillage
pub enum Lock {
    Unlocked,
    /// Demandé: confirmé après le premier rendu sans contenu
    Locking(SessionLocker),
    Locked,
}

impl State {
    pub fn is_locked(&self) -> bool {
        !matches!(self.lock, Lock::Unlocked)
    }

    /// Surface de verrouillage d'un écran
    pub fn lock_surface(&self, output: &Output) -> Option<&LockSurface> {
        self.lock_surfaces
            .iter()
            .find(|(o, _)| o == output)
            .map(|(_, s)| s)
    }

    /// Confirme le verrouillage (après un rendu)
    pub fn confirm_lock(&mut self) {
        if matches!(self.lock, Lock::Locking(_))
            && let Lock::Locking(locker) = std::mem::replace(&mut self.lock, Lock::Locked)
        {
            locker.lock();
            info!("session verrouillée");
        }
    }

    /// Donne le clavier à la surface de verrouillage de l'écran du pointeur
    fn focus_lock_surface(&mut self) {
        let surface = self
            .output_at(self.pointer)
            .and_then(|o| self.lock_surface(&o).map(|s| s.wl_surface().clone()))
            .or_else(|| self.lock_surfaces.first().map(|(_, s)| s.wl_surface().clone()));
        self.set_keyboard_focus(surface);
    }
}

impl SessionLockHandler for State {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.protocols.session_lock
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        self.close_menus();
        self.cycle = None;
        self.lock = Lock::Locking(confirmation);
        self.set_keyboard_focus(None);
    }

    fn unlock(&mut self) {
        info!("session déverrouillée");
        self.lock = Lock::Unlocked;
        self.lock_surfaces.clear();
        self.focus_top();
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let Some(output) = Output::from_resource(&output) else {
            return;
        };
        let size = self.output_geometry(&output).size;
        surface.with_pending_state(|s| s.size = Some((size.w as u32, size.h as u32).into()));
        surface.send_configure();
        self.lock_surfaces.retain(|(o, _)| *o != output);
        self.lock_surfaces.push((output, surface));
        self.focus_lock_surface();
    }
}

delegate_session_lock!(State);

// --- Inactivité (ext-idle-notify, idle-inhibit) ---------------------------

impl IdleNotifierHandler for State {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.protocols.idle_notifier
    }
}

impl IdleInhibitHandler for State {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.push(surface);
        self.protocols.idle_notifier.set_is_inhibited(true);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.retain(|s| *s != surface);
        let inhibited = self.idle_inhibitors.iter().any(|s| s.is_alive());
        self.protocols.idle_notifier.set_is_inhibited(inhibited);
    }
}

delegate_idle_notify!(State);
delegate_idle_inhibit!(State);

impl State {
    /// Activité de l'utilisateur (toute entrée)
    pub fn activity(&mut self) {
        let seat = self.seat.clone();
        self.protocols.idle_notifier.notify_activity(&seat);
        self.request_redraw();
    }
}

// --- Activation (xdg-activation, § 8.1) -----------------------------------

impl XdgActivationHandler for State {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.protocols.activation
    }

    /// Un jeton lié à une action récente de l'utilisateur donne le focus;
    /// sinon la vue est seulement marquée urgente
    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        let Some(i) = self.view_index(&surface) else {
            return;
        };
        let recent = data.timestamp.elapsed().as_secs() < 10;
        let from_focused = data
            .client_id
            .as_ref()
            .zip(self.focused_surface())
            .is_some_and(|(c, f)| f.client().is_some_and(|fc| fc.id() == *c));
        if recent && (data.serial.is_some() || from_focused) && !self.is_locked() {
            self.focus_view(i, true);
        } else {
            self.views[i].urgent = true;
        }
        self.protocols.activation.remove_token(&token);
    }
}

delegate_xdg_activation!(State);

// --- Inhibition des raccourcis (machines virtuelles, § 5.3) ---------------

impl KeyboardShortcutsInhibitHandler for State {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.protocols.shortcuts_inhibit
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        inhibitor.activate();
    }
}

delegate_keyboard_shortcuts_inhibit!(State);

// --- Liste des fenêtres (ext-foreign-toplevel-list) -----------------------

impl ForeignToplevelListHandler for State {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.protocols.foreign_list
    }
}

delegate_foreign_toplevel_list!(State);

// --- Presse-papiers des outils (wlr et ext data-control) ------------------

impl DataControlHandler for State {
    fn data_control_state(&self) -> &DataControlState {
        &self.protocols.data_control
    }
}

impl ExtDataControlHandler for State {
    fn data_control_state(&self) -> &ExtDataControlState {
        &self.protocols.ext_data_control
    }
}

delegate_data_control!(State);
delegate_ext_data_control!(State);

// --- Bacs à sable (security-context, Flatpak) -----------------------------

impl SecurityContextHandler for State {
    fn context_created(&mut self, source: SecurityContextListenerSource, context: SecurityContext) {
        let result = self.handle.insert_source(source, move |stream, _, state| {
            let data = ClientState {
                security_context: Some(context.clone()),
                ..ClientState::default()
            };
            if let Err(e) = state.dh.insert_client(stream, Arc::new(data)) {
                warn!("client d'un bac à sable: {e}");
            }
        });
        if let Err(e) = result {
            warn!("security-context: {e}");
        }
    }
}

delegate_security_context!(State);

// --- Dialogues -----------------------------------------------------------
// xdg-toplevel-icon n'est pas proposé: avec Smithay 0.7, la première
// configuration d'une fenêtre qui pose une icône (applications Qt) n'est
// jamais envoyée

impl XdgDialogHandler for State {}
delegate_xdg_dialog!(State);

// --- Méthodes de saisie, clavier virtuel ----------------------------------

impl InputMethodHandler for State {
    fn new_popup(&mut self, surface: ImPopup) {
        if let Err(e) = self.popups.track_popup(PopupKind::from(surface)) {
            warn!("fenêtre de la méthode de saisie: {e}");
        }
    }

    fn dismiss_popup(&mut self, surface: ImPopup) {
        if let Some(parent) = surface.get_parent().map(|p| p.surface.clone()) {
            let _ = smithay::desktop::PopupManager::dismiss_popup(&parent, &PopupKind::from(surface));
        }
    }

    fn popup_repositioned(&mut self, _surface: ImPopup) {}

    fn parent_geometry(&self, parent: &WlSurface) -> Rectangle<i32, Logical> {
        self.view_index(parent)
            .map(|i| self.views[i].window.geometry())
            .unwrap_or_default()
    }
}

delegate_input_method_manager!(State);
delegate_text_input_manager!(State);
delegate_virtual_keyboard_manager!(State);

// --- Pointeur: contraintes (jeux), mouvement relatif, gestes --------------

impl PointerConstraintsHandler for State {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        // Active tout de suite si le pointeur est sur la surface
        if pointer.current_focus().as_ref() == Some(surface) {
            with_pointer_constraint(surface, pointer, |c| {
                if let Some(c) = c {
                    c.activate();
                }
            });
        }
    }

    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        _location: smithay::utils::Point<f64, Logical>,
    ) {
    }
}

delegate_pointer_constraints!(State);
delegate_relative_pointer!(State);
delegate_pointer_gestures!(State);

// --- Divers ---------------------------------------------------------------

delegate_single_pixel_buffer!(State);
delegate_alpha_modifier!(State);
delegate_content_type!(State);
impl smithay::wayland::tablet_manager::TabletSeatHandler for State {}
delegate_cursor_shape!(State);
delegate_presentation!(State);
