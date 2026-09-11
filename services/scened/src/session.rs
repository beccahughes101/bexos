use crate::state::Scene;
use bexos_graphics_runtime as rt;
use bexos_userspace::{Memory, service_binding::BoundServiceEndpoint};
use graphics_fidl::*;
pub fn handle(s: &mut Scene, c: &BoundServiceEndpoint, m: bexos_userspace::ipc::Message) {
    let Some((ordinal, bytes)) = rt::envelope(&m.bytes) else {
        rt::close(&m.handles);
        return;
    };
    if !c.allows(ordinal) {
        rt::close(&m.handles);
        return;
    }
    if matches!(ordinal, 25 | 26) {
        crate::shell::request(s, c.channel, ordinal, bytes, &m.handles);
        rt::close(&m.handles);
        return;
    }
    let hs = rt::refs(&m.handles);
    if c.channel.0 == 0 || c.channel.0 > u32::MAX as u64 / 2 {
        rt::close(&m.handles);
        return;
    }
    if ordinal == 16 {
        crate::fences::present(s, c.channel, bytes, &m.handles);
        rt::close(&m.handles);
        return;
    }
    if ordinal == 17 {
        crate::controls::reference(s, c.channel, bytes, &m.handles);
        rt::close(&m.handles);
        return;
    }
    if ordinal == 20 {
        if m.handles.is_empty()
            && FlatlandSessionGetPresentationFeedbackRequest::decode(bytes, &[]).is_ok()
        {
            let queue = s.queues.get(&c.channel.0);
            rt::reply(
                c.channel,
                &FlatlandSessionGetPresentationFeedbackResponse {
                    status: Status::Ok,
                    latched_sequence: queue.map_or(0, |q| q.latched_sequence),
                    presented_sequence: queue.map_or(0, |q| q.presented_sequence),
                    rejected_sequence: queue.map_or(0, |q| q.rejected_sequence),
                    presented_at_ticks: queue.map_or(0, |q| q.presented_at),
                    scene_generation: s.controls.generation,
                },
            );
        }
        rt::close(&m.handles);
        return;
    }
    if ordinal == 15 {
        if FlatlandSessionReadInputRequest::decode(bytes, &hs).is_ok() {
            let mut events = [crate::input::wire(bexos_flatland_input::Event::Reset); 8];
            let mut count = 0;
            if let Some(queue) = s.input.queues.get_mut(&c.channel.0).filter(|_| {
                s.shell.visible(c.channel.0)
                    && (!s.shell.enabled || s.world.paint.iter().any(|p| p.view == c.channel.0))
            }) {
                while count < 8 {
                    let Some(event) = queue.pop() else { break };
                    events[count] = crate::input::wire(event);
                    count += 1;
                }
            }
            if !rt::try_reply(
                c.channel,
                &FlatlandSessionReadInputResponse {
                    status: Status::Ok,
                    events: WireVector::from_slice(&events[..count]),
                },
            ) {
                s.input.reset_view(c.channel.0);
            }
        }
        rt::close(&m.handles);
        return;
    }
    if ordinal == 14 {
        if FlatlandSessionGetPresentationInfoRequest::decode(bytes, &hs).is_ok() {
            let q = s.queues.get(&c.channel.0);
            rt::reply(
                c.channel,
                &FlatlandSessionGetPresentationInfoResponse {
                    status: Status::Ok,
                    accepted_sequence: q.map_or(0, |q| q.sequence),
                    pending_count: q.map_or(0, |q| q.frames.len() as u32),
                    latched_time_ticks: s.sessions.get(&c.channel.0).map_or(0, |s| s.presentation),
                },
            );
        }
        rt::close(&m.handles);
        return;
    }
    if ordinal == 24 {
        if m.handles.is_empty() && FlatlandSessionGetViewportRequest::decode(bytes, &[]).is_ok() {
            let result = crate::controls::geometry(s, c.channel.0);
            rt::reply(
                c.channel,
                &FlatlandSessionGetViewportResponse {
                    status: result
                        .as_ref()
                        .map_or_else(|status| *status, |_| Status::Ok),
                    geometry: result.unwrap_or_else(|_| crate::controls::empty_geometry()),
                },
            );
        }
        rt::close(&m.handles);
        return;
    }
    if !s.sessions.contains_key(&c.channel.0) && s.sessions.len() >= 16 {
        rt::close(&m.handles);
        return;
    }
    let node_count: usize = s.sessions.values().map(|s| s.pending.nodes.len()).sum();
    let available = crate::admission::available(s, c.channel.0);
    let session = s.sessions.entry(c.channel.0).or_default();
    let result: Result<(), bexos_graphics::Error> = (|| {
        let invalid = bexos_graphics::Error::Invalid;
        match ordinal {
            1 => {
                let styles = session
                    .pending
                    .nodes
                    .values()
                    .map(|n| n.style.bytes())
                    .sum();
                if node_count >= 4096
                    || !crate::admission::fits(available, session.pending.nodes.len() + 1, styles)
                {
                    return Err(invalid);
                }
                let q = FlatlandSessionCreateTransformRequest::decode(bytes, &hs)
                    .map_err(|_| invalid)?;
                session.pending.create(q.node_id)
            }
            2 => {
                let q = FlatlandSessionSetRootRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                session.pending.node(q.node_id)?;
                session.pending.root = Some(q.node_id);
                Ok(())
            }
            3 => {
                let q = FlatlandSessionAddChildRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                session.pending.attach(q.parent, q.child)
            }
            4 => {
                let q = FlatlandSessionSetTranslationRequest::decode(bytes, &hs)
                    .map_err(|_| invalid)?;
                session.pending.node(q.node_id)?.translation = (q.x, q.y);
                Ok(())
            }
            5 => {
                let q = FlatlandSessionSetScaleRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                if !q.x.is_finite()
                    || !q.y.is_finite()
                    || q.x <= 0.
                    || q.y <= 0.
                    || q.x > 64.
                    || q.y > 64.
                {
                    return Err(invalid);
                }
                session.pending.node(q.node_id)?.scale = (q.x, q.y);
                Ok(())
            }
            6 => {
                let q =
                    FlatlandSessionSetClipBoundsRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                session.pending.node(q.node_id)?.clip = Some((q.width, q.height));
                Ok(())
            }
            7 => {
                let q =
                    FlatlandSessionSetOpacityRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                if !q.opacity.is_finite() || !(0.0..=1.0).contains(&q.opacity) {
                    return Err(invalid);
                }
                session.pending.node(q.node_id)?.opacity = q.opacity;
                Ok(())
            }
            8 => {
                let q =
                    FlatlandSessionSetContentRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                let surface = rt::surface(q.surface)?;
                let size = surface.validate(u64::MAX)?;
                if s.buffers.len() >= 512
                    || s.buffers
                        .values()
                        .map(|m| m.size)
                        .sum::<u64>()
                        .saturating_add(size as u64)
                        > 256 * 1024 * 1024
                {
                    return Err(invalid);
                }
                let node = session.pending.node(q.node_id)?;
                let duplicate =
                    Memory::duplicate(q.buffer.raw, 1 | 2 | 16 | 32).map_err(|_| invalid)?;
                if duplicate > u32::MAX as u64 / 2 {
                    let _ = Memory::close(duplicate);
                    return Err(invalid);
                }
                let map = rt::Mapping::map(duplicate, size as u64, 2).map_err(|_| invalid)?;
                node.content = Some((map.handle, surface));
                s.buffers.insert(map.handle, map);
                Ok(())
            }
            9 => {
                let q = FlatlandSessionPresentRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                if q.presentation_time_ticks < session.presentation {
                    return Err(invalid);
                }
                let target = s.canvas.as_ref().ok_or(invalid)?.surface;
                let styled = crate::styling::prepare(
                    &mut s.style_cache,
                    c.channel.0,
                    &session.pending,
                    target,
                )?;
                let graph = s
                    .layout_cache
                    .entry(c.channel.0)
                    .or_default()
                    .prepare(&styled, target)?;
                s.queues
                    .entry(c.channel.0)
                    .or_default()
                    .submit(&graph, q.presentation_time_ticks)
                    .map(|_| ())
            }
            10 => {
                let q = FlatlandSessionReleaseTransformRequest::decode(bytes, &hs)
                    .map_err(|_| invalid)?;
                session.pending.remove(q.node_id)
            }
            11 => {
                let q =
                    FlatlandSessionRemoveChildRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                session.pending.detach(q.parent, q.child)
            }
            12 => {
                let q =
                    FlatlandSessionClearContentRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                session.pending.node(q.node_id)?.content = None;
                Ok(())
            }
            13 => {
                let q =
                    FlatlandSessionSetChildOrderRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                session.pending.reorder(q.parent, q.child, q.index)
            }
            23 => {
                if !m.handles.is_empty() {
                    return Err(invalid);
                }
                let q = FlatlandSessionSetStyleRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                let style = bexos_graphics::style::Properties {
                    identifier: q.identifier.into(),
                    classes: q.classes.into(),
                    inline: q.inline_style.into(),
                };
                style.validate()?;
                let node = session.pending.nodes.get(&q.node_id).ok_or(invalid)?;
                let bytes = session
                    .pending
                    .nodes
                    .values()
                    .map(|n| n.style.bytes())
                    .sum::<usize>()
                    - node.style.bytes()
                    + style.bytes();
                if bytes > 32 * 1024
                    || !crate::admission::fits(available, session.pending.nodes.len(), bytes)
                {
                    return Err(invalid);
                }
                session.pending.node(q.node_id)?.style = style;
                Ok(())
            }
            22 => {
                if !m.handles.is_empty() {
                    return Err(invalid);
                }
                let q = FlatlandSessionSetLayoutRequest::decode(bytes, &hs).map_err(|_| invalid)?;
                let layout = bexos_graphics::layout::Properties {
                    mode: q.mode,
                    width: q.width,
                    height: q.height,
                    grow: q.grow,
                    gap: q.gap,
                    padding: q.padding,
                    columns: q.columns,
                };
                layout.validate()?;
                session.pending.node(q.node_id)?.layout = layout;
                Ok(())
            }
            21 => {
                if !m.handles.is_empty() {
                    return Err(invalid);
                }
                let q = FlatlandSessionSetContentEffectsRequest::decode(bytes, &hs)
                    .map_err(|_| invalid)?;
                let effects = bexos_graphics::effects::Effects {
                    corner_radius: q.corner_radius,
                    backdrop_radius: q.backdrop_radius,
                };
                effects.validate()?;
                let node = session.pending.node(q.node_id)?;
                if effects.backdrop_radius != 0 && s.effects.is_none() {
                    let surface = s.canvas.as_ref().ok_or(invalid)?.surface;
                    s.effects = Some(crate::effects::Cache::new(surface)?);
                }
                node.effects = effects;
                Ok(())
            }
            18 => {
                if m.handles.len() != 1 {
                    return Err(invalid);
                }
                let q = FlatlandSessionSetViewContentRequest::decode(bytes, &hs)
                    .map_err(|_| invalid)?;
                let child = s.controls.lookup(q.token.raw).map_err(|_| invalid)?;
                if child == c.channel.0 || !s.shell.can_embed(c.channel.0, child) {
                    return Err(invalid);
                }
                session.pending.node(q.node_id)?.embedded = Some(child);
                Ok(())
            }
            19 => {
                if !m.handles.is_empty() {
                    return Err(invalid);
                }
                let q = FlatlandSessionClearViewContentRequest::decode(bytes, &hs)
                    .map_err(|_| invalid)?;
                session.pending.node(q.node_id)?.embedded = None;
                Ok(())
            }
            _ => Err(invalid),
        }
    })();
    let status = if result.is_ok() {
        Status::Ok
    } else {
        Status::ErrInvalidArgs
    };
    // Every mutation reply has the same explicit Status wire shape.
    rt::reply(c.channel, &FlatlandSessionPresentResponse { status });
    rt::close(&m.handles);
    collect_buffers(s);
}
pub fn collect_buffers(s: &mut Scene) {
    let handles: std::collections::BTreeSet<_> = s
        .sessions
        .values()
        .flat_map(|s| [&s.pending, &s.committed])
        .flat_map(|g| g.nodes.values())
        .filter_map(|n| n.content.map(|(h, _)| h))
        .chain(
            s.queues
                .values()
                .flat_map(|q| &q.frames)
                .flat_map(|p| p.graph.nodes.values())
                .filter_map(|n| n.content.map(|(h, _)| h)),
        )
        .collect();
    s.scanout.client.mark(|key| handles.contains(&key));
    s.buffers.retain(|h, _| {
        let keep = handles.contains(h) || s.scanout.client.retained(*h);
        if !keep {
            s.gpu.forget_buffer(*h);
        }
        keep
    });
}
