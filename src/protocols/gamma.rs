// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tables gamma des écrans (cahier des charges § 9.2) : wlr-gamma-control
//! v1, pour gammastep et wlsunset (lumière de nuit). Un seul client par
//! écran ; quand il part, la table d'origine (linéaire) revient. Réservé
//! aux clients de confiance.

use std::io::Read;
use std::os::fd::OwnedFd;

use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use tracing::{debug, warn};
use wayland_protocols_wlr::gamma_control::v1::server::{
    zwlr_gamma_control_manager_v1::{self, ZwlrGammaControlManagerV1},
    zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
};

use crate::state::State;

pub fn init(dh: &DisplayHandle) {
    dh.create_global::<State, ZwlrGammaControlManagerV1, _>(1, ());
}

/// Réglages en cours : l'écran, son client, la taille d'une rampe
#[derive(Default)]
pub struct Gamma {
    controls: Vec<(Output, ZwlrGammaControlV1, u32)>,
}

/// Lit les trois rampes (rouge, vert, bleu : `size` valeurs de 16 bits
/// chacune) ; `None` si le fichier n'a pas exactement cette taille
fn read_ramps(fd: OwnedFd, size: u32) -> Option<Vec<u16>> {
    let len = size as usize * 3 * 2;
    let mut bytes = Vec::with_capacity(len + 1);
    std::fs::File::from(fd)
        .take(len as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() == len).then(|| {
        let (pairs, _) = bytes.as_chunks::<2>();
        pairs.iter().map(|b| u16::from_ne_bytes(*b)).collect()
    })
}

impl State {
    fn gamma_size(&self, output: &Output) -> Option<u32> {
        self.backend.as_ref()?.gamma_size(output).filter(|n| *n > 0)
    }

    /// Applique des rampes à un écran, ou rétablit la table linéaire
    fn apply_gamma(&mut self, output: &Output, ramps: Option<&[u16]>) -> bool {
        let done = self.backend.as_mut().is_some_and(|b| b.set_gamma(output, ramps));
        self.request_redraw();
        done
    }

    /// Écran retiré : son réglage est abandonné
    pub fn gamma_output_removed(&mut self, output: &Output) {
        self.protocols.gamma.controls.retain(|(o, control, _)| {
            if o == output {
                control.failed();
            }
            o != output
        });
    }

    /// Retire un réglage et rend sa table d'origine à l'écran
    fn release_gamma(&mut self, control: &ZwlrGammaControlV1) {
        let controls = &mut self.protocols.gamma.controls;
        if let Some(i) = controls.iter().position(|(_, c, _)| c == control) {
            let (output, ..) = controls.remove(i);
            if self.outputs.contains(&output) {
                self.apply_gamma(&output, None);
            }
        }
    }
}

impl GlobalDispatch<ZwlrGammaControlManagerV1, ()> for State {
    fn bind(
        _state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrGammaControlManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _data: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        _resource: &ZwlrGammaControlManagerV1,
        request: zwlr_gamma_control_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        if let zwlr_gamma_control_manager_v1::Request::GetGammaControl { id, output } = request {
            let control = data_init.init(id, ());
            let output = Output::from_resource(&output).filter(|o| state.outputs.contains(o));
            let taken = |o: &Output| state.protocols.gamma.controls.iter().any(|(c, ..)| c == o);
            // Écran inconnu, déjà réglé par un autre client, ou sans table
            // gamma (winit) : l'objet reste inerte
            match output
                .as_ref()
                .filter(|o| !taken(o))
                .and_then(|o| state.gamma_size(o))
            {
                Some(size) => {
                    control.gamma_size(size);
                    let output = output.unwrap();
                    debug!("gamma: {} réglé par un client ({size} valeurs)", output.name());
                    state.protocols.gamma.controls.push((output, control, size));
                }
                None => control.failed(),
            }
        }
    }
}

impl Dispatch<ZwlrGammaControlV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        control: &ZwlrGammaControlV1,
        request: zwlr_gamma_control_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        let zwlr_gamma_control_v1::Request::SetGamma { fd } = request else {
            return;
        };
        let controls = &state.protocols.gamma.controls;
        // Réglage déjà en échec : requête ignorée
        let Some((output, _, size)) = controls.iter().find(|(_, c, _)| c == control).cloned() else {
            return;
        };
        let Some(ramps) = read_ramps(fd, size) else {
            control.post_error(
                zwlr_gamma_control_v1::Error::InvalidGamma,
                "la table n'a pas la taille annoncée",
            );
            return;
        };
        if !state.apply_gamma(&output, Some(&ramps)) {
            warn!("gamma: {}: table refusée", output.name());
            control.failed();
            state.release_gamma(control);
        }
    }

    fn destroyed(state: &mut State, _client: ClientId, control: &ZwlrGammaControlV1, _data: &()) {
        state.release_gamma(control);
    }
}
