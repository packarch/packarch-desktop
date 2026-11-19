// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pointeur virtuel (cahier des charges § 9.2) : wlr-virtual-pointer v2,
//! pour wlrctl, ydotool et les outils d'automatisation. Les événements
//! suivent le chemin de la souris (liaisons, focus, contraintes). Réservé
//! aux clients de confiance.

use std::sync::Mutex;

use smithay::backend::input::{Axis, AxisSource};
use smithay::input::pointer::AxisFrame;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_pointer;
use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, WEnum,
};
use smithay::utils::{Logical, Point, Rectangle};
use wayland_protocols_wlr::virtual_pointer::v1::server::{
    zwlr_virtual_pointer_manager_v1::{self, ZwlrVirtualPointerManagerV1},
    zwlr_virtual_pointer_v1::{self, ZwlrVirtualPointerV1},
};

use crate::state::State;

pub fn init(dh: &DisplayHandle) {
    dh.create_global::<State, ZwlrVirtualPointerManagerV1, _>(2, ());
}

/// Un pointeur virtuel : son écran (mouvements absolus) et la molette en
/// attente de `frame`
#[derive(Default)]
pub struct VirtualPointer {
    output: Option<Output>,
    axis: Mutex<Option<(AxisFrame, (f64, f64))>>,
}

fn axis_of(axis: WEnum<wl_pointer::Axis>) -> Option<Axis> {
    match axis {
        WEnum::Value(wl_pointer::Axis::VerticalScroll) => Some(Axis::Vertical),
        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => Some(Axis::Horizontal),
        _ => None,
    }
}

/// Zone des mouvements absolus : l'écran du pointeur, sinon tous les écrans
fn area(state: &State, vp: &VirtualPointer) -> Option<Rectangle<f64, Logical>> {
    if let Some(o) = vp.output.as_ref().filter(|o| state.outputs.contains(o)) {
        return Some(state.output_geometry(o).to_f64());
    }
    state
        .outputs
        .iter()
        .map(|o| state.output_geometry(o))
        .reduce(|a, b| a.merge(b))
        .map(|r| r.to_f64())
}

impl GlobalDispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn bind(
        _state: &mut State,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrVirtualPointerManagerV1>,
        _data: &(),
        data_init: &mut DataInit<'_, State>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _data: &()) -> bool {
        super::trusted(&client)
    }
}

impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for State {
    fn request(
        _state: &mut State,
        _client: &Client,
        _resource: &ZwlrVirtualPointerManagerV1,
        request: zwlr_virtual_pointer_manager_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, State>,
    ) {
        // Un seul siège : celui demandé est ignoré
        match request {
            zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointer { id, .. } => {
                data_init.init(id, VirtualPointer::default());
            }
            zwlr_virtual_pointer_manager_v1::Request::CreateVirtualPointerWithOutput {
                id, output, ..
            } => {
                let output = output.as_ref().and_then(Output::from_resource);
                data_init.init(
                    id,
                    VirtualPointer {
                        output,
                        ..Default::default()
                    },
                );
            }
            zwlr_virtual_pointer_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<ZwlrVirtualPointerV1, VirtualPointer> for State {
    fn request(
        state: &mut State,
        _client: &Client,
        _resource: &ZwlrVirtualPointerV1,
        request: zwlr_virtual_pointer_v1::Request,
        vp: &VirtualPointer,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, State>,
    ) {
        use zwlr_virtual_pointer_v1::Request;
        // Molette : accumulée jusqu'à `frame`
        let with_axis = |time: u32, f: &mut dyn FnMut(&mut AxisFrame, &mut (f64, f64))| {
            let mut pending = vp.axis.lock().unwrap();
            let (frame, steps) = pending.get_or_insert_with(|| (AxisFrame::new(time), (0.0, 0.0)));
            if frame.time == 0 {
                frame.time = time;
            }
            f(frame, steps);
        };
        match request {
            Request::Motion { time, dx, dy } => {
                let delta = Point::from((dx, dy));
                state.pointer_relative(delta, delta, time as u64 * 1000);
            }
            Request::MotionAbsolute {
                time,
                x,
                y,
                x_extent,
                y_extent,
            } => {
                if x_extent == 0 || y_extent == 0 {
                    return;
                }
                let Some(r) = area(state, vp) else {
                    return;
                };
                let pos = Point::from((
                    r.loc.x + r.size.w * x.min(x_extent) as f64 / x_extent as f64,
                    r.loc.y + r.size.h * y.min(y_extent) as f64 / y_extent as f64,
                ));
                state.pointer_motion(pos, time);
            }
            Request::Button {
                time,
                button,
                state: s,
            } => {
                let pressed = s == WEnum::Value(wl_pointer::ButtonState::Pressed);
                state.pointer_button(button, pressed, time);
            }
            Request::Axis { time, axis, value } => {
                if let Some(axis) = axis_of(axis) {
                    with_axis(time, &mut |frame, steps| {
                        *pick(&mut frame.axis, axis) += value;
                        *pick(steps, axis) += value / 15.0;
                    });
                }
            }
            Request::AxisDiscrete {
                time,
                axis,
                value,
                discrete,
            } => {
                if let Some(axis) = axis_of(axis) {
                    with_axis(time, &mut |frame, steps| {
                        *pick(&mut frame.axis, axis) += value;
                        *pick(frame.v120.get_or_insert((0, 0)), axis) += discrete * 120;
                        *pick(steps, axis) += discrete as f64;
                    });
                }
            }
            Request::AxisSource { axis_source } => {
                let source = match axis_source {
                    WEnum::Value(wl_pointer::AxisSource::Finger) => AxisSource::Finger,
                    WEnum::Value(wl_pointer::AxisSource::Continuous) => AxisSource::Continuous,
                    WEnum::Value(wl_pointer::AxisSource::WheelTilt) => AxisSource::WheelTilt,
                    _ => AxisSource::Wheel,
                };
                with_axis(0, &mut |frame, _| frame.source = Some(source));
            }
            Request::AxisStop { time, axis } => {
                if let Some(axis) = axis_of(axis) {
                    with_axis(time, &mut |frame, _| *pick(&mut frame.stop, axis) = true);
                }
            }
            Request::Frame => {
                let pending = vp.axis.lock().unwrap().take();
                if let Some((frame, steps)) = pending {
                    state.pointer_axis(frame, steps);
                }
            }
            Request::Destroy => {}
            _ => {}
        }
    }
}

/// Composante d'un couple (horizontal, vertical)
fn pick<T>(pair: &mut (T, T), axis: Axis) -> &mut T {
    match axis {
        Axis::Horizontal => &mut pair.0,
        Axis::Vertical => &mut pair.1,
    }
}
