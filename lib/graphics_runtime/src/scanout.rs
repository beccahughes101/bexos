//! Bounded, nonblocking imported-buffer client. The display endpoint carries one
//! control request or presentation at a time. Retirement uses separate channels.
use crate::{canvas::Canvas, now_us, presentation::Submission, stream::read_no_handles, wire};
use bexos_graphics::Surface;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{Channel, Memory, live_migration::Resource};
use graphics_fidl::*;
const LIMIT: usize = 16;
#[derive(Clone, Copy, Debug)]
pub struct Lease {
    pub sequence: u64,
    pub channel: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub key: u64,
    pub resource: u32,
    pub surface: Surface,
    pub wanted: bool,
    pub lease: Option<Lease>,
}
#[derive(Clone, Copy, Debug)]
enum Operation {
    Capabilities,
    Import { key: u64, surface: Surface },
    Release { resource: u32 },
}
#[derive(Clone, Copy, Debug)]
struct Pending {
    operation: Operation,
    deadline: u64,
}
pub struct Client {
    pub formats: Option<u32>,
    pub entries: Vec<Entry>,
    pub sequence: u64,
    pending: Option<Pending>,
}
impl Default for Client {
    fn default() -> Self {
        Self {
            formats: None,
            entries: Vec::with_capacity(LIMIT),
            sequence: 0,
            pending: None,
        }
    }
}
fn send<Q: FidlEncode>(
    channel: Channel,
    ordinal: u64,
    request: &Q,
    handles: &[u64],
) -> Result<(), Status> {
    let mut bytes = [0; 256];
    let mut refs = [HandleRef { raw: 0 }; 2];
    let result = request
        .encode(&mut bytes[8..], &mut refs)
        .map_err(|_| Status::ErrInvalidArgs)
        .and_then(|n| {
            bytes[..8].copy_from_slice(&ordinal.to_le_bytes());
            channel
                .send(&bytes[..8 + n.bytes], handles)
                .map_err(|_| Status::ErrIo)
        });
    if result.is_err() {
        crate::close(handles);
    }
    result
}
impl Client {
    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub fn retained(&self, key: u64) -> bool {
        self.entries.iter().any(|e| e.key == key)
            || self.pending.is_some_and(
                |p| matches!(p.operation, Operation::Import { key: k, .. } if k == key),
            )
    }
    pub fn lease(&self) -> Option<u64> {
        self.entries
            .iter()
            .filter_map(|e| e.lease.map(|l| l.sequence))
            .max()
    }
    pub fn has_lease(&self, sequence: u64) -> bool {
        self.entries
            .iter()
            .any(|e| e.lease.is_some_and(|l| l.sequence == sequence))
    }
    /// Marked from logical scene and presentation-queue references, before the
    /// caller retains mappings needed solely by this imported-resource cache.
    pub fn mark(&mut self, mut wanted: impl FnMut(u64) -> bool) {
        for e in &mut self.entries {
            e.wanted = wanted(e.key);
        }
    }
    pub fn poll(&mut self, canvas: &mut Canvas, now: u64) -> bool {
        let mut changed = false;
        for e in &mut self.entries {
            let Some(lease) = e.lease else {
                continue;
            };
            let mut bytes = [0; crate::stream::ENVELOPE_BYTES + 64];
            match read_no_handles(Channel(lease.channel), &mut bytes) {
                Ok(bytes) => {
                    // A peer close or malformed message is not proof of retirement.
                    if PresentationRelease::decode(bytes, &[])
                        .is_ok_and(|r| r.sequence == lease.sequence)
                    {
                        let _ = Memory::close(lease.channel);
                        e.lease = None;
                        changed = true;
                    }
                }
                Err(_) => {}
            }
        }
        let Some(pending) = self.pending else {
            return changed;
        };
        let mut bytes = [0; crate::stream::ENVELOPE_BYTES + 128];
        let received = match read_no_handles(canvas.display, &mut bytes) {
            Ok(bytes) => bytes,
            Err(kernel_fidl::Status::ErrTimedOut) if now < pending.deadline => return changed,
            Err(_) => {
                let _ = Memory::close(canvas.display.0);
                canvas.display.0 = 0;
                self.pending = None;
                self.formats = Some(0);
                return true;
            }
        };
        self.pending = None;
        let valid = match pending.operation {
            Operation::Capabilities => {
                DisplayCoordinatorGetScanoutCapabilitiesResponse::decode(received, &[]).map(|r| {
                    self.formats = Some(if r.status == Status::Ok { r.formats } else { 0 });
                })
            }
            Operation::Import { key, surface } => {
                DisplayCoordinatorImportScanoutResponse::decode(received, &[]).map(|r| {
                    if r.status == Status::Ok
                        && r.resource >= 3
                        && !self
                            .entries
                            .iter()
                            .any(|e| e.resource == r.resource || e.key == key)
                    {
                        self.entries.push(Entry {
                            key,
                            surface,
                            resource: r.resource,
                            wanted: true,
                            lease: None,
                        });
                    } else {
                        // A rejected import falls back to composition. The endpoint
                        // remains usable; repeated attempts cannot starve frames.
                        self.formats = Some(0);
                    }
                })
            }
            Operation::Release { resource } => {
                DisplayCoordinatorReleaseScanoutResponse::decode(received, &[]).map(|r| {
                    if r.status == Status::Ok {
                        self.entries.retain(|e| e.resource != resource);
                    }
                })
            }
        };
        if valid.is_err() {
            let _ = Memory::close(canvas.display.0);
            canvas.display.0 = 0;
            self.formats = Some(0);
        }
        true
    }
    fn begin(&mut self, operation: Operation) {
        self.pending = Some(Pending {
            operation,
            deadline: now_us().saturating_add(2_000_000),
        });
    }
    pub fn discover(&mut self, canvas: &Canvas) -> Result<(), Status> {
        if self.busy() {
            return Err(Status::ErrBusy);
        }
        send(
            canvas.display,
            16,
            &DisplayCoordinatorGetScanoutCapabilitiesRequest {},
            &[],
        )?;
        self.begin(Operation::Capabilities);
        Ok(())
    }
    pub fn import(&mut self, canvas: &Canvas, key: u64, surface: Surface) -> Result<(), Status> {
        if self.busy() || self.entries.len() == LIMIT {
            return Err(Status::ErrBusy);
        }
        let handle =
            Memory::duplicate(key, 1 | 2 | 16 | 32).map_err(|_| Status::ErrAccessDenied)?;
        send(
            canvas.display,
            17,
            &DisplayCoordinatorImportScanoutRequest {
                generation: canvas.generation,
                buffer: HandleRef { raw: handle },
                surface: wire(surface),
            },
            &[handle],
        )?;
        self.begin(Operation::Import { key, surface });
        Ok(())
    }
    pub fn prune(&mut self, canvas: &Canvas) -> Result<bool, Status> {
        if self.busy() {
            return Ok(false);
        }
        let Some(resource) = self
            .entries
            .iter()
            .find(|e| !e.wanted && e.lease.is_none())
            .map(|e| e.resource)
        else {
            return Ok(false);
        };
        send(
            canvas.display,
            19,
            &DisplayCoordinatorReleaseScanoutRequest {
                generation: canvas.generation,
                resource,
            },
            &[],
        )?;
        self.begin(Operation::Release { resource });
        Ok(true)
    }
    pub fn present(&mut self, canvas: &Canvas, key: u64) -> Result<Submission, Status> {
        if self.busy() {
            return Err(Status::ErrBusy);
        }
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.key == key)
            .ok_or(Status::ErrInvalidArgs)?;
        if e.lease.is_some() {
            return Err(Status::ErrBusy);
        }
        let sequence = self.sequence.checked_add(1).ok_or(Status::ErrBusy)?;
        let (local, remote) = Channel::pair().map_err(|_| Status::ErrNoMemory)?;
        let result = send(
            canvas.display,
            18,
            &DisplayCoordinatorPresentScanoutRequest {
                generation: canvas.generation,
                resource: e.resource,
                sequence,
                release: HandleRef { raw: remote.0 },
                damage: Damage {
                    x: 0,
                    y: 0,
                    width: e.surface.width,
                    height: e.surface.height,
                },
            },
            &[remote.0],
        );
        if let Err(e) = result {
            let _ = Memory::close(local.0);
            return Err(e);
        }
        self.sequence = sequence;
        e.lease = Some(Lease {
            sequence,
            channel: local.0,
        });
        Ok(Submission {
            deadline_us: now_us().saturating_add(2_000_000),
        })
    }
    pub fn resources(&self) -> Vec<Resource> {
        self.entries
            .iter()
            .filter_map(|e| e.lease.map(|l| Resource::Handle(l.channel)))
            .collect()
    }
    pub fn encode(&self, w: &mut Encoder) -> Result<(), Error> {
        // Pre-copy may observe an outstanding control exchange. Its final delta
        // is drained before quiescence, but the logical request remains decodable.
        w.word(1);
        w.word(self.formats.is_some() as u64);
        w.word(self.formats.unwrap_or(0) as u64);
        w.word(self.sequence);
        w.word(self.entries.len() as u64);
        for e in &self.entries {
            for v in [
                e.key,
                e.resource as u64,
                e.surface.width as u64,
                e.surface.height as u64,
                e.surface.stride as u64,
                e.surface.format as u64,
                e.wanted as u64,
                e.lease.map_or(0, |l| l.sequence),
                e.lease.map_or(0, |l| l.channel),
            ] {
                w.word(v);
            }
        }
        w.word(self.pending.is_some() as u64);
        if let Some(p) = self.pending {
            w.word(p.deadline);
            match p.operation {
                Operation::Capabilities => w.word(0),
                Operation::Release { resource } => {
                    w.word(1);
                    w.word(resource as u64);
                }
                Operation::Import { key, surface } => {
                    w.word(2);
                    for v in [
                        key,
                        surface.width as u64,
                        surface.height as u64,
                        surface.stride as u64,
                        surface.format as u64,
                    ] {
                        w.word(v);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn decode(r: &mut Decoder<'_>) -> Result<Self, Error> {
        fn u32word(r: &mut Decoder<'_>) -> Result<u32, Error> {
            r.word()?.try_into().map_err(|_| Error::InvalidData)
        }
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let has_formats = r.flag()?;
        let formats = u32word(r)?;
        let mut s = Self {
            formats: has_formats.then_some(formats),
            sequence: r.word()?,
            ..Self::default()
        };
        for _ in 0..r.count(LIMIT)? {
            let mut e = Entry {
                key: r.word()?,
                resource: u32word(r)?,
                surface: Surface {
                    width: u32word(r)?,
                    height: u32word(r)?,
                    stride: u32word(r)?,
                    format: u32word(r)?.try_into().map_err(|_| Error::InvalidData)?,
                },
                wanted: r.flag()?,
                lease: None,
            };
            let sequence = r.word()?;
            let channel = r.word()?;
            if e.key == 0
                || e.resource < 3
                || e.surface.validate(u64::MAX).is_err()
                || !e.surface.format.is_opaque()
                || e.surface.stride != e.surface.width * 4
                || (sequence == 0) != (channel == 0)
                || sequence > s.sequence
                || s.entries.iter().any(|v| {
                    v.key == e.key
                        || v.resource == e.resource
                        || v.lease
                            .is_some_and(|l| l.channel == channel || l.sequence == sequence)
                })
            {
                return Err(Error::InvalidData);
            }
            if channel != 0 {
                e.lease = Some(Lease { sequence, channel });
            }
            s.entries.push(e);
        }
        if r.flag()? {
            let deadline = r.word()?;
            let operation = match r.word()? {
                0 => Operation::Capabilities,
                1 => {
                    let resource = u32word(r)?;
                    if !s
                        .entries
                        .iter()
                        .any(|e| e.resource == resource && e.lease.is_none())
                    {
                        return Err(Error::InvalidData);
                    }
                    Operation::Release { resource }
                }
                2 => {
                    let key = r.word()?;
                    let surface = Surface {
                        width: u32word(r)?,
                        height: u32word(r)?,
                        stride: u32word(r)?,
                        format: u32word(r)?.try_into().map_err(|_| Error::InvalidData)?,
                    };
                    if key == 0
                        || surface.validate(u64::MAX).is_err()
                        || !surface.format.is_opaque()
                        || s.entries.len() == LIMIT
                        || s.entries.iter().any(|e| e.key == key)
                    {
                        return Err(Error::InvalidData);
                    }
                    Operation::Import { key, surface }
                }
                _ => return Err(Error::InvalidData),
            };
            if deadline == 0 {
                return Err(Error::InvalidData);
            }
            s.pending = Some(Pending {
                operation,
                deadline,
            });
        }
        Ok(s)
    }
}
