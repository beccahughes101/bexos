use super::codec::{read_entry, word32, write_entry};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_wasm_runtime::{
    resources::{Entry, Handle},
    wasi::{
        checkpoint::Saved,
        output_state::{OutputState, Target},
        sockets::*,
        state::*,
        wasi::{filesystem::types::*, sockets::network::*},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, atomic::AtomicBool},
};
type Table = BTreeMap<u32, Saved>;
fn visit(value: &Saved, mut f: impl FnMut(&dyn Handle)) {
    match value {
        Saved::Descriptor(d) => f(&*d.entry.handle),
        Saved::Input(Input::File(e, _)) => f(&*e.handle),
        Saved::Input(Input::Socket(h))
        | Saved::Pollable(Pollable::Network(h, _, _))
        | Saved::Pollable(Pollable::Socket(h, _)) => f(&**h),
        Saved::Output(output) | Saved::Pollable(Pollable::Output(output)) => {
            match output.snapshot().target {
                Target::File(entry, _) => f(&*entry.handle),
                Target::Socket(handle) => f(&*handle),
                _ => {}
            }
        }
        Saved::Network(Network(e)) => {
            if let Some(e) = e {
                f(&*e.handle)
            }
        }
        Saved::Tcp(t) => {
            for e in [&t.network, &t.endpoint].into_iter().flatten() {
                f(&*e.handle)
            }
        }
        Saved::Udp(u) => {
            if let Some(e) = &u.endpoint {
                f(&*e.handle)
            }
        }
        Saved::Incoming(i) => f(&*i.entry.handle),
        Saved::Outgoing(o) => f(&*o.entry.handle),
        _ => {}
    }
}
pub fn handles(table: &Table, out: &mut BTreeSet<u64>) {
    for value in table.values() {
        visit(value, |h| {
            out.insert(h.native());
            out.extend(h.companions());
        });
    }
}
pub fn estimate(table: &Table) -> Result<usize, Error> {
    let mut size = 8usize;
    for value in table.values() {
        let mut n = 1024usize;
        visit(value, |h| {
            n = n.saturating_add(
                512 + h.allowed_methods().map_or(0, |m| m.len() * 8)
                    + super::grant_codec::estimate(h.grant()),
            );
        });
        match value {
            Saved::DirectoryStream(d) => {
                for e in &d.entries {
                    n = n.saturating_add(16 + e.name.len());
                }
            }
            Saved::Output(output) | Saved::Pollable(Pollable::Output(output)) => {
                let state = output.snapshot();
                n = n.saturating_add(
                    state.pending.len() + state.failure.as_ref().map_or(0, |error| error.len()),
                );
            }
            Saved::IoError(e) => n = n.saturating_add(e.0.len()),
            Saved::Addresses(a) => n = n.saturating_add(a.0.len() * 80),
            _ => {}
        }
        size = size.checked_add(n).ok_or(Error::Capacity)?;
    }
    Ok(size)
}
fn entry(e: &mut Encoder, v: &Entry) {
    write_entry(e, v)
}
fn optional_entry(e: &mut Encoder, v: &Option<Entry>) {
    e.word(v.is_some() as u64);
    if let Some(v) = v {
        entry(e, v)
    }
}
fn handle(e: &mut Encoder, h: &Arc<dyn Handle>) {
    entry(
        e,
        &Entry {
            name: String::new(),
            handle: h.clone(),
        },
    )
}
fn write_output(e: &mut Encoder, output: &Output, seen: &mut BTreeSet<u64>) {
    e.word(output.id());
    let first = seen.insert(output.id());
    e.word(first as u64);
    if !first {
        return;
    }
    let state = output.snapshot();
    match state.target {
        Target::Closed => e.word(0),
        Target::Log => e.word(1),
        Target::File(file, position) => {
            e.word(2);
            entry(e, &file);
            e.word(position);
        }
        Target::Socket(socket) => {
            e.word(3);
            handle(e, &socket);
        }
    }
    e.bytes(&state.pending);
    e.word(state.permit as u64);
    e.word(state.flushing as u64);
    e.word(state.failure.is_some() as u64);
    if let Some(error) = state.failure {
        e.bytes(error.as_bytes());
    }
}
fn ip(e: &mut Encoder, v: &IpAddress) {
    match v {
        IpAddress::Ipv4(a) => {
            e.word(4);
            e.bytes(&[a.0, a.1, a.2, a.3]);
        }
        IpAddress::Ipv6(a) => {
            e.word(6);
            for n in [a.0, a.1, a.2, a.3, a.4, a.5, a.6, a.7] {
                e.word(n as u64)
            }
        }
    }
}
fn address(e: &mut Encoder, v: &Option<IpSocketAddress>) {
    e.word(v.is_some() as u64);
    if let Some(v) = v {
        match v {
            IpSocketAddress::Ipv4(a) => {
                ip(e, &IpAddress::Ipv4(a.address));
                e.word(a.port as u64);
            }
            IpSocketAddress::Ipv6(a) => {
                ip(e, &IpAddress::Ipv6(a.address));
                e.word(a.port as u64);
                e.word(a.flow_info as u64);
                e.word(a.scope_id as u64);
            }
        }
    }
}
fn family(e: &mut Encoder, f: IpAddressFamily) {
    e.word(match f {
        IpAddressFamily::Ipv4 => 4,
        IpAddressFamily::Ipv6 => 6,
    })
}
pub fn write_table(e: &mut Encoder, table: &Table) -> Result<(), Error> {
    let mut outputs = BTreeSet::new();
    e.word(table.len() as u64);
    for (id, v) in table {
        e.word(*id as u64);
        match v {
            Saved::Descriptor(d) => {
                e.word(1);
                entry(e, &d.entry);
                e.word(encode_flags(d.flags));
            }
            Saved::Input(i) => {
                e.word(2);
                match i {
                    Input::Empty => e.word(0),
                    Input::File(h, p) => {
                        e.word(1);
                        entry(e, h);
                        e.word(*p)
                    }
                    Input::Socket(h) => {
                        e.word(2);
                        handle(e, h)
                    }
                }
            }
            Saved::Output(o) => {
                e.word(3);
                write_output(e, o, &mut outputs);
            }
            Saved::Pollable(p) => {
                e.word(4);
                match p {
                    Pollable::Output(output) => {
                        e.word(3);
                        write_output(e, output, &mut outputs);
                    }
                    Pollable::Network(h, udp, write) => {
                        e.word(4);
                        handle(e, h);
                        e.word(*udp as u64);
                        e.word(*write as u64);
                    }
                    Pollable::Ready => e.word(0),
                    Pollable::Timer(t) => {
                        e.word(1);
                        e.word(*t)
                    }
                    Pollable::Socket(h, w) => {
                        e.word(2);
                        handle(e, h);
                        e.word(*w as u64)
                    }
                }
            }
            Saved::DirectoryStream(d) => {
                e.word(5);
                e.word(d.entries.len() as u64);
                for d in &d.entries {
                    e.word(match d.type_ {
                        DescriptorType::Unknown => 0,
                        DescriptorType::BlockDevice => 1,
                        DescriptorType::CharacterDevice => 2,
                        DescriptorType::Directory => 3,
                        DescriptorType::Fifo => 4,
                        DescriptorType::SymbolicLink => 5,
                        DescriptorType::RegularFile => 6,
                        DescriptorType::Socket => 7,
                    });
                    e.text(&d.name);
                }
            }
            Saved::IoError(error) => {
                e.word(6);
                e.text(&error.0);
            }
            Saved::Network(n) => {
                e.word(7);
                optional_entry(e, &n.0);
            }
            Saved::Tcp(t) => {
                e.word(8);
                family(e, t.family);
                e.word(match t.phase {
                    TcpPhase::Initial => 0,
                    TcpPhase::Binding => 1,
                    TcpPhase::Bound => 2,
                    TcpPhase::Connecting => 3,
                    TcpPhase::Connected => 4,
                    TcpPhase::ListeningStart => 5,
                    TcpPhase::Listening => 6,
                });
                optional_entry(e, &t.network);
                optional_entry(e, &t.endpoint);
                address(e, &t.local);
                address(e, &t.remote);
            }
            Saved::Udp(u) => {
                e.word(9);
                family(e, u.family);
                optional_entry(e, &u.endpoint);
                address(e, &u.local);
                address(e, &u.remote);
                e.word(u.binding as u64);
                e.word(u.streamed as u64);
            }
            Saved::Incoming(i) => {
                e.word(10);
                entry(e, &i.entry);
                address(e, &i.remote);
            }
            Saved::Outgoing(o) => {
                e.word(11);
                e.word(o.permit);
                entry(e, &o.entry);
                address(e, &o.remote);
            }
            Saved::Addresses(a) => {
                e.word(12);
                e.word(a.0.len() as u64);
                for a in &a.0 {
                    ip(e, a);
                }
            }
            Saved::TerminalInput => e.word(13),
            Saved::TerminalOutput => e.word(14),
        }
    }
    Ok(())
}
struct Read<'a, 'b> {
    outputs: BTreeMap<u64, Output>,
    d: &'a mut Decoder<'b>,
    ownership: Arc<AtomicBool>,
    handles: &'a mut BTreeMap<u64, Arc<dyn Handle>>,
}
impl Read<'_, '_> {
    fn output(&mut self) -> Result<Output, Error> {
        let id = self.d.word()?;
        if !self.d.flag()? {
            return self.outputs.get(&id).cloned().ok_or(Error::InvalidData);
        }
        if self.outputs.contains_key(&id) {
            return Err(Error::InvalidData);
        }
        let target = match self.d.word()? {
            0 => Target::Closed,
            1 => Target::Log,
            2 => Target::File(self.entry()?, self.d.word()?),
            3 => Target::Socket(self.entry()?.handle),
            _ => return Err(Error::InvalidData),
        };
        let pending = self.d.bytes(32768)?.to_vec();
        let permit = usize::try_from(self.d.word()?).map_err(|_| Error::Capacity)?;
        let flushing = self.d.flag()?;
        let failure = if self.d.flag()? {
            Some(
                core::str::from_utf8(self.d.bytes(4096)?)
                    .map_err(|_| Error::InvalidData)?
                    .to_string(),
            )
        } else {
            None
        };
        let output = Output::restore(
            id,
            OutputState {
                target,
                pending,
                permit,
                flushing,
                failure,
            },
        )
        .map_err(|_| Error::InvalidData)?;
        self.outputs.insert(id, output.clone());
        Ok(output)
    }

    fn entry(&mut self) -> Result<Entry, Error> {
        read_entry(self.d, self.ownership.clone(), self.handles)
    }
    fn optional_entry(&mut self) -> Result<Option<Entry>, Error> {
        Ok(if self.d.flag()? {
            Some(self.entry()?)
        } else {
            None
        })
    }
    fn ip(&mut self) -> Result<IpAddress, Error> {
        Ok(match self.d.word()? {
            4 => {
                let b = self.d.bytes(4)?;
                if b.len() != 4 {
                    return Err(Error::InvalidData);
                }
                IpAddress::Ipv4((b[0], b[1], b[2], b[3]))
            }
            6 => {
                let mut a = [0; 8];
                for n in &mut a {
                    *n = u16::try_from(self.d.word()?).map_err(|_| Error::InvalidData)?;
                }
                IpAddress::Ipv6((a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]))
            }
            _ => return Err(Error::InvalidData),
        })
    }
    fn address(&mut self) -> Result<Option<IpSocketAddress>, Error> {
        if !self.d.flag()? {
            return Ok(None);
        }
        let ip = self.ip()?;
        let port = u16::try_from(self.d.word()?).map_err(|_| Error::InvalidData)?;
        Ok(Some(match ip {
            IpAddress::Ipv4(address) => IpSocketAddress::Ipv4(Ipv4SocketAddress { address, port }),
            IpAddress::Ipv6(address) => IpSocketAddress::Ipv6(Ipv6SocketAddress {
                address,
                port,
                flow_info: word32(self.d)?,
                scope_id: word32(self.d)?,
            }),
        }))
    }
    fn family(&mut self) -> Result<IpAddressFamily, Error> {
        match self.d.word()? {
            4 => Ok(IpAddressFamily::Ipv4),
            6 => Ok(IpAddressFamily::Ipv6),
            _ => Err(Error::InvalidData),
        }
    }
    fn value(&mut self) -> Result<Saved, Error> {
        Ok(match self.d.word()? {
            1 => Saved::Descriptor(Descriptor {
                entry: self.entry()?,
                flags: decode_flags(self.d.word()?)?,
            }),
            2 => Saved::Input(match self.d.word()? {
                0 => Input::Empty,
                1 => Input::File(self.entry()?, self.d.word()?),
                2 => Input::Socket(self.entry()?.handle),
                _ => return Err(Error::InvalidData),
            }),
            3 => Saved::Output(self.output()?),
            4 => Saved::Pollable(match self.d.word()? {
                4 => Pollable::Network(self.entry()?.handle, self.d.flag()?, self.d.flag()?),
                3 => Pollable::Output(self.output()?),
                0 => Pollable::Ready,
                1 => Pollable::Timer(self.d.word()?),
                2 => Pollable::Socket(self.entry()?.handle, self.d.flag()?),
                _ => return Err(Error::InvalidData),
            }),
            5 => {
                let mut entries = std::collections::VecDeque::new();
                for _ in 0..self.d.count(1024)? {
                    let type_ = match self.d.word()? {
                        0 => DescriptorType::Unknown,
                        1 => DescriptorType::BlockDevice,
                        2 => DescriptorType::CharacterDevice,
                        3 => DescriptorType::Directory,
                        4 => DescriptorType::Fifo,
                        5 => DescriptorType::SymbolicLink,
                        6 => DescriptorType::RegularFile,
                        7 => DescriptorType::Socket,
                        _ => return Err(Error::InvalidData),
                    };
                    entries.push_back(DirectoryEntry {
                        type_,
                        name: self.d.text(4096)?.into(),
                    });
                }
                Saved::DirectoryStream(DirectoryStream { entries })
            }
            6 => Saved::IoError(IoError(self.d.text(32768)?.into())),
            7 => Saved::Network(Network(self.optional_entry()?)),
            8 => {
                let family = self.family()?;
                let phase = match self.d.word()? {
                    0 => TcpPhase::Initial,
                    1 => TcpPhase::Binding,
                    2 => TcpPhase::Bound,
                    3 => TcpPhase::Connecting,
                    4 => TcpPhase::Connected,
                    5 => TcpPhase::ListeningStart,
                    6 => TcpPhase::Listening,
                    _ => return Err(Error::InvalidData),
                };
                Saved::Tcp(Tcp {
                    family,
                    phase,
                    network: self.optional_entry()?,
                    endpoint: self.optional_entry()?,
                    local: self.address()?,
                    remote: self.address()?,
                })
            }
            9 => Saved::Udp(Udp {
                family: self.family()?,
                endpoint: self.optional_entry()?,
                local: self.address()?,
                remote: self.address()?,
                binding: self.d.flag()?,
                streamed: self.d.flag()?,
            }),
            10 => Saved::Incoming(Incoming {
                entry: self.entry()?,
                remote: self.address()?,
            }),
            11 => Saved::Outgoing(Outgoing {
                permit: self.d.count(16)? as u64,
                entry: self.entry()?,
                remote: self.address()?,
            }),
            12 => {
                let mut addresses = std::collections::VecDeque::new();
                for _ in 0..self.d.count(256)? {
                    addresses.push_back(self.ip()?);
                }
                Saved::Addresses(Addresses(addresses))
            }
            13 => Saved::TerminalInput,
            14 => Saved::TerminalOutput,
            _ => return Err(Error::InvalidData),
        })
    }
}
pub fn read_table(
    d: &mut Decoder<'_>,
    ownership: Arc<AtomicBool>,
    handles: &mut BTreeMap<u64, Arc<dyn Handle>>,
    maximum: usize,
) -> Result<Table, Error> {
    let mut r = Read {
        outputs: BTreeMap::new(),
        d,
        ownership,
        handles,
    };
    let mut table = Table::new();
    for _ in 0..r.d.count(maximum)? {
        let id = word32(r.d)?;
        let value = r.value()?;
        if table.insert(id, value).is_some() {
            return Err(Error::InvalidData);
        }
    }
    Ok(table)
}

const FLAGS: [DescriptorFlags; 6] = [
    DescriptorFlags::READ,
    DescriptorFlags::WRITE,
    DescriptorFlags::FILE_INTEGRITY_SYNC,
    DescriptorFlags::DATA_INTEGRITY_SYNC,
    DescriptorFlags::REQUESTED_WRITE_SYNC,
    DescriptorFlags::MUTATE_DIRECTORY,
];
fn encode_flags(flags: DescriptorFlags) -> u64 {
    FLAGS
        .iter()
        .enumerate()
        .fold(0, |bits, (i, f)| bits | ((flags.contains(*f) as u64) << i))
}
fn decode_flags(bits: u64) -> Result<DescriptorFlags, Error> {
    if bits >> FLAGS.len() != 0 {
        return Err(Error::InvalidData);
    }
    let mut flags = DescriptorFlags::empty();
    for (i, f) in FLAGS.iter().enumerate() {
        if bits & (1 << i) != 0 {
            flags |= *f;
        }
    }
    Ok(flags)
}
