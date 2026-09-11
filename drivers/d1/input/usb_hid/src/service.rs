use crate::{HidKind, Runtime, parser, wire};
use alloc::vec::Vec;
use bexos_usb_host::hid::{BootKeyboardReport, BootMouseReport};
use bexos_userspace::{
    Channel, HardwareResourceKind, Memory, Startup,
    live_migration::Source,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use input_fidl::*;

pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).unwrap_or_else(|_| bexos_userspace::exit());
    let state = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime>(control, start.migration_generation)
            .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let mut runtime = Runtime::new(control, start.migration);
        runtime.lifecycle = start.driver_lifecycle;
        runtime.interface = start
            .driver_resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::BusControl)
            .map(|resource| Channel(resource.handle));
        runtime.node = start.arg1;
        runtime.kind = match start
            .driver_resources
            .iter()
            .find(|resource| resource.kind == HardwareResourceKind::BusControl)
            .map(|resource| resource.base)
            .unwrap_or(0)
        {
            2 => HidKind::Mouse,
            _ => HidKind::Keyboard,
        };
        if runtime.interface.is_none() {
            bexos_userspace::log("usb-hid: missing scoped interface channel\n");
            bexos_userspace::exit();
        }
        Startup::ready(control).unwrap();
        runtime
    };
    serve(state).await
}

async fn serve(mut state: Runtime) -> ! {
    let control = state.control;
    let mut source = Source::new(state.migration);
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            state.paused = true;
            bexos_userspace_async::yield_once().await;
            continue;
        }
        poll_control(control, &mut state, &mut source);
        poll_clients(&mut state, &mut source);
        bexos_userspace_async::yield_once().await;
    }
}

fn poll_control(control: Channel, state: &mut Runtime, source: &mut Source) {
    let Ok(message) = control.try_recv() else {
        return;
    };
    if let (Some(endpoint), Ok(metadata)) = (
        message.handles.first().copied(),
        core::str::from_utf8(&message.bytes),
    ) {
        if let Some(binding) = ServiceBinding::parse(metadata) {
            if binding.protocol_is("InputDevice") {
                state.clients.push(BoundServiceEndpoint::new(
                    Channel(endpoint),
                    binding.method_ordinals,
                ));
                source.changed(1);
                return;
            }
        }
        let _ = Memory::close(endpoint);
        return;
    }
    wire::close(&message.handles);
}

fn poll_clients(state: &mut Runtime, source: &mut Source) {
    let mut index = 0;
    while index < state.clients.len() {
        let keep = match state.clients[index].channel.try_recv() {
            Ok(message) => {
                handle_client(index, state, message.bytes, message.handles);
                source.changed(0);
                true
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => false,
            Err(_) => true,
        };
        if keep {
            index += 1;
        } else {
            let channel = state.clients.remove(index).channel;
            let _ = Memory::close(channel.0);
            source.changed(1);
        }
    }
}

fn handle_client(index: usize, state: &mut Runtime, bytes: Vec<u8>, handles: Vec<u64>) {
    let channel = state.clients[index].channel;
    let Some((ordinal, request)) = wire::envelope(&bytes) else {
        wire::close(&handles);
        return;
    };
    let refs = wire::refs(&handles);
    if ordinal == 1 && state.clients[index].allows(ordinal) {
        let status = match InputDeviceSubscribeRequest::decode(request, &refs) {
            Ok(q) if handles.len() == 1 => {
                if state.sink.is_none() {
                    state.sink = Some(Channel(q.sink.raw));
                    state.reset = true;
                    Status::Ok
                } else {
                    let _ = Memory::close(q.sink.raw);
                    Status::Unavailable
                }
            }
            _ => {
                wire::close(&handles);
                Status::Invalid
            }
        };
        wire::reply(
            channel,
            &InputDeviceSubscribeResponse {
                status,
                node: state.node,
                axes: [0; 8],
            },
        );
    } else {
        wire::close(&handles);
    }
}

pub fn deliver_keyboard(state: &mut Runtime, report: BootKeyboardReport) {
    let batch = parser::report_events(
        state.held_keyboard,
        Some(report),
        BootMouseReport::default(),
        None,
    );
    state.held_keyboard = report;
    deliver_batch(state, &batch);
}

pub fn deliver_mouse(state: &mut Runtime, report: BootMouseReport) {
    let batch = parser::report_events(
        BootKeyboardReport::default(),
        None,
        state.held_mouse,
        Some(report),
    );
    state.held_mouse = report;
    deliver_batch(state, &batch);
}

fn deliver_batch(state: &mut Runtime, batch: &parser::ReportBatch) {
    let Some(sink) = state.sink else {
        return;
    };
    let report = InputReport {
        sequence: state.sequence,
        reset: state.reset,
        events: WireVector::from_slice(&batch.events[..batch.count]),
    };
    let mut out = [0; 2048];
    if let Ok(encoded) = report.encode(&mut out, &mut []) {
        if sink.send(&out[..encoded.bytes], &[]).is_ok() {
            state.sequence = state.sequence.wrapping_add(1);
            state.reports = state.reports.wrapping_add(1);
            state.reset = false;
        }
    }
}
