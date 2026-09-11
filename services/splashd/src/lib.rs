mod handover;
pub mod state;
use bexos_graphics_runtime::{self as rt, canvas::Canvas, migration::Runtime};
use bexos_userspace::{Channel, Memory, Startup, live_migration::Source};
use graphics_fidl::*;
use state::Splash;
pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).expect("splash startup");
    let mut runtime = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime<Splash>>(
            control,
            start.migration_generation,
        )
        .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let display = start
            .service_grants
            .iter()
            .find(|g| g.protocol == "DisplayCoordinator")
            .map(|g| Channel(g.endpoint));
        let canvas = display.and_then(|c| Canvas::connect(c, true).ok());
        let firmware = if start.resources.len() == 2 {
            let descriptor = rt::Mapping::map(start.resources[1], 4096, 2).ok();
            descriptor.and_then(|d| {
                let words: Vec<_> = d.bytes()[..48]
                    .chunks_exact(8)
                    .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                let surface = bexos_graphics::Surface {
                    width: words[2].try_into().ok()?,
                    height: words[3].try_into().ok()?,
                    stride: words[4].try_into().ok()?,
                    format: (words[5] as u32).try_into().ok()?,
                };
                surface.validate(words[1]).ok()?;
                let map = rt::Mapping::map(start.resources[0], words[1], 6).ok()?;
                Some((map, surface))
            })
        } else {
            None
        };
        if let Err(error) = rt::scheduling::request(control) {
            bexos_userspace::log(&format!("graphics: profile request failed {error:?}\n"));
        }
        Startup::ready(control).unwrap();
        bexos_userspace::log("splashd: ready\n");
        Runtime::new(
            control,
            start.migration,
            Splash {
                canvas,
                firmware,
                ..Default::default()
            },
        )
    };
    let control = runtime.control;
    let mut source = Source::new(runtime.migration);
    loop {
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if runtime.bindings("ProgressTracker") {
            source.changed(0);
        }
        runtime.clients.retain(|c| match c.channel.try_recv() {
            Ok(m) => {
                handle(&mut runtime.component, c, m);
                source.changed(1);
                true
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                source.changed(0);
                let _ = Memory::close(c.channel.0);
                false
            }
            Err(_) => true,
        });
        let s = &mut runtime.component;
        if let Some(ack) = s.ack {
            let message = ack.try_recv();
            let confirmed = message.as_ref().is_ok_and(|m| {
                m.handles.is_empty() && m.bytes == s.handoff_generation.to_le_bytes()
            });
            if confirmed {
                if let Some(c) = &mut s.canvas {
                    let info: Result<DisplayCoordinatorGetInfoResponse, _> =
                        rt::call(&mut c.display, 1, &DisplayCoordinatorGetInfoRequest {});
                    if info.is_ok_and(|i| {
                        i.status == Status::Ok
                            && i.generation == s.handoff_generation
                            && !i.transfer_pending
                            && i.owner_id != c.client_id
                    }) {
                        finish(runtime);
                    }
                }
            }
            if let Ok(m) = &message {
                rt::close(&m.handles);
            }
            if rt::now_us() >= s.deadline
                || matches!(message, Err(kernel_fidl::Status::ErrPeerClosed))
            {
                if let Some(c) = &mut s.canvas {
                    let response: Result<DisplayCoordinatorRollbackResponse, _> = rt::call(
                        &mut c.display,
                        6,
                        &DisplayCoordinatorRollbackRequest {
                            generation: s.handoff_generation,
                        },
                    );
                    if let Ok(r) = response {
                        if r.status == Status::Ok {
                            c.generation = r.generation;
                            s.frozen = false;
                        }
                    }
                }
                let _ = Memory::close(ack.0);
                s.ack = None;
                // A timed-out owner never resumes writes without a successful rollback.
                if s.frozen {
                    if let Some(c) = &mut s.canvas {
                        let info: Result<DisplayCoordinatorGetInfoResponse, _> =
                            rt::call(&mut c.display, 1, &DisplayCoordinatorGetInfoRequest {});
                        if let Ok(i) = info {
                            if i.status == Status::Ok
                                && !i.transfer_pending
                                && i.generation == s.handoff_generation
                                && i.owner_id != 0
                                && i.owner_id != c.client_id
                            {
                                // The driver completion fence also resolves a lost acknowledgement.
                                finish(runtime);
                            }
                            if i.status == Status::Ok
                                && i.owner_id == c.client_id
                                && !i.transfer_pending
                            {
                                c.generation = i.generation;
                                s.frozen = false;
                            }
                        }
                    }
                    if s.frozen {
                        s.canvas = None;
                        s.frozen = false;
                    }
                }
                source.changed(1);
            }
        }
        let now = rt::now_us();
        if !s.frozen && s.clock.due(now) {
            if s.canvas.is_none() {
                if let Some((m, surface)) = &mut s.firmware {
                    if s.renderer.is_none() {
                        s.renderer = bexos_graphics::render::Renderer::new(*surface).ok();
                    }
                    if let Some(r) = &mut s.renderer {
                        let pixels = r.splash(now, s.progress.percent);
                        let _ = surface.copy_rgba(pixels, m.bytes_mut(), surface.full());
                    }
                }
            }
            if let Some(c) = &mut s.canvas {
                if s.renderer.is_none() {
                    s.renderer = bexos_graphics::render::Renderer::new(c.surface).ok();
                }
                if let Some(r) = &mut s.renderer {
                    let begin = rt::now_us();
                    let pixels = r.splash(now, s.progress.percent);
                    if c.surface
                        .copy_rgba(pixels, c.output.bytes_mut(), c.surface.full())
                        .is_ok()
                    {
                        let display_was_open = c.display.0 != 0;
                        let presented = c.present();
                        if display_was_open && presented.is_err() {
                            bexos_userspace::log(&format!(
                                "splashd: presentation failed {:?}; display_open={}\n",
                                presented.err().unwrap(),
                                c.display.0 != 0
                            ));
                        }
                        if presented.is_ok() {
                            s.firmware = None;
                            s.frames += 1;
                            if s.resumed {
                                bexos_userspace::log(
                                    "splashd: post-transplant animation presented\n",
                                );
                                s.resumed = false;
                            }
                            if s.frames == 1 {
                                bexos_userspace::log("splashd: first frame presented\n");
                            }
                            s.render_us += rt::now_us().saturating_sub(begin);
                        }
                    }
                }
            }
            source.changed(1);
        }
        let mut waiters = vec![control];
        waiters.extend(runtime.migration);
        waiters.extend(runtime.clients.iter().map(|c| c.channel));
        waiters.extend(runtime.component.ack);
        let now = rt::now_us();
        let frame = runtime.component.clock.next_us;
        rt::wait(
            &waiters,
            if frame > now {
                frame
            } else {
                now.saturating_add(bexos_graphics::progress::FrameClock::PERIOD_US)
            },
        );
    }
}
fn finish(runtime: Runtime<Splash>) -> ! {
    bexos_userspace::log(&format!(
        "splashd: handoff complete frames={} render_us={}\n",
        runtime.component.frames, runtime.component.render_us
    ));
    let _ = runtime.control.send(b"bexos.graphics.completed", &[]);
    drop(runtime);
    bexos_userspace::exit()
}

fn handle(
    s: &mut Splash,
    c: &bexos_userspace::service_binding::BoundServiceEndpoint,
    m: bexos_userspace::ipc::Message,
) {
    let Some((ordinal, bytes)) = rt::envelope(&m.bytes) else {
        rt::close(&m.handles);
        return;
    };
    if !c.allows(ordinal) {
        rt::close(&m.handles);
        return;
    }
    let hs = rt::refs(&m.handles);
    match ordinal {
        1 => {
            let status = match ProgressTrackerReportStageRequest::decode(bytes, &hs) {
                Ok(q) => {
                    if s.progress
                        .report(q.stage as u8, q.progress_pct, &q.status_message)
                        .is_ok()
                    {
                        bexos_userspace::log(&format!(
                            "splashd: stage={} percent={}\n",
                            s.progress.stage, s.progress.percent
                        ));
                        Status::Ok
                    } else {
                        Status::ErrInvalidArgs
                    }
                }
                Err(_) => Status::ErrInvalidArgs,
            };
            rt::reply(c.channel, &ProgressTrackerReportStageResponse { status });
        }
        2 => {
            let response = match handover::begin(s, bytes, &hs) {
                Ok(frame) => {
                    rt::reply(
                        c.channel,
                        &ProgressTrackerHandoverToCompositorResponse {
                            status: Status::Ok,
                            frame: Some(frame),
                        },
                    );
                    return;
                }
                Err(status) => ProgressTrackerHandoverToCompositorResponse {
                    status,
                    frame: None,
                },
            };
            bexos_userspace::log(&format!(
                "splashd: handover deferred status={:?} stage={} frozen={}\n",
                response.status, s.progress.stage, s.frozen
            ));
            rt::reply(c.channel, &response);
        }
        _ => {}
    }
    rt::close(&m.handles);
}
