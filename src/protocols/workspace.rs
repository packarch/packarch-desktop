// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bureaux pour les barres (cahier des charges § 8.4): ext-workspace-v1,
//! module `ext/workspaces` de waybar. Un groupe pour tous les écrans, un
//! espace de travail par bureau; le clic d'un bureau le rend actif.

use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use wayland_protocols::ext::workspace::v1::server::ext_workspace_group_handle_v1::{
    self, ExtWorkspaceGroupHandleV1, GroupCapabilities,
};
use wayland_protocols::ext::workspace::v1::server::ext_workspace_handle_v1::{
    self, ExtWorkspaceHandleV1, WorkspaceCapabilities,
};
use wayland_protocols::ext::workspace::v1::server::ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1};

use crate::state::State;

const VERSION: u32 = 1;

/// Ce qu'un client a reçu
struct Instance {
    manager: ExtWorkspaceManagerV1,
    group: Option<ExtWorkspaceGroupHandleV1>,
    workspaces: Vec<ExtWorkspaceHandleV1>,
    names: Vec<String>,
    active: Option<usize>,
    outputs: Vec<Output>,
    /// Bureau à activer au prochain `commit`
    pending: Option<usize>,
}

pub struct Workspaces {
    instances: Vec<Instance>,
}

impl Workspaces {
    pub fn new(dh: &DisplayHandle) -> Workspaces {
        dh.create_global::<State, ExtWorkspaceManagerV1, _>(VERSION, ());
        Workspaces { instances: vec![] }
    }
}

fn coords(i: usize) -> Vec<u8> {
    (i as u32).to_ne_bytes().to_vec()
}

impl State {
    /// Envoie aux barres les bureaux, l'actif et les écrans
    pub fn refresh_workspaces(&mut self) {
        let names = self.config.desktop_names.clone();
        let active = self.desktop;
        let outputs = self.outputs.clone();
        let dh = self.dh.clone();
        let w = &mut self.protocols.workspaces;
        w.instances.retain(|x| x.manager.is_alive());
        for inst in &mut w.instances {
            let Some(client) = inst.manager.client() else {
                continue;
            };
            let mut changed = false;
            if inst.group.is_none() {
                let Ok(group) = client.create_resource::<ExtWorkspaceGroupHandleV1, (), State>(
                    &dh,
                    inst.manager.version(),
                    (),
                ) else {
                    continue;
                };
                inst.manager.workspace_group(&group);
                group.capabilities(GroupCapabilities::empty());
                inst.group = Some(group);
                changed = true;
            }
            let group = inst.group.clone().unwrap();
            for o in &outputs {
                if !inst.outputs.contains(o) {
                    let wls: Vec<_> = o.client_outputs(&client).collect();
                    if wls.is_empty() {
                        continue;
                    }
                    for wl in wls {
                        group.output_enter(&wl);
                    }
                    inst.outputs.push(o.clone());
                    changed = true;
                }
            }
            if inst.names != names {
                for ws in inst.workspaces.drain(..) {
                    group.workspace_leave(&ws);
                    ws.removed();
                }
                for (i, name) in names.iter().enumerate() {
                    let Ok(ws) = client.create_resource::<ExtWorkspaceHandleV1, usize, State>(
                        &dh,
                        inst.manager.version(),
                        i,
                    ) else {
                        continue;
                    };
                    inst.manager.workspace(&ws);
                    ws.id(format!("packarch-{}", i + 1));
                    ws.name(name.clone());
                    ws.coordinates(coords(i));
                    ws.capabilities(WorkspaceCapabilities::Activate);
                    ws.state(if i == active {
                        ext_workspace_handle_v1::State::Active
                    } else {
                        ext_workspace_handle_v1::State::empty()
                    });
                    group.workspace_enter(&ws);
                    inst.workspaces.push(ws);
                }
                inst.names = names.clone();
                inst.active = Some(active);
                changed = true;
            }
            if inst.active != Some(active) {
                for (i, ws) in inst.workspaces.iter().enumerate() {
                    if Some(i) == inst.active || i == active {
                        ws.state(if i == active {
                            ext_workspace_handle_v1::State::Active
                        } else {
                            ext_workspace_handle_v1::State::empty()
                        });
                    }
                }
                inst.active = Some(active);
                changed = true;
            }
            if changed {
                inst.manager.done();
            }
        }
    }
}

impl GlobalDispatch<ExtWorkspaceManagerV1, ()> for State {
    fn bind(
        state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ExtWorkspaceManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        let manager = data_init.init(resource, ());
        state.protocols.workspaces.instances.push(Instance {
            manager,
            group: None,
            workspaces: vec![],
            names: vec![],
            active: None,
            outputs: vec![],
            pending: None,
        });
        state.refresh_workspaces();
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ExtWorkspaceManagerV1,
        request: ext_workspace_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            ext_workspace_manager_v1::Request::Commit => {
                let pending = state
                    .protocols
                    .workspaces
                    .instances
                    .iter_mut()
                    .find(|x| x.manager == *resource)
                    .and_then(|x| x.pending.take());
                if let Some(d) = pending
                    && !state.is_locked()
                {
                    state.switch_desktop(d);
                }
            }
            ext_workspace_manager_v1::Request::Stop => {
                resource.finished();
                state
                    .protocols
                    .workspaces
                    .instances
                    .retain(|x| x.manager != *resource);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut State, _client: ClientId, resource: &ExtWorkspaceManagerV1, _data: &()) {
        state
            .protocols
            .workspaces
            .instances
            .retain(|x| x.manager != *resource);
    }
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _resource: &ExtWorkspaceGroupHandleV1,
        _request: ext_workspace_group_handle_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        // Création de bureaux: non prise en charge (capacités vides)
    }
}

impl Dispatch<ExtWorkspaceHandleV1, usize> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ExtWorkspaceHandleV1,
        request: ext_workspace_handle_v1::Request,
        index: &usize,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        if let ext_workspace_handle_v1::Request::Activate = request
            && let Some(inst) = state
                .protocols
                .workspaces
                .instances
                .iter_mut()
                .find(|x| x.workspaces.contains(resource))
        {
            inst.pending = Some(*index);
        }
    }
}
