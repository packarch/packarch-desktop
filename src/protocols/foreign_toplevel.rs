// SPDX-License-Identifier: MIT OR Apache-2.0
//! Barre des tâches (cahier des charges § 9.2): wlr-foreign-toplevel-management
//! (module `wlr/taskbar` de waybar) et ext-foreign-toplevel-list. L'état
//! des vues est comparé une fois par image à ce qui a été envoyé, et seules
//! les différences partent.

use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::wayland::foreign_toplevel_list::ForeignToplevelHandle;
use wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_handle_v1::{
    self, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::server::zwlr_foreign_toplevel_manager_v1::{
    self, ZwlrForeignToplevelManagerV1,
};

use crate::state::State;
use crate::view::{Axis, Tiling};

const VERSION: u32 = 3;

/// Ce qui est envoyé d'une vue
#[derive(Debug, Clone, PartialEq)]
struct Info {
    title: String,
    app_id: String,
    /// États wlr: maximisée 0, réduite 1, active 2, plein écran 3
    states: Vec<u32>,
    output: Option<Output>,
}

struct Entry {
    id: u64,
    info: Info,
    handles: Vec<ZwlrForeignToplevelHandleV1>,
    ext: ForeignToplevelHandle,
}

pub struct WlrToplevels {
    managers: Vec<ZwlrForeignToplevelManagerV1>,
    /// Gestionnaires qui n'ont pas encore reçu les vues existantes
    fresh: Vec<ZwlrForeignToplevelManagerV1>,
    entries: Vec<Entry>,
}

impl WlrToplevels {
    /// Vue d'une poignée ext-foreign-toplevel
    pub fn view_of(&self, handle: &ForeignToplevelHandle) -> Option<u64> {
        let id = handle.identifier();
        self.entries
            .iter()
            .find(|e| e.ext.identifier() == id)
            .map(|e| e.id)
    }

    pub fn new(dh: &DisplayHandle) -> WlrToplevels {
        dh.create_global::<State, ZwlrForeignToplevelManagerV1, _>(VERSION, ());
        WlrToplevels {
            managers: vec![],
            fresh: vec![],
            entries: vec![],
        }
    }
}

fn states_bytes(states: &[u32]) -> Vec<u8> {
    states.iter().flat_map(|s| s.to_ne_bytes()).collect()
}

/// Crée la poignée d'une vue pour un gestionnaire, et envoie son état
fn create_handle(
    dh: &DisplayHandle,
    manager: &ZwlrForeignToplevelManagerV1,
    id: u64,
    info: &Info,
) -> Option<ZwlrForeignToplevelHandleV1> {
    let client = manager.client()?;
    let handle = client
        .create_resource::<ZwlrForeignToplevelHandleV1, u64, State>(dh, manager.version(), id)
        .ok()?;
    manager.toplevel(&handle);
    handle.title(info.title.clone());
    handle.app_id(info.app_id.clone());
    if let Some(o) = &info.output {
        for wl in o.client_outputs(&client) {
            handle.output_enter(&wl);
        }
    }
    handle.state(states_bytes(&info.states));
    handle.done();
    Some(handle)
}

impl State {
    fn toplevel_info(&self, i: usize) -> Info {
        let v = &self.views[i];
        let mut states = vec![];
        if v.tiling.fills_zone() {
            states.push(zwlr_foreign_toplevel_handle_v1::State::Maximized as u32);
        }
        if v.minimized {
            states.push(zwlr_foreign_toplevel_handle_v1::State::Minimized as u32);
        }
        if self.focused_view() == Some(i) {
            states.push(zwlr_foreign_toplevel_handle_v1::State::Activated as u32);
        }
        if v.tiling == Tiling::Fullscreen {
            states.push(zwlr_foreign_toplevel_handle_v1::State::Fullscreen as u32);
        }
        let frame = self.frame_rect(i);
        let center = frame.loc.to_f64() + frame.size.to_f64().to_point().downscale(2.0);
        Info {
            title: v.title(),
            app_id: v.app_id(),
            states,
            output: self.output_at(center),
        }
    }

    /// Envoie aux barres des tâches les vues apparues, changées ou fermées
    pub fn refresh_toplevels(&mut self) {
        let current: Vec<(u64, Info)> = (0..self.views.len())
            .filter(|&i| self.views[i].mapped && !self.views[i].skip_taskbar)
            .map(|i| (self.views[i].id, self.toplevel_info(i)))
            .collect();
        let dh = self.dh.clone();
        let p = &mut self.protocols;
        let t = &mut p.wlr_toplevels;
        // Fermées
        t.entries.retain(|e| {
            if current.iter().any(|(id, _)| *id == e.id) {
                return true;
            }
            for h in &e.handles {
                h.closed();
            }
            p.foreign_list.remove_toplevel(&e.ext);
            false
        });
        // Nouveaux gestionnaires: toutes les vues existantes
        for m in std::mem::take(&mut t.fresh) {
            if !m.is_alive() {
                continue;
            }
            for e in &mut t.entries {
                if let Some(h) = create_handle(&dh, &m, e.id, &e.info) {
                    e.handles.push(h);
                }
            }
        }
        for (id, info) in current {
            match t.entries.iter_mut().find(|e| e.id == id) {
                Some(e) => {
                    if e.info == info {
                        continue;
                    }
                    e.handles.retain(|h| h.is_alive());
                    for h in &e.handles {
                        if e.info.title != info.title {
                            h.title(info.title.clone());
                        }
                        if e.info.app_id != info.app_id {
                            h.app_id(info.app_id.clone());
                        }
                        if e.info.output != info.output
                            && let Some(client) = h.client()
                        {
                            if let Some(o) = &e.info.output {
                                for wl in o.client_outputs(&client) {
                                    h.output_leave(&wl);
                                }
                            }
                            if let Some(o) = &info.output {
                                for wl in o.client_outputs(&client) {
                                    h.output_enter(&wl);
                                }
                            }
                        }
                        if e.info.states != info.states {
                            h.state(states_bytes(&info.states));
                        }
                        h.done();
                    }
                    let mut ext_changed = false;
                    if e.info.title != info.title {
                        e.ext.send_title(&info.title);
                        ext_changed = true;
                    }
                    if e.info.app_id != info.app_id {
                        e.ext.send_app_id(&info.app_id);
                        ext_changed = true;
                    }
                    if ext_changed {
                        e.ext.send_done();
                    }
                    e.info = info;
                }
                None => {
                    let ext = p
                        .foreign_list
                        .new_toplevel::<State>(info.title.clone(), info.app_id.clone());
                    let handles = t
                        .managers
                        .iter()
                        .filter_map(|m| create_handle(&dh, m, id, &info))
                        .collect();
                    t.entries.push(Entry {
                        id,
                        info,
                        handles,
                        ext,
                    });
                }
            }
        }
    }

    fn view_by_id(&self, id: u64) -> Option<usize> {
        self.views.iter().position(|v| v.id == id)
    }
}

impl GlobalDispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn bind(
        state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrForeignToplevelManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        let m = data_init.init(resource, ());
        let t = &mut state.protocols.wlr_toplevels;
        t.managers.push(m.clone());
        t.fresh.push(m);
    }

    fn can_view(client: Client, _data: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrForeignToplevelManagerV1,
        request: zwlr_foreign_toplevel_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        if let zwlr_foreign_toplevel_manager_v1::Request::Stop = request {
            state.protocols.wlr_toplevels.managers.retain(|m| m != resource);
            resource.finished();
        }
    }

    fn destroyed(state: &mut State, _client: ClientId, resource: &ZwlrForeignToplevelManagerV1, _data: &()) {
        state.protocols.wlr_toplevels.managers.retain(|m| m != resource);
    }
}

impl Dispatch<ZwlrForeignToplevelHandleV1, u64> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        _resource: &ZwlrForeignToplevelHandleV1,
        request: zwlr_foreign_toplevel_handle_v1::Request,
        id: &u64,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_foreign_toplevel_handle_v1::Request as R;
        let Some(i) = state.view_by_id(*id) else { return };
        if state.is_locked() {
            return;
        }
        let tiling = state.views[i].tiling.clone();
        match request {
            R::SetMaximized => state.maximize(i, Axis::Both),
            R::UnsetMaximized if tiling.is_maximized() => state.restore(i),
            R::SetMinimized => state.iconify(i),
            R::UnsetMinimized => {
                state.focus_view(i, true);
            }
            R::Activate { .. } => {
                state.focus_view(i, true);
            }
            R::Close => state.views[i].close(),
            R::SetFullscreen { .. } if tiling != Tiling::Fullscreen => state.toggle_fullscreen(i),
            R::UnsetFullscreen if tiling == Tiling::Fullscreen => state.toggle_fullscreen(i),
            _ => {}
        }
    }

    fn destroyed(state: &mut State, _client: ClientId, resource: &ZwlrForeignToplevelHandleV1, _id: &u64) {
        for e in &mut state.protocols.wlr_toplevels.entries {
            e.handles.retain(|h| h != resource);
        }
    }
}
