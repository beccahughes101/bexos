//! Stable per-session and per-buffer migration streams. Each stream has a
//! length/checksum header and 16 KiB data records; deltas replace only changed
//! chunks. Logical state is reconstructed after the final delta prefix.
use crate::state::{Scene, decode_graph_version, encode_graph};
use bexos_graphics::{
    presentation::{Pending, Queue},
    scene::Session,
};
use bexos_graphics_runtime::{Mapping, migration::Component};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder, checksum},
};
use std::collections::BTreeMap;
const CHUNK: usize = 16 * 1024;
const MAX_STREAM: usize = 512 * 1024;
#[derive(Default)]
pub struct Stream {
    pub bytes: Vec<u8>,
    pub checksum: u64,
    pub chunks: BTreeMap<u32, Vec<u8>>,
}
fn stream_id(handle: u64, session: bool) -> Result<u64, Error> {
    if handle == 0 || handle > u32::MAX as u64 / 2 {
        return Err(Error::Capacity);
    }
    Ok(handle * 2 + u64::from(session))
}
fn session_bytes(s: &Scene, id: u64) -> Vec<u8> {
    let mut w = Encoder::new();
    let session = &s.sessions[&id];
    w.word(session.presentation);
    encode_graph(
        &session.pending,
        &mut w,
        !s.legacy_graphs,
        !s.legacy_effects,
        !s.legacy_styles,
    );
    encode_graph(
        &session.committed,
        &mut w,
        !s.legacy_graphs,
        !s.legacy_effects,
        !s.legacy_styles,
    );
    let empty = Queue::default();
    let q = s.queues.get(&id).unwrap_or(&empty);
    w.word(q.sequence);
    w.word(q.last_time);
    w.word(q.frames.len() as u64);
    for frame in &q.frames {
        w.word(frame.sequence);
        w.word(frame.time);
        encode_graph(
            &frame.graph,
            &mut w,
            !s.legacy_graphs,
            !s.legacy_effects,
            !s.legacy_styles,
        );
    }
    for v in [
        q.latched_sequence,
        q.presented_sequence,
        q.rejected_sequence,
        q.presented_at,
    ] {
        w.word(v);
    }
    w.finish()
}
fn streams(s: &Scene) -> impl Iterator<Item = (u64, Vec<u8>)> + '_ {
    core::iter::once((
        1,
        s.input
            .encode_secure(s.shell.enabled && (s.shell.locked || s.shell.uid == 0)),
    ))
    .chain(
        s.sessions
            .keys()
            .map(|id| (id * 2 + 1, session_bytes(s, *id)))
            .chain(s.buffers.iter().map(|(id, m)| {
                let mut w = Encoder::new();
                m.encode(&mut w);
                (id * 2, w.finish())
            })),
    )
}
pub fn keys(s: &Scene) -> Vec<u64> {
    if s.legacy_records {
        return vec![1];
    }
    let mut keys = if s.legacy_fences { vec![1] } else { vec![1, 2] };
    keys.push(7);
    if !s.legacy_controls {
        keys.push(3);
    }
    if !s.legacy_presentations {
        keys.push(4);
    }
    if !s.scanout.legacy {
        keys.push(5);
    }
    if !s.gpu.legacy {
        keys.push(6);
    }
    for (stream, bytes) in streams(s) {
        keys.extend((0..=bytes.len().div_ceil(CHUNK)).map(|n| (stream << 32) | n as u64));
    }
    keys
}
pub fn encode(s: &Scene, key: u64) -> Result<Option<Vec<u8>>, Error> {
    if key == 5 {
        return s.scanout.encode().map(Some);
    }
    if key == 7 {
        return Ok(Some(s.shell.encode()));
    }
    if key == 6 {
        return Ok(Some(s.gpu.encode()));
    }
    if key == 4 {
        return Ok(Some(crate::presentation::encode_state(s)));
    }
    if key == 3 {
        return Ok(Some(s.controls.encode()));
    }
    if key == 2 {
        return Ok(Some(crate::fences::encode(&s.fences)));
    }
    if key == 1 {
        // Version 6 adds the separately keyed outstanding display submission.
        let mut w = Encoder::new();
        let version = if s.records_version == 0 {
            11
        } else {
            s.records_version
        };
        w.word(version);
        w.word(s.canvas.is_some() as u64);
        if let Some(c) = &s.canvas {
            c.encode(&mut w);
        }
        w.word(s.splash.map_or(0, |c| c.0));
        w.word(s.frozen.is_some() as u64);
        if let Some(m) = &s.frozen {
            m.encode(&mut w);
        }
        w.word(s.ack.map_or(0, |c| c.0));
        w.word(s.start_us);
        w.word(s.clock.next_us);
        w.word(s.ready as u64);
        w.word(0);
        w.word(0); // legacy empty session/buffer lists
        if version >= 11 {
            w.word(s.font_provider.map_or(0, |channel| channel.0));
        }
        return Ok(Some(w.finish()));
    }
    let stream = key >> 32;
    let id = stream >> 1;
    let bytes = if stream == 1 {
        s.input
            .encode_secure(s.shell.enabled && (s.shell.locked || s.shell.uid == 0))
    } else if stream & 1 == 1 {
        if !s.sessions.contains_key(&id) {
            return Ok(None);
        }
        stream_id(id, true)?;
        session_bytes(s, id)
    } else {
        let Some(m) = s.buffers.get(&id) else {
            return Ok(None);
        };
        stream_id(id, false)?;
        let mut w = Encoder::new();
        m.encode(&mut w);
        w.finish()
    };
    let chunk = key as u32 as usize;
    if bytes.len() > MAX_STREAM {
        return Err(Error::Capacity);
    }
    if chunk == 0 {
        let mut w = Encoder::new();
        w.word(bytes.len() as u64);
        w.word(checksum(&bytes));
        return Ok(Some(w.finish()));
    }
    let start = (chunk - 1).checked_mul(CHUNK).ok_or(Error::Capacity)?;
    Ok(bytes
        .get(start..bytes.len().min(start.saturating_add(CHUNK)))
        .filter(|b| !b.is_empty())
        .map(|b| b.to_vec()))
}
pub fn adopt(s: &mut Scene, key: u64, data: Option<&[u8]>) -> Result<(), Error> {
    if key == 7 {
        s.shell = crate::shell::Composition::decode(data.ok_or(Error::InvalidData)?)?;
        return Ok(());
    }
    if key == 6 {
        s.gpu = crate::gpu::Backend::decode(data.ok_or(Error::InvalidData)?)?;
        return Ok(());
    }
    if key == 5 {
        s.scanout = crate::scanout::Scanout::decode(data.ok_or(Error::InvalidData)?)?;
        return Ok(());
    }
    if key == 4 {
        crate::presentation::decode_state(s, data.ok_or(Error::InvalidData)?)?;
        return Ok(());
    }
    if key == 3 {
        let mut controls = crate::controls::Controls::decode(data.ok_or(Error::InvalidData)?)?;
        if controls.display != bexos_graphics::accessibility::DisplayTransform::default()
            || controls
                .pending_display
                .is_some_and(|d| d != Default::default())
        {
            if let Some(canvas) = &s.canvas {
                controls.scratch = match s.controls.scratch.take() {
                    Some(scratch) if scratch.size == canvas.output.size => Some(scratch),
                    _ => Some(
                        bexos_graphics_runtime::Mapping::new(canvas.output.size)
                            .map_err(|_| Error::Capacity)?,
                    ),
                };
            }
        }
        s.controls = controls;
        return Ok(());
    }
    if key == 2 {
        s.fences = crate::fences::decode(data.ok_or(Error::InvalidData)?)?;
        return Ok(());
    }
    if key == 1 {
        let data = data.ok_or(Error::InvalidData)?;
        let mut r = Decoder::new(data);
        let tag = r.word()?;
        if (2..=11).contains(&tag) {
            let scanout = std::mem::take(&mut s.scanout);
            let gpu = std::mem::take(&mut s.gpu);
            let pending_frame = s.pending_frame.take();
            let presentation_encoding = s.presentation_encoding;
            let period_us = s.period_us;
            let takeover = (s.takeover_deadline_us, s.takeover_retry_us);
            let controls = std::mem::take(&mut s.controls);
            let input = std::mem::take(&mut s.input);
            let fences = std::mem::take(&mut s.fences);
            let shell = std::mem::take(&mut s.shell);
            let sessions = std::mem::take(&mut s.sessions);
            let buffers = std::mem::take(&mut s.buffers);
            let queues = std::mem::take(&mut s.queues);
            let records = std::mem::take(&mut s.records);
            s.records_version = tag;
            s.decode(&mut r)?;
            r.finish()?;
            s.records_version = tag;
            s.gpu = gpu;
            s.gpu.legacy = tag < 9;
            s.scanout = scanout;
            s.scanout.legacy = tag < 7;
            s.legacy_effects = tag < 8;
            s.legacy_styles = tag < 10;
            s.input = input;
            s.fences = fences;
            s.shell = shell;
            s.sessions = sessions;
            s.buffers = buffers;
            s.queues = queues;
            s.records = records;
            s.controls = controls;
            s.pending_frame = pending_frame;
            s.presentation_encoding = presentation_encoding;
            s.period_us = period_us;
            (s.takeover_deadline_us, s.takeover_retry_us) = takeover;
            s.legacy_presentations = tag < 6;
            s.legacy_fences = tag == 2;
            s.legacy_controls = tag < 4;
            s.legacy_graphs = tag < 5;
            return Ok(());
        }
        // Version-one sources still send their complete component in record 1.
        let mut r = Decoder::new(data);
        s.decode(&mut r)?;
        r.finish()?;
        s.scanout.legacy = true;
        s.gpu.legacy = true;
        s.legacy_effects = true;
        s.legacy_styles = true;
        s.legacy_presentations = true;
        s.legacy_records = true;
        s.legacy_controls = true;
        s.legacy_graphs = true;
        return Ok(());
    }
    let stream = key >> 32;
    let chunk = key as u32;
    if stream < 1 {
        return Err(Error::InvalidData);
    }
    if data.is_none() {
        if chunk == 0 {
            s.records.remove(&stream);
            if stream == 1 {
                return Err(Error::InvalidData);
            }
            if stream & 1 == 1 {
                s.sessions.remove(&(stream >> 1));
                s.queues.remove(&(stream >> 1));
            } else {
                s.buffers.remove(&(stream >> 1));
            }
        } else if let Some(record) = s.records.get_mut(&stream) {
            record.chunks.remove(&chunk);
        }
        return Ok(());
    }
    let data = data.unwrap();
    let record = s.records.entry(stream).or_default();
    if chunk == 0 {
        let mut r = Decoder::new(data);
        let len = r.count(MAX_STREAM)?;
        if len == 0 {
            return Err(Error::InvalidData);
        }
        record.checksum = r.word()?;
        r.finish()?;
        record.bytes.resize(len, 0);
    } else {
        if chunk as usize > MAX_STREAM.div_ceil(CHUNK) || data.is_empty() || data.len() > CHUNK {
            return Err(Error::Capacity);
        }
        record.chunks.insert(chunk, data.to_vec());
    }
    if s.records
        .values()
        .map(|r| r.bytes.len() + r.chunks.values().map(Vec::len).sum::<usize>())
        .sum::<usize>()
        > 8 * 1024 * 1024
    {
        return Err(Error::Capacity);
    }
    Ok(())
}
pub fn finish(s: &mut Scene) -> Result<(), Error> {
    for (stream, record) in &mut s.records {
        let count = record.bytes.len().div_ceil(CHUNK);
        if count == 0 || record.chunks.len() != count {
            return Err(Error::InvalidData);
        }
        for n in 1..=count {
            let start = (n - 1) * CHUNK;
            let end = (start + CHUNK).min(record.bytes.len());
            let chunk = record.chunks.get(&(n as u32)).ok_or(Error::InvalidData)?;
            if chunk.len() != end - start {
                return Err(Error::InvalidData);
            }
            record.bytes[start..end].copy_from_slice(chunk);
        }
        if checksum(&record.bytes) != record.checksum {
            return Err(Error::InvalidData);
        }
        if *stream == 1 {
            s.input = crate::input::Input::decode(&record.bytes)?;
            continue;
        }
        let id = stream >> 1;
        let mut r = Decoder::new(&record.bytes);
        if stream & 1 == 1 {
            let session = Session {
                presentation: r.word()?,
                pending: decode_graph_version(
                    &mut r,
                    !s.legacy_graphs,
                    !s.legacy_effects,
                    !s.legacy_styles,
                )?,
                committed: decode_graph_version(
                    &mut r,
                    !s.legacy_graphs,
                    !s.legacy_effects,
                    !s.legacy_styles,
                )?,
            };
            let mut q = Queue {
                sequence: r.word()?,
                last_time: r.word()?,
                ..Default::default()
            };
            for _ in 0..r.count(bexos_graphics::presentation::MAX_QUEUED_PRESENTS)? {
                q.frames.push_back(Pending {
                    sequence: r.word()?,
                    time: r.word()?,
                    graph: decode_graph_version(
                        &mut r,
                        !s.legacy_graphs,
                        !s.legacy_effects,
                        !s.legacy_styles,
                    )?,
                });
            }
            if !s.legacy_graphs {
                q.latched_sequence = r.word()?;
                q.presented_sequence = r.word()?;
                q.rejected_sequence = r.word()?;
                q.presented_at = r.word()?;
            }
            r.finish()?;
            q.validate().map_err(|_| Error::InvalidData)?;
            s.sessions.insert(id, session);
            s.queues.insert(id, q);
        } else {
            let mapping = Mapping::decode(&mut r)?;
            r.finish()?;
            if mapping.handle != id {
                return Err(Error::InvalidData);
            }
            s.buffers.insert(id, mapping);
        }
    }
    if s.legacy_fences && !s.fences.frames.is_empty() {
        return Err(Error::InvalidData);
    }
    s.fences.validate().map_err(|_| Error::InvalidData)?;
    for ((view, sequence), frame) in &s.fences.frames {
        let queue = s.queues.get(view).ok_or(Error::InvalidData)?;
        if !s.sessions.contains_key(view) || *sequence > queue.sequence {
            return Err(Error::InvalidData);
        }
        if frame.stage == bexos_graphics::synchronization::Stage::Committed {
            if queue.frames.iter().any(|p| p.sequence <= *sequence) {
                return Err(Error::InvalidData);
            }
        } else if !queue.frames.iter().any(|p| p.sequence == *sequence) {
            return Err(Error::InvalidData);
        }
    }
    s.validate()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::{Composition, Owner};

    #[test]
    fn global_record_adoption_preserves_shell_composition() {
        let mut scene = Scene {
            shell: Composition {
                enabled: true,
                sysui: "bexos.app.sysui".into(),
                userui: "bexos.app.userui".into(),
                uid: 1000,
                epoch: 4,
                owners: [(
                    7,
                    Owner {
                        package: "bexos.app.sysui".into(),
                        uid: 0,
                        role: 1,
                        epoch: 0,
                    },
                )]
                .into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let bytes = encode(&scene, 1).unwrap().unwrap();

        adopt(&mut scene, 1, Some(&bytes)).unwrap();

        assert!(scene.shell.enabled);
        assert_eq!(scene.shell.uid, 1000);
        assert_eq!(scene.shell.root(), Some(7));
    }
}
