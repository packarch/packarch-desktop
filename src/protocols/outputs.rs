// SPDX-License-Identifier: MIT OR Apache-2.0
//! Écrans pour les outils (cahier des charges § 9.2): wlr-output-management
//! (wlr-randr, kanshi: mode, position, échelle) et wlr-output-power (wlopm:
//! extinction). L'état des écrans est comparé une fois par image à ce qui a
//! été envoyé.

use std::sync::Mutex;

use smithay::output::{Mode, Output, Scale};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use smithay::utils::{Point, Transform};
use tracing::info;
use wayland_protocols_wlr::output_management::v1::server::{
    zwlr_output_configuration_head_v1::{self, ZwlrOutputConfigurationHeadV1},
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};
use wayland_protocols_wlr::output_power_management::v1::server::{
    zwlr_output_power_manager_v1::{self, ZwlrOutputPowerManagerV1},
    zwlr_output_power_v1::{self, ZwlrOutputPowerV1},
};

use crate::state::State;

/// Ce qui est envoyé d'un écran
#[derive(Debug, Clone, PartialEq)]
struct HeadInfo {
    modes: Vec<Mode>,
    current: Option<Mode>,
    position: Point<i32, smithay::utils::Logical>,
    scale: f64,
    enabled: bool,
}

struct Head {
    output: Output,
    head: ZwlrOutputHeadV1,
    modes: Vec<(ZwlrOutputModeV1, Mode)>,
    info: HeadInfo,
}

struct Manager {
    manager: ZwlrOutputManagerV1,
    heads: Vec<Head>,
}

pub struct OutputTools {
    managers: Vec<Manager>,
    serial: u32,
    powers: Vec<(ZwlrOutputPowerV1, Output)>,
    /// Écrans éteints (wlopm)
    pub off: Vec<Output>,
}

impl OutputTools {
    pub fn new(dh: &DisplayHandle) -> OutputTools {
        dh.create_global::<State, ZwlrOutputManagerV1, _>(4, ());
        dh.create_global::<State, ZwlrOutputPowerManagerV1, _>(1, ());
        OutputTools {
            managers: vec![],
            serial: 1,
            powers: vec![],
            off: vec![],
        }
    }
}

/// Configuration demandée pour un écran
#[derive(Default)]
pub struct HeadConfig {
    mode: Option<Mode>,
    position: Option<Point<i32, smithay::utils::Logical>>,
    scale: Option<f64>,
    transform_ok: bool,
}

pub struct ConfigData {
    serial: u32,
    heads: Mutex<Vec<(Output, Option<ZwlrOutputConfigurationHeadV1>)>>,
    used: Mutex<bool>,
}

fn send_mode(m: &ZwlrOutputModeV1, mode: Mode, preferred: bool) {
    m.size(mode.size.w, mode.size.h);
    m.refresh(mode.refresh);
    if preferred {
        m.preferred();
    }
}

impl State {
    fn head_info(&self, output: &Output) -> HeadInfo {
        HeadInfo {
            modes: self.output_modes(output),
            current: output.current_mode(),
            position: output.current_location(),
            scale: output.current_scale().fractional_scale(),
            enabled: true,
        }
    }

    /// Modes possibles d'un écran (ceux du connecteur en session réelle)
    pub fn output_modes(&self, output: &Output) -> Vec<Mode> {
        match self.backend.as_ref() {
            Some(crate::backend::Backend::Udev(u)) => u.modes(output),
            _ => output.current_mode().into_iter().collect(),
        }
    }

    fn create_head(
        &self,
        dh: &DisplayHandle,
        manager: &ZwlrOutputManagerV1,
        output: &Output,
    ) -> Option<Head> {
        let client = manager.client()?;
        let head = client
            .create_resource::<ZwlrOutputHeadV1, (), State>(dh, manager.version(), ())
            .ok()?;
        manager.head(&head);
        let info = self.head_info(output);
        head.name(output.name());
        head.description(output.description());
        let size = output.physical_properties().size;
        if size.w > 0 && size.h > 0 {
            head.physical_size(size.w, size.h);
        }
        let preferred = output.preferred_mode();
        let mut modes = vec![];
        for mode in &info.modes {
            let Ok(m) = client.create_resource::<ZwlrOutputModeV1, (), State>(dh, manager.version(), ())
            else {
                continue;
            };
            head.mode(&m);
            send_mode(&m, *mode, preferred == Some(*mode));
            modes.push((m, *mode));
        }
        head.enabled(1);
        if let Some((m, _)) = modes.iter().find(|(_, x)| Some(*x) == info.current) {
            head.current_mode(m);
        }
        head.position(info.position.x, info.position.y);
        head.transform(smithay::utils::Transform::Normal.into());
        head.scale(info.scale);
        if head.version() >= 2 {
            head.make(output.physical_properties().make);
            head.model(output.physical_properties().model);
        }
        Some(Head {
            output: output.clone(),
            head,
            modes,
            info,
        })
    }

    /// Envoie aux outils les écrans apparus, changés ou débranchés
    pub fn refresh_output_tools(&mut self) {
        let dh = self.dh.clone();
        let outputs = self.outputs.clone();
        let mut managers = std::mem::take(&mut self.protocols.outputs.managers);
        managers.retain(|m| m.manager.is_alive());
        let mut changed = false;
        for m in &mut managers {
            m.heads.retain(|h| {
                if outputs.contains(&h.output) {
                    return true;
                }
                for (mode, _) in &h.modes {
                    mode.finished();
                }
                h.head.finished();
                changed = true;
                false
            });
            for o in &outputs {
                match m.heads.iter_mut().find(|h| h.output == *o) {
                    Some(h) => {
                        let info = self.head_info(o);
                        if info == h.info {
                            continue;
                        }
                        if info.current != h.info.current
                            && let Some((mode, _)) = h.modes.iter().find(|(_, x)| Some(*x) == info.current)
                        {
                            h.head.current_mode(mode);
                        }
                        if info.position != h.info.position {
                            h.head.position(info.position.x, info.position.y);
                        }
                        if info.scale != h.info.scale {
                            h.head.scale(info.scale);
                        }
                        h.info = info;
                        changed = true;
                    }
                    None => {
                        if let Some(h) = self.create_head(&dh, &m.manager, o) {
                            m.heads.push(h);
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed {
            self.protocols.outputs.serial += 1;
        }
        let serial = self.protocols.outputs.serial;
        for m in &managers {
            if changed {
                m.manager.done(serial);
            }
        }
        self.protocols.outputs.managers.extend(managers);
        // Extinction: l'état des écrans débranchés est oublié
        let p = &mut self.protocols.outputs;
        p.off.retain(|o| outputs.contains(o));
        p.powers.retain(|(r, _)| r.is_alive());
    }

    /// Applique (ou vérifie seulement, `test`) une configuration
    fn apply_output_config(&mut self, heads: &[(Output, Option<HeadConfig>)], test: bool) -> bool {
        for (output, cfg) in heads {
            let Some(cfg) = cfg else {
                // Écran à éteindre: pas pris en charge (wlopm pour l'extinction)
                return false;
            };
            if !cfg.transform_ok {
                return false;
            }
            if let Some(mode) = cfg.mode
                && !self.output_modes(output).contains(&mode)
            {
                return false;
            }
            if cfg.scale.is_some_and(|s| !(0.25..=4.0).contains(&s)) {
                return false;
            }
        }
        if test {
            return true;
        }
        for (output, cfg) in heads {
            let cfg = cfg.as_ref().unwrap();
            if let Some(mode) = cfg.mode
                && output.current_mode() != Some(mode)
            {
                if let Some(crate::backend::Backend::Udev(u)) = self.backend.as_mut()
                    && !u.set_mode(output, mode)
                {
                    return false;
                }
                output.change_current_state(Some(mode), None, None, None);
            }
            let scale = cfg.scale.map(Scale::Fractional);
            output.change_current_state(None, None, scale, cfg.position);
            let (size, loc) = (
                output.current_mode().map(|m| m.size).unwrap_or_default(),
                output.current_location(),
            );
            info!(
                "écran {}: {}×{} en ({}, {}), échelle {}",
                output.name(),
                size.w,
                size.h,
                loc.x,
                loc.y,
                output.current_scale().fractional_scale()
            );
            self.output_changed(output);
        }
        true
    }

    /// Un écran a changé (mode, position, échelle): couches, échelle des
    /// clients, vues imposées et vues perdues
    pub fn output_changed(&mut self, output: &Output) {
        smithay::desktop::layer_map_for_output(output).arrange();
        self.update_xwayland_scale();
        let scale = output.current_scale().fractional_scale();
        let mut surfaces: Vec<_> = self.views.iter().map(|v| v.surface().clone()).collect();
        surfaces.extend(
            smithay::desktop::layer_map_for_output(output)
                .layers()
                .map(|l| l.wl_surface().clone()),
        );
        for s in surfaces {
            smithay::wayland::compositor::with_states(&s, |data| {
                smithay::wayland::fractional_scale::with_fractional_scale(data, |fs| {
                    fs.set_preferred_scale(scale)
                });
                smithay::wayland::compositor::send_surface_state(
                    &s,
                    data,
                    scale.ceil() as i32,
                    Transform::Normal,
                );
            });
        }
        for i in 0..self.views.len() {
            match self.views[i].tiling.clone() {
                crate::view::Tiling::Maximized(a) => self.maximize(i, a),
                crate::view::Tiling::Edge(e) => self.snap_to_edge(i, e),
                crate::view::Tiling::Region(r) => self.snap_to_region(i, &r),
                _ => {}
            }
        }
    }

    /// Écran éteint (wlopm)
    pub fn output_off(&self, output: &Output) -> bool {
        self.protocols.outputs.off.contains(output)
    }

    fn set_power(&mut self, output: &Output, on: bool) {
        let p = &mut self.protocols.outputs;
        p.off.retain(|o| o != output);
        if !on {
            p.off.push(output.clone());
        }
        let mode = if on {
            zwlr_output_power_v1::Mode::On
        } else {
            zwlr_output_power_v1::Mode::Off
        };
        for (r, o) in &p.powers {
            if o == output {
                r.mode(mode);
            }
        }
        info!("écran {} {}", output.name(), if on { "allumé" } else { "éteint" });
        if let Some(crate::backend::Backend::Udev(u)) = self.backend.as_mut() {
            u.set_power(output, on);
        }
        if on {
            self.render_all();
        }
    }

    /// Redessine tous les écrans (session réelle; les autres backends
    /// dessinent en continu)
    pub fn render_all(&mut self) {
        self.request_redraw();
    }

    /// L'image a changé: à redessiner (session réelle)
    pub fn request_redraw(&mut self) {
        if matches!(self.backend, Some(crate::backend::Backend::Udev(_))) {
            self.udev_request_redraw();
        }
    }
}

// --- wlr-output-management ----------------------------------------------

impl GlobalDispatch<ZwlrOutputManagerV1, ()> for State {
    fn bind(
        state: &mut State,
        dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        let manager = data_init.init(resource, ());
        let mut heads = vec![];
        for o in state.outputs.clone() {
            if let Some(h) = state.create_head(dh, &manager, &o) {
                heads.push(h);
            }
        }
        manager.done(state.protocols.outputs.serial);
        state.protocols.outputs.managers.push(Manager { manager, heads });
    }

    fn can_view(client: Client, _data: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrOutputManagerV1,
        request: zwlr_output_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        match request {
            zwlr_output_manager_v1::Request::CreateConfiguration { id, serial } => {
                data_init.init(
                    id,
                    ConfigData {
                        serial,
                        heads: Mutex::new(vec![]),
                        used: Mutex::new(false),
                    },
                );
            }
            zwlr_output_manager_v1::Request::Stop => {
                resource.finished();
                state
                    .protocols
                    .outputs
                    .managers
                    .retain(|m| m.manager != *resource);
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut State, _client: ClientId, resource: &ZwlrOutputManagerV1, _data: &()) {
        state
            .protocols
            .outputs
            .managers
            .retain(|m| m.manager != *resource);
    }
}

impl Dispatch<ZwlrOutputHeadV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _resource: &ZwlrOutputHeadV1,
        _request: zwlr_output_head_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
    }
}

impl Dispatch<ZwlrOutputModeV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _resource: &ZwlrOutputModeV1,
        _request: zwlr_output_mode_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
    }
}

impl State {
    fn head_output(&self, head: &ZwlrOutputHeadV1) -> Option<Output> {
        self.protocols
            .outputs
            .managers
            .iter()
            .flat_map(|m| &m.heads)
            .find(|h| h.head == *head)
            .map(|h| h.output.clone())
    }

    fn mode_of(&self, mode: &ZwlrOutputModeV1) -> Option<Mode> {
        self.protocols
            .outputs
            .managers
            .iter()
            .flat_map(|m| &m.heads)
            .flat_map(|h| &h.modes)
            .find(|(m, _)| m == mode)
            .map(|(_, x)| *x)
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ConfigData> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrOutputConfigurationV1,
        request: zwlr_output_configuration_v1::Request,
        data: &ConfigData,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_output_configuration_v1::Request as R;
        match request {
            R::EnableHead { id, head } => {
                let Some(output) = state.head_output(&head) else {
                    resource.post_error(
                        zwlr_output_configuration_v1::Error::AlreadyConfiguredHead,
                        "écran inconnu",
                    );
                    return;
                };
                let ch = data_init.init(
                    id,
                    Mutex::new(HeadConfig {
                        transform_ok: true,
                        ..HeadConfig::default()
                    }),
                );
                data.heads.lock().unwrap().push((output, Some(ch)));
            }
            R::DisableHead { head } => {
                if let Some(output) = state.head_output(&head) {
                    data.heads.lock().unwrap().push((output, None));
                }
            }
            R::Apply | R::Test => {
                {
                    let mut used = data.used.lock().unwrap();
                    if *used {
                        resource
                            .post_error(zwlr_output_configuration_v1::Error::AlreadyUsed, "déjà utilisée");
                        return;
                    }
                    *used = true;
                }
                if data.serial != state.protocols.outputs.serial {
                    resource.cancelled();
                    return;
                }
                let heads: Vec<(Output, Option<HeadConfig>)> = data
                    .heads
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(o, ch)| {
                        let cfg = ch.as_ref().map(|ch| {
                            let c = ch.data::<Mutex<HeadConfig>>().unwrap().lock().unwrap();
                            HeadConfig {
                                mode: c.mode,
                                position: c.position,
                                scale: c.scale,
                                transform_ok: c.transform_ok,
                            }
                        });
                        (o.clone(), cfg)
                    })
                    .collect();
                if state.apply_output_config(&heads, matches!(request, R::Test)) {
                    resource.succeeded();
                } else {
                    resource.failed();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, Mutex<HeadConfig>> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        _resource: &ZwlrOutputConfigurationHeadV1,
        request: zwlr_output_configuration_head_v1::Request,
        data: &Mutex<HeadConfig>,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_output_configuration_head_v1::Request as R;
        let mode = match &request {
            R::SetMode { mode } => state.mode_of(mode),
            _ => None,
        };
        let mut c = data.lock().unwrap();
        match request {
            R::SetMode { .. } => c.mode = mode,
            R::SetCustomMode {
                width,
                height,
                refresh,
            } => {
                c.mode = Some(Mode {
                    size: (width, height).into(),
                    refresh: if refresh > 0 { refresh } else { 60_000 },
                })
            }
            R::SetPosition { x, y } => c.position = Some((x, y).into()),
            R::SetScale { scale } => c.scale = Some(scale),
            R::SetTransform { transform } => {
                c.transform_ok =
                    matches!(transform.into_result(), Ok(t) if Transform::from(t) == Transform::Normal);
            }
            _ => {}
        }
    }
}

// --- wlr-output-power -----------------------------------------------------

impl GlobalDispatch<ZwlrOutputPowerManagerV1, ()> for State {
    fn bind(
        _state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrOutputPowerManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _data: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ZwlrOutputPowerManagerV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        _resource: &ZwlrOutputPowerManagerV1,
        request: zwlr_output_power_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        if let zwlr_output_power_manager_v1::Request::GetOutputPower { id, output } = request {
            let r = data_init.init(id, ());
            match Output::from_resource(&output) {
                Some(o) => {
                    r.mode(if state.output_off(&o) {
                        zwlr_output_power_v1::Mode::Off
                    } else {
                        zwlr_output_power_v1::Mode::On
                    });
                    state.protocols.outputs.powers.push((r, o));
                }
                None => r.failed(),
            }
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, ()> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        resource: &ZwlrOutputPowerV1,
        request: zwlr_output_power_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        if let zwlr_output_power_v1::Request::SetMode { mode } = request {
            let Some(output) = state
                .protocols
                .outputs
                .powers
                .iter()
                .find(|(r, _)| r == resource)
                .map(|(_, o)| o.clone())
            else {
                return;
            };
            let on = !matches!(mode.into_result(), Ok(zwlr_output_power_v1::Mode::Off));
            state.set_power(&output, on);
        }
    }

    fn destroyed(state: &mut State, _client: ClientId, resource: &ZwlrOutputPowerV1, _data: &()) {
        state.protocols.outputs.powers.retain(|(r, _)| r != resource);
    }
}
