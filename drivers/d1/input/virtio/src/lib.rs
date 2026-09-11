pub mod hardware;
pub mod report_queue;
pub mod state;
use bexos_flatland_input::virtio::RawEvent;
use bexos_graphics_runtime::{self as rt, migration::Runtime};
use bexos_userspace::{Channel, HardwareResourceKind, Memory, Startup, live_migration::Source};
use input_fidl::*;
use state::Input;
pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).expect("input startup");
    let mut runtime = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime<Input>>(
            control,
            start.migration_generation,
        )
        .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let domain = start
            .driver_resources
            .iter()
            .find(|r| r.kind == HardwareResourceKind::IommuDomain)
            .map_or(0, |r| r.handle);
        bexos_virtio_hal::set_iommu_domain(domain);
        let hardware = hardware::Hardware::new(start.arg1).unwrap_or_else(|e| {
            bexos_userspace::log(&format!("virtio-input: initialization failed {e:?}\n"));
            bexos_userspace::exit()
        });
        Startup::ready(control).unwrap();
        bexos_userspace::log(&format!("virtio-input: ready node={}\n", start.arg1));
        Runtime::new(
            control,
            start.migration,
            Input {
                hardware: Some(hardware),
                domain,
                lifecycle: start.driver_lifecycle.map_or(0, |c| c.0),
                reset: true,
                ..Default::default()
            },
        )
    };
    let mut source = Source::new(runtime.migration);
    let mut reports = [RawEvent::default(); 64];
    let mut events = [RawInputEvent {
        kind: 0,
        code: 0,
        value: 0,
    }; 64];
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if runtime.bindings("InputDevice") {
            source.changed(0);
        }
        runtime
            .clients
            .retain(|client| match client.channel.try_recv() {
                Ok(m) => {
                    if let Some((ordinal, bytes)) = rt::envelope(&m.bytes) {
                        if ordinal == 1 && client.allows(ordinal) {
                            let handles: Vec<_> =
                                m.handles.iter().map(|h| HandleRef { raw: *h }).collect();
                            if let Ok(q) = InputDeviceSubscribeRequest::decode(bytes, &handles) {
                                let s = &mut runtime.component;
                                let channel = m.handles.len() == 1
                                    && Memory::object_info(q.sink.raw).is_ok_and(|(kind, _)| {
                                        kind == kernel_fidl::ObjectType::Channel
                                    });
                                let status = if !channel {
                                    Status::Invalid
                                } else if s.hardware.as_ref().is_none_or(|h| !h.healthy) {
                                    Status::Unavailable
                                } else if s.sink.is_none() {
                                    match Memory::duplicate(q.sink.raw, 1 | 2 | 4 | 32) {
                                        Ok(h) => {
                                            s.sink = Some(Channel(h));
                                            s.reset = true;
                                            Status::Ok
                                        }
                                        Err(_) => Status::Invalid,
                                    }
                                } else {
                                    Status::Unavailable
                                };
                                let response = InputDeviceSubscribeResponse {
                                    status,
                                    node: s.hardware.as_ref().map_or(0, |h| h.node),
                                    axes: s.hardware.as_ref().map_or([0; 8], |h| h.axes),
                                };
                                let mut out = [0; 128];
                                let mut hs = [];
                                if let Ok(n) = response.encode(&mut out, &mut hs) {
                                    let _ = client.channel.send(&out[..n.bytes], &[]);
                                }
                                source.changed(1);
                            }
                        }
                    }
                    rt::close(&m.handles);
                    true
                }
                Err(kernel_fidl::Status::ErrPeerClosed) => {
                    let _ = Memory::close(client.channel.0);
                    source.changed(0);
                    false
                }
                Err(_) => true,
            });
        let s = &mut runtime.component;
        let mut stopping = false;
        if s.lifecycle != 0 {
            if let Ok(m) = Channel(s.lifecycle).try_recv() {
                use hardware_manager_fidl::{FidlDecode, FidlEncode};
                let (_, bytes) = rt::envelope(&m.bytes).unwrap_or((0, &m.bytes));
                let request =
                    hardware_manager_fidl::DriverLifecyclePrepareStopRequest::decode(bytes, &[]);
                let status = match request {
                    Ok(request) if m.handles.is_empty() => {
                        if request.reason == hardware_manager_fidl::StopReason::AppdReplacement {
                            hardware_manager_fidl::Status::Ok
                        } else {
                            stopping = true;
                            s.reset = true;
                            if s.hardware.as_mut().is_none_or(|h| h.stop()) {
                                hardware_manager_fidl::Status::Ok
                            } else {
                                hardware_manager_fidl::Status::ErrTimedOut
                            }
                        }
                    }
                    _ => hardware_manager_fidl::Status::ErrInvalidArgs,
                };
                let r = hardware_manager_fidl::DriverLifecyclePrepareStopResponse { status };
                let mut out = [0; 16];
                if let Ok(n) = r.encode(&mut out, &mut []) {
                    let _ = Channel(s.lifecycle).send(&out[..n.bytes], &[]);
                }
                rt::close(&m.handles);
                source.changed(1);
            }
        }
        let count = match s
            .hardware
            .as_mut()
            .filter(|h| h.healthy)
            .map(|h| h.drain(&mut reports))
        {
            Some(Ok(count)) => count,
            Some(Err(_)) => {
                s.reset = true;
                stopping = true;
                0
            }
            None => 0,
        };
        if count != 0 || s.reset {
            if let Some(sink) = s.sink {
                for (out, raw) in events.iter_mut().zip(&reports).take(count) {
                    *out = RawInputEvent {
                        kind: raw.kind,
                        code: raw.code,
                        value: raw.value,
                    };
                }
                let report = InputReport {
                    sequence: s.sequence,
                    reset: s.reset,
                    events: WireVector::from_slice(&events[..count]),
                };
                let mut out = [0; 4096];
                if let Ok(n) = report.encode(&mut out, &mut []) {
                    match sink.send(&out[..n.bytes], &[]) {
                        Ok(()) => {
                            s.sequence = s.sequence.wrapping_add(1);
                            s.reset = false;
                            s.reports += count as u64;
                            if s.resumed {
                                bexos_userspace::log(
                                    "virtio-input: post-transplant reports delivered\n",
                                );
                                s.resumed = false;
                            }
                        }
                        Err(kernel_fidl::Status::ErrPeerClosed) => {
                            let _ = Memory::close(sink.0);
                            s.sink = None;
                            s.reset = true;
                        }
                        Err(_) => s.reset = true,
                    }
                } else {
                    s.reset = true;
                }
            }
            source.changed(1);
        }
        if stopping {
            if let Some(sink) = s.sink.take() {
                let _ = Memory::close(sink.0);
            }
            source.changed(1);
        }
        let mut waiters = [Channel(0); 67];
        let mut len = 0;
        for c in core::iter::once(control)
            .chain(runtime.migration)
            .chain(runtime.clients.iter().map(|c| c.channel))
            .chain((s.lifecycle != 0).then_some(Channel(s.lifecycle)))
        {
            if len < waiters.len() {
                waiters[len] = c;
                len += 1;
            }
        }
        rt::wait(&waiters[..len], rt::now_us().saturating_add(1000));
    }
}
