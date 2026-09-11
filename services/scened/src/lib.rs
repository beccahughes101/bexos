pub mod admission;
pub mod controls;
mod controls_migration;
mod damage;
pub mod desktop;
pub mod desktop_config;
pub mod effects;
pub mod fences;
pub mod gpu;
pub mod input;
pub mod input_config;
mod input_link;
pub mod metrics;
mod metrics_json;
pub mod presentation;
mod render;
pub mod scanout;
mod world;
use render::{render, takeover};
mod records;
mod session;
pub mod shell;
pub mod state;
pub mod styling;
use bexos_graphics_runtime::{self as rt, canvas::Canvas, migration::Runtime};
use bexos_userspace::{Channel, Memory, Startup, live_migration::Source};
use state::Scene;
pub async fn main(channel: u64) -> ! {
    let control = Channel(channel);
    let start = Startup::receive(control).expect("scened startup");
    bexos_userspace::log("scened: startup received\n");
    let mut runtime = if start.migration_target {
        bexos_userspace::live_migration::receive::<Runtime<Scene>>(
            control,
            start.migration_generation,
        )
        .unwrap_or_else(|_| bexos_userspace::exit())
    } else {
        let initialization_started = rt::now_us();
        let configuration =
            desktop_config::DesktopConfig::decode(include_bytes!(env!("DESKTOP_CONFIG")))
                .expect("scened desktop configuration");
        let period_us = configuration.frame_period_us();
        let display = start
            .service_grants
            .iter()
            .find(|g| g.protocol == "DisplayCoordinator" && g.method_ordinals.contains(&1))
            .map(|g| Channel(g.endpoint));
        let gpu_endpoint = start
            .service_grants
            .iter()
            .find(|g| g.protocol == "DisplayCoordinator" && g.method_ordinals.contains(&10))
            .map_or(0, |g| g.endpoint);
        let splash = start
            .service_grants
            .iter()
            .find(|g| g.protocol == "ProgressTracker")
            .map(|g| Channel(g.endpoint));
        let mut input = input::Input::default();
        input.settings = input_config::decode(include_bytes!(env!("INPUT_CONFIG")))
            .expect("scened input policy");
        for grant in start
            .service_grants
            .iter()
            .filter(|g| g.protocol == "InputDevice")
        {
            if let Err(e) = input.queue_control(Channel(grant.endpoint)) {
                let _ = Memory::close(grant.endpoint);
                bexos_userspace::log(&format!("scened: input grant queue failed {e:?}\n"));
            }
        }
        bexos_userspace::log(&format!(
            "scened: input discovery complete elapsed_us={}\n",
            rt::now_us().saturating_sub(initialization_started),
        ));
        bexos_userspace::log(&format!(
            "scened: display connection deferred available={} elapsed_us={}\n",
            display.is_some(),
            rt::now_us().saturating_sub(initialization_started),
        ));
        bexos_userspace::log(&format!(
            "scened: desktop initialization complete elapsed_us={}\n",
            rt::now_us().saturating_sub(initialization_started),
        ));
        if let Err(error) = rt::scheduling::request_period(control, period_us * 1000) {
            bexos_userspace::log(&format!("graphics: profile request failed {error:?}\n"));
        }
        Startup::ready(control).unwrap();
        bexos_userspace::log("scened: ready for boot completion\n");
        bexos_userspace::log(&format!(
            "scened: {} Hz timer fallback; hardware VSYNC unavailable\n",
            configuration.refresh_hz
        ));
        Runtime::new(
            control,
            start.migration,
            Scene {
                shell: shell::Composition {
                    enabled: !configuration.legacy_standalone,
                    ..Default::default()
                },
                period_us,
                canvas: None,
                pending_display: display,
                gpu: gpu::Backend::new(gpu_endpoint),
                desktop: None,
                input,
                splash,
                ..Default::default()
            },
        )
    };
    let control = runtime.control;
    let mut source = Source::new(runtime.migration);
    let mut changes = bexos_userspace::live_migration::RecordChanges::default();
    let mut waiters = Vec::with_capacity(20);
    loop {
        let loop_sample = metrics::Sample::capture();
        changes.poll(&runtime, &mut source);
        if source.poll(&runtime).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            runtime.component.metrics.end_loop(loop_sample);
            continue;
        }
        // appd is the sole holder of the bootstrap manager endpoint.
        if let Ok(m) = control.try_recv() {
            if m.bytes == b"bexos.graphics.ready" && m.handles.is_empty() {
                runtime.component.ready = true;
            } else if m.bytes.starts_with(b"bexos.shell.configure\0") {
                shell::configure(&mut runtime.component, &m.bytes[22..], &m.handles);
            } else if m.handles.len() == 1 {
                if let Ok(text) = core::str::from_utf8(&m.bytes) {
                    if let Some(b) = bexos_userspace::service_binding::ServiceBinding::parse(text) {
                        if b.service == "bexos.hardware.input.InputDevice"
                            && b.protocol_is("InputDevice")
                            && b.allows(1)
                        {
                            if runtime
                                .component
                                .input
                                .connect(Channel(m.handles[0]))
                                .is_err()
                            {
                                rt::close(&m.handles);
                            }
                            source.changed(1);
                            runtime.component.metrics.end_loop(loop_sample);
                            continue;
                        }
                        if b.protocol_is("FlatlandSession") && runtime.clients.len() < 16 {
                            runtime.component.controls.register(m.handles[0]);
                            shell::register(&mut runtime.component, m.handles[0], &b);
                            runtime.clients.push(
                                bexos_userspace::service_binding::BoundServiceEndpoint::new(
                                    Channel(m.handles[0]),
                                    b.method_ordinals,
                                ),
                            );
                            source.changed(0);
                            runtime.component.metrics.end_loop(loop_sample);
                            continue;
                        }
                        let controls = &mut runtime.component.controls;
                        let clients = if b.protocol_is("AccessibilityControl") {
                            Some(&mut controls.accessibility)
                        } else if b.protocol_is("ShellControl") {
                            Some(&mut controls.shell)
                        } else {
                            None
                        };
                        if let Some(clients) = clients {
                            if clients.len() < 2 {
                                clients.push(
                                    bexos_userspace::service_binding::BoundServiceEndpoint::new(
                                        Channel(m.handles[0]),
                                        b.method_ordinals,
                                    ),
                                );
                                source.changed(3);
                                runtime.component.metrics.end_loop(loop_sample);
                                continue;
                            }
                        }
                    }
                }
                rt::close(&m.handles);
            } else {
                rt::close(&m.handles);
            }
            source.changed(1);
        }
        if runtime.component.ready && runtime.component.canvas.is_none() {
            if let Some(display) = runtime.component.pending_display.take() {
                let start = rt::now_us();
                bexos_userspace::log("scened: display connection begin\n");
                runtime.component.canvas = Canvas::connect(display, false).ok();
                bexos_userspace::log(&format!(
                    "scened: display connection complete available={} elapsed_us={}\n",
                    runtime.component.canvas.is_some(),
                    rt::now_us().saturating_sub(start),
                ));
                runtime.component.desktop = runtime.component.canvas.as_ref().and_then(|c| {
                    match desktop::Desktop::new(c.surface) {
                        Ok(cache) => {
                            bexos_userspace::log(
                                "scened: Parley/Noto text and Taffy desktop cache ready\n",
                            );
                            Some(cache)
                        }
                        Err(error) => {
                            bexos_userspace::log(&format!(
                                "scened: desktop text cache failed {error:?}\n"
                            ));
                            None
                        }
                    }
                });
                runtime.component.dirty = runtime.component.canvas.is_some();
                source.changed(1);
            }
        }
        if runtime.component.ready && !runtime.component.input.pending_controls.is_empty() {
            match runtime.component.input.bind_one_pending() {
                Ok(true) => {
                    source.changed(1);
                }
                Ok(false) => {}
                Err(error) => {
                    bexos_userspace::log(&format!("scened: input binding failed {error:?}\n"));
                    source.changed(1);
                }
            }
        }
        runtime.clients.retain(|c| match c.channel.try_recv() {
            Ok(m) => {
                session::handle(&mut runtime.component, c, m);
                source.changed(1);
                true
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {
                if let Some(bounds) = runtime
                    .component
                    .snapshots
                    .get(&c.channel.0)
                    .and_then(|s| s.bounds())
                {
                    runtime.component.damage =
                        Some(runtime.component.damage.map_or(bounds, |d| d.union(bounds)));
                }
                let was_root = runtime.component.shell.role(c.channel.0) == 1;
                runtime.component.shell.owners.remove(&c.channel.0);
                if was_root {
                    runtime.component.shell.locked = true;
                    for owner in runtime
                        .component
                        .sessions
                        .keys()
                        .copied()
                        .collect::<Vec<_>>()
                    {
                        runtime.component.input.reset_view(owner);
                    }
                    runtime.component.input.router.focused = None;
                }
                runtime.component.input.remove_view(c.channel.0);
                runtime.component.controls.disconnected(c.channel.0);
                runtime.component.sessions.remove(&c.channel.0);
                runtime.component.layout_cache.remove(&c.channel.0);
                runtime.component.style_cache.remove(&c.channel.0);
                runtime.component.queues.remove(&c.channel.0);
                runtime.component.snapshots.remove(&c.channel.0);
                runtime.component.snapshot_changes.remove(&c.channel.0);
                runtime.component.dirty = true;
                session::collect_buffers(&mut runtime.component);
                fences::disconnect(&mut runtime.component, c.channel.0);
                source.changed(0);
                source.changed(1);
                let _ = Memory::close(c.channel.0);
                false
            }
            Err(_) => true,
        });
        let s = &mut runtime.component;
        controls::poll(s);
        fences::poll(s);
        let now = rt::now_us();
        presentation::poll(s, now);
        scanout::poll(s, now);
        gpu::poll(s, source.draining());
        if s.ready && s.canvas.is_some() && s.start_us == 0 && now >= s.takeover_retry_us {
            if s.takeover_deadline_us == 0 {
                s.takeover_deadline_us = now.saturating_add(2_000_000);
            }
            if let Err(error) = takeover(s) {
                let recoverable = matches!(
                    error,
                    graphics_fidl::Status::ErrBusy
                        | graphics_fidl::Status::ErrTimedOut
                        | graphics_fidl::Status::ErrIo
                );
                if recoverable && rt::now_us() < s.takeover_deadline_us {
                    s.takeover_retry_us = rt::now_us()
                        .saturating_add(50_000)
                        .min(s.takeover_deadline_us);
                    s.dirty = true;
                    bexos_userspace::log(&format!("scened: takeover deferred {error:?}\n"));
                } else {
                    s.ready = false;
                    bexos_userspace::log(&format!(
                        "scened: takeover failed {error:?}; retained splash\n"
                    ));
                }
            } else {
                s.takeover_deadline_us = 0;
                s.takeover_retry_us = 0;
            }
            source.changed(1);
        }
        let period_us = s.frame_period_us();
        let scheduled_us = s.clock.next_us;
        if !source.draining()
            && s.start_us != 0
            && !s.gpu.blocks_frames()
            && s.pending_frame.is_none()
            && s.clock.due_with_period(now, period_us)
        {
            let started_us = rt::now_us();
            if s.controls.latch() {
                s.full_damage = true;
                s.dirty = true;
                s.damage = None;
            }
            let ticks = bexos_userspace::syscall::ticks();
            let latched = world::latch(s, ticks);
            let geometry_changed =
                latched || s.resumed || s.world_generation != s.controls.generation;
            render(s, started_us);
            if s.metrics.completed >= 32 && (s.pending_frame.is_some() || s.gpu.pending.is_some()) {
                s.metrics.dispatch_lateness_us.record(if scheduled_us == 0 {
                    0
                } else {
                    started_us.saturating_sub(scheduled_us)
                });
            }
            if geometry_changed {
                s.input.reconcile(&s.snapshots);
            }
            source.changed(1);
        }
        if let Some(canvas) = &s.canvas {
            if s.shell.enabled && (s.shell.locked || s.shell.uid == 0) {
                if let Some(root) = s
                    .shell
                    .root()
                    .filter(|id| s.world.paint.iter().any(|p| p.view == *id))
                {
                    if s.input.router.focused != Some(root) {
                        s.input.focus(root);
                    }
                }
            }
            s.input.shell_views = s
                .shell
                .owners
                .keys()
                .filter(|id| s.shell.role(**id) != 0)
                .copied()
                .collect();
            s.input.poll(
                &s.snapshots,
                canvas.surface.width as f64,
                canvas.surface.height as f64,
                now,
                &s.controls.stacking,
                s.controls.display,
                &s.world,
            );
            if s.shell.enabled && !s.shell.locked {
                if let Some(id) = s.input.router.focused.filter(|id| {
                    s.shell.role(*id) == 0
                        && s.shell
                            .owners
                            .get(id)
                            .is_some_and(|o| o.uid == s.shell.uid && o.uid != 0)
                }) {
                    s.shell.user_focus = id;
                }
            }
        }
        waiters.clear();
        waiters.push(control);
        waiters.extend(runtime.migration);
        waiters.extend(runtime.clients.iter().map(|c| c.channel));
        waiters.extend(runtime.component.ack);
        waiters.extend(runtime.component.gpu.wakeup());
        if runtime.component.pending_frame.is_some() || runtime.component.scanout.client.busy() {
            waiters.extend(runtime.component.canvas.as_ref().map(|c| c.display));
        }
        waiters.extend(
            runtime
                .component
                .controls
                .accessibility
                .iter()
                .chain(&runtime.component.controls.shell)
                .map(|c| c.channel),
        );
        waiters.extend(runtime.component.input.devices.values().map(|l| {
            if l.subscription_deadline.is_some() {
                l.control
            } else {
                l.reports
            }
        }));
        waiters.extend(runtime.component.queues.iter().filter_map(|(view, queue)| {
            let front = queue.frames.front()?;
            runtime
                .component
                .fences
                .frames
                .get(&(*view, front.sequence))?
                .acquire
                .map(Channel)
        }));
        let now = rt::now_us();
        let frame = runtime.component.clock.next_us;
        runtime.component.metrics.end_loop(loop_sample);
        runtime.component.metrics.report();
        rt::wait(
            &waiters,
            if frame > now {
                frame
            } else {
                now.saturating_add(runtime.component.frame_period_us())
            },
        );
    }
}
