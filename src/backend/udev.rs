// SPDX-License-Identifier: MIT OR Apache-2.0
//! Backend udev (cahier des charges § 4.3): session réelle sur une console,
//! DRM/KMS sur le GPU principal (un compositeur DRM par écran branché),
//! libseat (logind ou seatd), libinput, branchement à chaud des écrans,
//! changement de console (Ctrl+Alt+F1…F12) et reprise.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use smithay::backend::allocator::Fourcc;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::drm::compositor::{DrmCompositor, FrameFlags};
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmNode};
use smithay::backend::egl::{EGLContext, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{UdevBackend, UdevEvent, primary_gpu};
use smithay::desktop::layer_map_for_output;
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, RegistrationToken};
use smithay::reexports::drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc};
use smithay::reexports::input::{self as libinput, Libinput};
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{DeviceFd, Transform};
use tracing::{debug, info, warn};

use super::{Backend, after_render, init_renderer};
use crate::render::{CLEAR, output_elements};
use crate::state::State;

type Compositor =
    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// Un écran branché
struct Surface {
    output: Output,
    global: GlobalId,
    connector: connector::Handle,
    compositor: Compositor,
    /// Modes du connecteur
    modes: Vec<smithay::reexports::drm::control::Mode>,
    /// Une image attend le prochain vblank
    queued: bool,
    /// Éteint (wlopm)
    off: bool,
    /// L'image a changé depuis le dernier rendu
    dirty: bool,
    /// Un rendu est déjà prévu
    scheduled: bool,
}

pub struct Udev {
    pub session: LibSeatSession,
    pub renderer: GlesRenderer,
    node: DrmNode,
    drm: DrmDevice,
    gbm: GbmDevice<DrmDeviceFd>,
    surfaces: HashMap<crtc::Handle, Surface>,
    libinput: Libinput,
    /// Sources des écrans (pour les retirer avec le GPU)
    _drm_token: RegistrationToken,
}

impl Udev {
    /// Modes possibles d'un écran
    pub fn modes(&self, output: &Output) -> Vec<OutputMode> {
        self.surfaces
            .values()
            .find(|s| s.output == *output)
            .map(|s| s.modes.iter().map(|m| OutputMode::from(*m)).collect())
            .unwrap_or_default()
    }

    /// Change le mode d'un écran
    pub fn set_mode(&mut self, output: &Output, mode: OutputMode) -> bool {
        let Some(s) = self.surfaces.values_mut().find(|s| s.output == *output) else {
            return false;
        };
        let Some(drm_mode) = s.modes.iter().find(|m| OutputMode::from(**m) == mode).copied() else {
            return false;
        };
        match s.compositor.use_mode(drm_mode) {
            Ok(()) => true,
            Err(e) => {
                warn!("{}: mode: {e:?}", output.name());
                false
            }
        }
    }

    /// Allume ou éteint un écran
    pub fn set_power(&mut self, output: &Output, on: bool) {
        let Some(s) = self.surfaces.values_mut().find(|s| s.output == *output) else {
            return;
        };
        s.off = !on;
        if !on {
            if let Err(e) = s.compositor.surface().clear() {
                warn!("{}: extinction: {e}", output.name());
            }
            s.queued = false;
        } else if let Err(e) = s.compositor.reset_state() {
            warn!("{}: {e}", output.name());
        }
    }

    pub fn change_vt(&mut self, vt: i32) {
        if let Err(e) = self.session.change_vt(vt) {
            warn!("console {vt}: {e}");
        }
    }
}

pub fn init(event_loop: &mut EventLoop<'static, State>, state: &mut State) -> anyhow::Result<()> {
    let (mut session, notifier) =
        LibSeatSession::new().map_err(|e| anyhow::anyhow!("session libseat: {e}"))?;
    let seat = session.seat();
    info!("siège {seat}");
    let path: PathBuf = primary_gpu(&seat)?
        .or_else(|| smithay::backend::udev::all_gpus(&seat).ok()?.into_iter().next())
        .ok_or_else(|| anyhow::anyhow!("aucun GPU"))?;
    info!("GPU: {}", path.display());
    let fd = session
        .open(
            &path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        )
        .map_err(|e| anyhow::anyhow!("ouverture de {}: {e}", path.display()))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let node = DrmNode::from_file(fd.clone())?;
    let (drm, drm_notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd.clone())?;
    // Sûr: l'affichage EGL vit aussi longtemps que le moteur de rendu
    let display = unsafe { EGLDisplay::new(gbm.clone())? };
    let context = EGLContext::new(&display)?;
    let mut renderer = unsafe { GlesRenderer::new(context)? };
    init_renderer(state, &mut renderer);

    // Entrées
    let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput
        .udev_assign_seat(&seat)
        .map_err(|_| anyhow::anyhow!("libinput: siège {seat}"))?;
    let input = LibinputInputBackend::new(libinput.clone());
    event_loop
        .handle()
        .insert_source(input, |event, _, state| {
            if let InputEvent::DeviceAdded { mut device } = event {
                state.configure_device(&mut device);
                return;
            }
            state.process_input(event);
        })
        .map_err(|e| anyhow::anyhow!("libinput: {e}"))?;

    // Images affichées: l'écran suivant est dessiné au vblank
    let drm_token = event_loop
        .handle()
        .insert_source(drm_notifier, |event, _, state| match event {
            DrmEvent::VBlank(crtc) => state.udev_vblank(crtc),
            DrmEvent::Error(e) => warn!("DRM: {e}"),
        })
        .map_err(|e| anyhow::anyhow!("DRM: {e}"))?;

    // Pause et reprise de la session (changement de console)
    event_loop
        .handle()
        .insert_source(notifier, |event, _, state| state.udev_session(event))
        .map_err(|e| anyhow::anyhow!("session: {e}"))?;

    // Écrans branchés ou débranchés
    let udev = UdevBackend::new(&seat)?;
    event_loop
        .handle()
        .insert_source(udev, |event, _, state| {
            if let UdevEvent::Changed { device_id } = event
                && state.udev_node().is_some_and(|n| n.dev_id() == device_id)
            {
                state.udev_scan();
            }
        })
        .map_err(|e| anyhow::anyhow!("udev: {e}"))?;

    state.backend = Some(Backend::Udev(Box::new(Udev {
        session,
        renderer,
        node,
        drm,
        gbm,
        surfaces: HashMap::new(),
        libinput,
        _drm_token: drm_token,
    })));
    state.udev_scan();
    if state.outputs.is_empty() {
        warn!("aucun écran branché");
    }
    Ok(())
}

/// Nom d'un connecteur (`DP-2`, `HDMI-A-1`)
fn connector_name(info: &connector::Info) -> String {
    format!("{}-{}", info.interface().as_str(), info.interface_id())
}

impl State {
    fn udev(&mut self) -> Option<&mut Udev> {
        match self.backend.as_mut() {
            Some(Backend::Udev(u)) => Some(u),
            _ => None,
        }
    }

    fn udev_node(&self) -> Option<DrmNode> {
        match self.backend.as_ref() {
            Some(Backend::Udev(u)) => Some(u.node),
            _ => None,
        }
    }

    /// Ctrl+Alt+Fn
    pub fn change_vt(&mut self, vt: i32) {
        if let Some(u) = self.udev() {
            u.change_vt(vt);
        }
    }

    /// Compare les connecteurs branchés aux écrans connus: ajoute et retire
    fn udev_scan(&mut self) {
        let Some(Backend::Udev(mut u)) = self.backend.take() else {
            return;
        };
        let res = match u.drm.resource_handles() {
            Ok(r) => r,
            Err(e) => {
                warn!("DRM: {e}");
                self.backend = Some(Backend::Udev(u));
                return;
            }
        };
        let connected: Vec<connector::Info> = res
            .connectors()
            .iter()
            .filter_map(|c| u.drm.get_connector(*c, true).ok())
            .filter(|c| c.state() == connector::State::Connected && !c.modes().is_empty())
            .collect();
        // Débranchés
        let gone: Vec<crtc::Handle> = u
            .surfaces
            .iter()
            .filter(|(_, s)| !connected.iter().any(|c| c.handle() == s.connector))
            .map(|(c, _)| *c)
            .collect();
        for crtc in gone {
            if let Some(s) = u.surfaces.remove(&crtc) {
                info!("écran débranché: {}", s.output.name());
                self.dh.remove_global::<State>(s.global);
                layer_map_for_output(&s.output).cleanup();
                self.outputs.retain(|o| *o != s.output);
            }
        }
        // Branchés
        for info in connected {
            if u.surfaces.values().any(|s| s.connector == info.handle()) {
                continue;
            }
            let used: Vec<crtc::Handle> = u.surfaces.keys().copied().collect();
            let crtc = info
                .encoders()
                .iter()
                .filter_map(|e| u.drm.get_encoder(*e).ok())
                .flat_map(|e| res.filter_crtcs(e.possible_crtcs()))
                .find(|c| !used.contains(c));
            let Some(crtc) = crtc else {
                warn!("{}: aucun CRTC libre", connector_name(&info));
                continue;
            };
            let mode = info
                .modes()
                .iter()
                .find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
                .copied()
                .unwrap_or(info.modes()[0]);
            let surface = match u.drm.create_surface(crtc, mode, &[info.handle()]) {
                Ok(s) => s,
                Err(e) => {
                    warn!("{}: {e}", connector_name(&info));
                    continue;
                }
            };
            let name = connector_name(&info);
            let (w, h) = info.size().unwrap_or((0, 0));
            let output = Output::new(
                name.clone(),
                PhysicalProperties {
                    size: (w as i32, h as i32).into(),
                    subpixel: Subpixel::Unknown,
                    make: "Inconnu".into(),
                    model: name.clone(),
                },
            );
            let global = output.create_global::<State>(&self.dh);
            let omode = OutputMode::from(mode);
            // Les écrans sont posés côte à côte, de gauche à droite
            let x = self
                .outputs
                .iter()
                .map(|o| {
                    let g = self.output_geometry(o);
                    g.loc.x + g.size.w
                })
                .max()
                .unwrap_or(0);
            output.change_current_state(
                Some(omode),
                Some(Transform::Normal),
                Some(smithay::output::Scale::Integer(1)),
                Some((x, 0).into()),
            );
            output.set_preferred(omode);
            let allocator =
                GbmAllocator::new(u.gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
            let exporter = GbmFramebufferExporter::new(u.gbm.clone(), Some(u.node));
            let formats = u.renderer.egl_context().dmabuf_render_formats().clone();
            let compositor = match DrmCompositor::new(
                &output,
                surface,
                None,
                allocator,
                exporter,
                [Fourcc::Argb8888, Fourcc::Xrgb8888],
                formats,
                u.drm.cursor_size(),
                Some(u.gbm.clone()),
            ) {
                Ok(c) => c,
                Err(e) => {
                    warn!("{name}: {e:?}");
                    self.dh.remove_global::<State>(global);
                    continue;
                }
            };
            info!(
                "écran {name}: {}×{} à {} Hz",
                mode.size().0,
                mode.size().1,
                mode.vrefresh()
            );
            self.outputs.push(output.clone());
            u.surfaces.insert(
                crtc,
                Surface {
                    output,
                    global,
                    connector: info.handle(),
                    compositor,
                    modes: info.modes().to_vec(),
                    queued: false,
                    off: false,
                    dirty: true,
                    scheduled: false,
                },
            );
        }
        let crtcs: Vec<crtc::Handle> = u.surfaces.keys().copied().collect();
        self.backend = Some(Backend::Udev(u));
        for o in self.outputs.clone() {
            layer_map_for_output(&o).arrange();
        }
        self.rescue_views();
        for crtc in crtcs {
            self.udev_render(crtc);
        }
    }

    /// Vues sorties de tous les écrans (écran débranché): replacées
    fn rescue_views(&mut self) {
        for i in 0..self.views.len() {
            let f = self.frame_rect(i);
            let center = f.loc.to_f64() + f.size.to_f64().to_point().downscale(2.0);
            let on_screen = self
                .outputs
                .iter()
                .any(|o| self.output_geometry(o).to_f64().contains(center));
            if !on_screen && self.views[i].mapped {
                self.place_new(i);
            }
        }
    }

    fn udev_vblank(&mut self, crtc: crtc::Handle) {
        let Some(u) = self.udev() else { return };
        let Some(s) = u.surfaces.get_mut(&crtc) else {
            return;
        };
        if let Err(e) = s.compositor.frame_submitted() {
            warn!("vblank: {e:?}");
        }
        s.queued = false;
        if s.dirty {
            self.udev_render(crtc);
        }
    }

    /// Quelque chose a changé: les écrans au repos sont redessinés dès que
    /// la boucle d'événements a fini son tour
    pub fn udev_request_redraw(&mut self) {
        let handle = self.handle.clone();
        let Some(u) = self.udev() else { return };
        for (crtc, s) in u.surfaces.iter_mut() {
            s.dirty = true;
            if !s.queued && !s.scheduled && !s.off {
                s.scheduled = true;
                let crtc = *crtc;
                handle.insert_idle(move |state| state.udev_render(crtc));
            }
        }
    }

    /// Dessine un écran; le suivant le sera au vblank s'il a encore changé
    fn udev_render(&mut self, crtc: crtc::Handle) {
        let Some(Backend::Udev(mut u)) = self.backend.take() else {
            return;
        };
        if let Some(s) = u.surfaces.get_mut(&crtc) {
            s.scheduled = false;
        }
        if !u.session.is_active() {
            self.backend = Some(Backend::Udev(u));
            return;
        }
        let Some(output) = u
            .surfaces
            .get(&crtc)
            .filter(|s| !s.queued && !s.off)
            .map(|s| s.output.clone())
        else {
            self.backend = Some(Backend::Udev(u));
            return;
        };
        let mut elements = self.cursor_elements(&mut u.renderer, &output);
        elements.extend(output_elements(self, &mut u.renderer, &output));
        crate::protocols::screencopy::process(self, &mut u.renderer, &output, &elements);
        let animating = self.animating();
        let s = u.surfaces.get_mut(&crtc).unwrap();
        s.dirty = animating;
        let mut queued = false;
        match s.compositor.render_frame(
            &mut u.renderer,
            &elements,
            CLEAR.premultiplied(),
            FrameFlags::DEFAULT,
        ) {
            Ok(frame) if !frame.is_empty => match s.compositor.queue_frame(()) {
                Ok(()) => queued = true,
                // Changement de console: l'accès à l'écran est retiré juste
                // avant la pause de la session
                Err(e) if format!("{e:?}").contains("PermissionDenied") => {
                    debug!("{}: écran inaccessible (changement de console)", output.name())
                }
                Err(e) => warn!("{}: {e:?}", output.name()),
            },
            Ok(_) => {}
            Err(e) => warn!("{}: rendu: {e:?}", output.name()),
        }
        s.queued = queued;
        self.backend = Some(Backend::Udev(u));
        after_render(self, &output);
        if !queued && animating {
            // Animation sans changement visible cette fois: image suivante
            let refresh = output
                .current_mode()
                .map(|m| m.refresh)
                .unwrap_or(60_000)
                .max(1000);
            let _ = self.handle.insert_source(
                Timer::from_duration(Duration::from_micros(1_000_000_000 / refresh as u64)),
                move |_, _, state| {
                    state.udev_render(crtc);
                    TimeoutAction::Drop
                },
            );
        }
    }

    fn udev_session(&mut self, event: SessionEvent) {
        let Some(u) = self.udev() else { return };
        match event {
            SessionEvent::PauseSession => {
                info!("session en pause");
                u.libinput.suspend();
                u.drm.pause();
            }
            SessionEvent::ActivateSession => {
                info!("session reprise");
                if u.libinput.resume().is_err() {
                    warn!("libinput: reprise impossible");
                }
                if let Err(e) = u.drm.activate(false) {
                    warn!("DRM: {e}");
                }
                let mut crtcs = vec![];
                for (crtc, s) in u.surfaces.iter_mut() {
                    if let Err(e) = s.compositor.reset_state() {
                        warn!("{e}");
                    }
                    s.queued = false;
                    s.dirty = true;
                    crtcs.push(*crtc);
                }
                self.udev_scan();
                for c in crtcs {
                    self.udev_render(c);
                }
            }
        }
    }

    /// Réglages `<libinput>` d'un périphérique (§ 5.2): catégorie `touchpad`
    /// pour un pavé tactile, sinon `non-touch`, puis `default`
    fn configure_device(&mut self, device: &mut libinput::Device) {
        let touchpad = device.config_tap_finger_count() > 0;
        let wanted = if touchpad { "touchpad" } else { "non-touch" };
        let mut devs: Vec<&crate::config::InputDevice> = self
            .config
            .input_devices
            .iter()
            .filter(|d| d.category.eq_ignore_ascii_case("default") || d.category.eq_ignore_ascii_case(wanted))
            .collect();
        // Le plus précis en dernier: il l'emporte
        devs.sort_by_key(|d| !d.category.eq_ignore_ascii_case("default"));
        debug!("périphérique: {} (pavé tactile: {touchpad})", device.name());
        for d in devs {
            if let Some(v) = d.tap {
                let _ = device.config_tap_set_enabled(v);
            }
            if let Some(v) = d.natural_scroll {
                let _ = device.config_scroll_set_natural_scroll_enabled(v);
            }
            if let Some(v) = d.middle_emulation {
                let _ = device.config_middle_emulation_set_enabled(v);
            }
            if let Some(v) = d.disable_while_typing {
                let _ = device.config_dwt_set_enabled(v);
            }
            if let Some(v) = d.left_handed {
                let _ = device.config_left_handed_set(v);
            }
            if let Some(v) = d.pointer_speed {
                let _ = device.config_accel_set_speed(v.clamp(-1.0, 1.0));
            }
            match d.accel_profile.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("flat") => {
                    let _ = device.config_accel_set_profile(libinput::AccelProfile::Flat);
                }
                Some("adaptive") => {
                    let _ = device.config_accel_set_profile(libinput::AccelProfile::Adaptive);
                }
                _ => {}
            }
            match d.scroll_method.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("twofinger") => {
                    let _ = device.config_scroll_set_method(libinput::ScrollMethod::TwoFinger);
                }
                Some("edge") => {
                    let _ = device.config_scroll_set_method(libinput::ScrollMethod::Edge);
                }
                Some("none") => {
                    let _ = device.config_scroll_set_method(libinput::ScrollMethod::NoScroll);
                }
                _ => {}
            }
        }
    }
}
