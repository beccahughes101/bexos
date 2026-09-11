use crate::state::Scene;
use bexos_graphics_runtime as rt;
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;
pub(crate) fn takeover(s: &mut Scene) -> Result<(), Status> {
    s.dirty = true;
    s.damage = None;
    let c = s.canvas.as_mut().ok_or(Status::ErrIo)?;
    if let Some(splash) = s.splash.as_mut() {
        let (local, remote) = Channel::pair().map_err(|_| Status::ErrIo)?;
        let result = (|| {
            let response: ProgressTrackerHandoverToCompositorResponse = rt::call_owned(
                splash,
                2,
                &ProgressTrackerHandoverToCompositorRequest {
                    next_client: c.client_id,
                    ack_channel: HandleRef { raw: remote.0 },
                },
                &[remote.0],
            )?;
            if response.status != Status::Ok {
                if let Some(frame) = response.frame {
                    let _ = Memory::close(frame.buffer.raw);
                }
                return Err(response.status);
            }
            let response = response.frame.ok_or(Status::ErrInvalidArgs)?;
            if rt::surface(response.surface).map_err(|_| Status::ErrInvalidArgs)? != c.surface {
                rt::close(&[response.buffer.raw]);
                return Err(Status::ErrInvalidArgs);
            }
            let frozen = rt::Mapping::map(response.buffer.raw, c.output.size, 2)
                .map_err(|_| Status::ErrIo)?;
            c.generation = response.generation;
            c.output.bytes_mut().copy_from_slice(frozen.bytes());
            c.present()?;
            if let Err(error) = local.send(&response.generation.to_le_bytes(), &[]) {
                bexos_userspace::log(&format!("scened: splash handoff ack skipped {error:?}\n"));
            }
            s.frozen = Some(frozen);
            s.start_us = rt::now_us().max(1);
            bexos_userspace::log("scened: splash frame presented; takeover acknowledged\n");
            Ok(())
        })();
        let _ = Memory::close(local.0);
        result?;
    } else {
        let response: DisplayCoordinatorAcquireResponse =
            rt::call(&mut c.display, 2, &DisplayCoordinatorAcquireRequest {})?;
        if response.status != Status::Ok {
            return Err(response.status);
        }
        c.generation = response.generation;
        s.start_us = rt::now_us().max(1);
    }
    Ok(())
}
pub(crate) fn render(s: &mut Scene, now: u64) {
    if !s.dirty && s.frozen.is_none() && !s.resumed {
        return;
    }
    let Some(surface) = s.canvas.as_ref().map(|c| c.surface) else {
        return;
    };
    let display = s.controls.display;
    let transformed = display != bexos_graphics::accessibility::DisplayTransform::default();
    let force_full = s.full_damage
        || transformed
        || s.controls.ring.is_some()
        || s.frozen.is_some()
        || s.resumed
        || s.snapshots.is_empty();
    if s.world_generation != s.controls.generation
        || !s.snapshot_changes.is_empty()
        || s.resumed
        || s.snapshots.is_empty()
    {
        if crate::world::compile(s, surface).is_err() {
            return;
        }
    }
    if force_full {
        s.damage = Some(surface.full());
    }
    if crate::scanout::render(s, now) {
        return;
    }
    if crate::gpu::render(s, now) {
        return;
    }
    s.gpu.invalidate_outputs();
    let c = s.canvas.as_mut().unwrap();
    let mut damage = if force_full {
        c.surface.full()
    } else {
        s.damage.unwrap_or(c.surface.full())
    };
    let halo = s
        .world
        .items(&s.snapshots)
        .map(|(_, item)| item.effects.backdrop_radius)
        .fold(0u32, u32::saturating_add)
        .min(c.surface.width.max(c.surface.height));
    if halo != 0 {
        damage = bexos_graphics::effects::expand(damage, halo, c.surface);
    }
    let work = bexos_graphics::effects::expand(damage, halo, c.surface);
    {
        let (output, mut blur): (&mut [u8], Option<&mut bexos_graphics::blur::BoxBlur>) =
            if halo != 0 {
                let Some(cache) = &mut s.effects else {
                    return;
                };
                (&mut cache.pixels, Some(&mut cache.blur))
            } else if transformed {
                let Some(scratch) = &mut s.controls.scratch else {
                    return;
                };
                (scratch.bytes_mut(), None)
            } else {
                (c.output.bytes_mut(), None)
            };
        if let Some(frozen) = &s.frozen {
            let _ = bexos_graphics::render::cross_fade(
                frozen.bytes(),
                output,
                now.saturating_sub(s.start_us),
            );
        } else {
            let stride = c.surface.stride as usize;
            for y in work.y..work.y + work.height {
                let begin = y as usize * stride + work.x as usize * 4;
                let end = begin + work.width as usize * 4;
                for p in output[begin..end].chunks_exact_mut(4) {
                    p.copy_from_slice(&[14, 18, 28, 255]);
                }
            }
            if let Some(desktop) = &s.desktop {
                if desktop.compose(c.surface, output, work).is_err() {
                    return;
                }
            }
        }
        for (_, item) in s.world.items(&s.snapshots) {
            if item.effects.backdrop_radius != 0 {
                if let Some(bounds) = bexos_graphics::effects::intersect(item.pixels, work) {
                    let Some(blur) = blur.as_deref_mut() else {
                        return;
                    };
                    if blur
                        .apply_masked(
                            c.surface,
                            output,
                            bounds,
                            item.effects.backdrop_radius,
                            |x, y| item.contains(x as f64 + 0.5, y as f64 + 0.5),
                        )
                        .is_err()
                    {
                        return;
                    }
                }
            }
            if bexos_graphics::composition::composite_items(
                core::iter::once(item),
                c.surface,
                output,
                work,
                |h| s.buffers.get(&h).map(|m| m.bytes()),
            )
            .is_err()
            {
                return;
            }
        }
        if let Some(ring) = s.controls.ring {
            if s.controls
                .views
                .get(&ring.view)
                .is_some_and(|v| v.generation == ring.generation)
            {
                if let Some((transform, clip, true)) = s
                    .sessions
                    .get(&ring.view)
                    .and_then(|session| session.committed.root)
                    .and_then(|root| s.snapshots.get(&ring.view)?.geometry(root))
                {
                    let bounds = bexos_graphics::resolved::Rect {
                        x: transform.x + ring.bounds.x * transform.sx,
                        y: transform.y + ring.bounds.y * transform.sy,
                        width: ring.bounds.width * transform.sx,
                        height: ring.bounds.height * transform.sy,
                    };
                    let _ = bexos_graphics::accessibility::focus_ring(
                        c.surface, output, bounds, clip, ring.width, ring.rgba,
                    );
                }
            }
        }
    }
    if halo != 0 {
        let output = &s.effects.as_ref().unwrap().pixels;
        let destination = if transformed {
            let Some(scratch) = &mut s.controls.scratch else {
                return;
            };
            scratch.bytes_mut()
        } else {
            c.output.bytes_mut()
        };
        for y in damage.y..damage.y + damage.height {
            let start = (y * c.surface.stride + damage.x * 4) as usize;
            let end = start + damage.width as usize * 4;
            destination[start..end].copy_from_slice(&output[start..end]);
        }
    }
    if transformed
        && display
            .apply(
                c.surface,
                s.controls.scratch.as_ref().unwrap().bytes(),
                c.output.bytes_mut(),
                damage,
            )
            .is_err()
    {
        return;
    }
    if let Ok(submission) = rt::presentation::Submission::begin(c, damage) {
        s.metrics.pending_cpu_bytes = damage.width as u64 * damage.height as u64 * 4;
        let frame = crate::presentation::PendingFrame::new(s, submission, now, 0);
        s.pending_frame = Some(frame);
        s.full_damage = false;
        s.dirty = false;
        s.damage = None;
    }
}
