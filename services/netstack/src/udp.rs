use alloc::collections::VecDeque;
use alloc::vec::Vec;
use net_fidl::{IpAddress, Ipv4Address, Ipv6Address, SocketAddress, Status};
use smoltcp::iface::SocketHandle;

#[derive(Clone, Debug)]
pub struct Datagram {
    pub data: [u8; 8192],
    pub len: usize,
    pub source: SocketAddress,
}

#[derive(Clone, Debug)]
pub struct OutboundDatagram {
    pub data: [u8; 8192],
    pub len: usize,
    pub source: SocketAddress,
    pub destination: SocketAddress,
}

#[derive(Clone, Debug)]
pub struct UdpEndpoint {
    pub control: u64,
    pub local: Option<SocketAddress>,
    pub queue: VecDeque<Datagram>,
    pub outbound: VecDeque<OutboundDatagram>,
    pub closed: bool,
    pub smoltcp_handle: Option<SocketHandle>,
}

impl UdpEndpoint {
    pub fn new(control: u64) -> Self {
        Self {
            control,
            local: None,
            queue: VecDeque::new(),
            outbound: VecDeque::new(),
            closed: false,
            smoltcp_handle: None,
        }
    }

    pub fn bind(&mut self, local: SocketAddress) -> Status {
        if self.closed {
            return Status::ErrPeerClosed;
        }
        if self.local.is_some() {
            return Status::ErrAlreadyExists;
        }
        self.local = Some(local);
        Status::Ok
    }

    pub fn send_to(&mut self, bytes: &[u8], destination: SocketAddress) -> (Status, u64) {
        if self.closed {
            return (Status::ErrPeerClosed, 0);
        }
        let Some(source) = self.local else {
            return (Status::ErrInvalidArgs, 0);
        };
        if bytes.len() > 8192 {
            return (Status::ErrBufferTooSmall, 0);
        }
        if self.outbound.len() >= 64 {
            return (Status::ErrResourceExhausted, 0);
        }
        let mut packet = OutboundDatagram {
            data: [0; 8192],
            len: bytes.len(),
            source,
            destination,
        };
        packet.data[..bytes.len()].copy_from_slice(bytes);
        self.outbound.push_back(packet);
        (Status::Ok, bytes.len() as u64)
    }

    pub fn push_datagram(&mut self, data: &[u8], source: SocketAddress) -> Status {
        if data.len() > 8192 {
            return Status::ErrBufferTooSmall;
        }
        if self.queue.len() >= 64 {
            return Status::ErrResourceExhausted;
        }
        let mut packet = Datagram {
            data: [0; 8192],
            len: data.len(),
            source,
        };
        packet.data[..data.len()].copy_from_slice(data);
        self.queue.push_back(packet);
        Status::Ok
    }

    pub fn recv_from(&mut self) -> Result<Datagram, Status> {
        if self.closed {
            return Err(Status::ErrPeerClosed);
        }
        self.queue.pop_front().ok_or(Status::ErrShouldWait)
    }

    pub fn close(&mut self) {
        self.closed = true;
        self.queue.clear();
        self.outbound.clear();
    }

    pub fn drain_outbound(&mut self) -> Vec<OutboundDatagram> {
        let mut packets = Vec::new();
        while let Some(packet) = self.outbound.pop_front() {
            packets.push(packet);
        }
        packets
    }

    pub fn matches_destination(&self, destination: SocketAddress) -> bool {
        self.local.is_some_and(|local| {
            local.port == destination.port
                && (is_unspecified(local.addr) || local.addr == destination.addr)
        })
    }
}

#[derive(Clone, Debug)]
pub struct DecodedUdpDatagram {
    pub data: [u8; 8192],
    pub len: usize,
    pub source: SocketAddress,
    pub destination: SocketAddress,
}

pub fn encode_udp_ipv4_ethernet(
    payload: &[u8],
    source: SocketAddress,
    destination: SocketAddress,
    source_mac: [u8; 6],
    out: &mut [u8],
) -> Option<usize> {
    let src = ipv4(source.addr)?;
    let dst = ipv4(destination.addr)?;
    let udp_len = 8usize.checked_add(payload.len())?;
    let ip_len = 20usize.checked_add(udp_len)?;
    let frame_len = 14usize.checked_add(ip_len)?;
    if payload.len() > 8192 || udp_len > u16::MAX as usize || out.len() < frame_len {
        return None;
    }
    out[..6].fill(0xff);
    out[6..12].copy_from_slice(&source_mac);
    out[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    out[14] = 0x45;
    out[15] = 0;
    out[16..18].copy_from_slice(&(ip_len as u16).to_be_bytes());
    out[18..20].fill(0);
    out[20..22].fill(0);
    out[22] = 64;
    out[23] = 17;
    out[24..26].fill(0);
    out[26..30].copy_from_slice(&src);
    out[30..34].copy_from_slice(&dst);
    let csum = checksum(&out[14..34]);
    out[24..26].copy_from_slice(&csum.to_be_bytes());
    let udp = 34;
    out[udp..udp + 2].copy_from_slice(&source.port.to_be_bytes());
    out[udp + 2..udp + 4].copy_from_slice(&destination.port.to_be_bytes());
    out[udp + 4..udp + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    out[udp + 6..udp + 8].fill(0);
    out[udp + 8..udp + 8 + payload.len()].copy_from_slice(payload);
    Some(frame_len)
}

pub fn decode_udp_ipv4_ethernet(frame: &[u8]) -> Option<DecodedUdpDatagram> {
    if frame.len() < 42 || frame[12..14] != 0x0800u16.to_be_bytes() || frame[14] >> 4 != 4 {
        return None;
    }
    let ihl = ((frame[14] & 0x0f) as usize) * 4;
    if ihl < 20 || frame.len() < 14 + ihl + 8 || frame[23] != 17 {
        return None;
    }
    let total_len = u16::from_be_bytes([frame[16], frame[17]]) as usize;
    if total_len < ihl + 8 || 14 + total_len > frame.len() {
        return None;
    }
    let udp = 14 + ihl;
    let udp_len = u16::from_be_bytes([frame[udp + 4], frame[udp + 5]]) as usize;
    if udp_len < 8 || udp + udp_len > frame.len() {
        return None;
    }
    let payload = &frame[udp + 8..udp + udp_len];
    if payload.len() > 8192 {
        return None;
    }
    let mut data = [0; 8192];
    data[..payload.len()].copy_from_slice(payload);
    Some(DecodedUdpDatagram {
        data,
        len: payload.len(),
        source: SocketAddress {
            addr: IpAddress::Ipv4(Ipv4Address {
                octets: [frame[26], frame[27], frame[28], frame[29]],
            }),
            port: u16::from_be_bytes([frame[udp], frame[udp + 1]]),
        },
        destination: SocketAddress {
            addr: IpAddress::Ipv4(Ipv4Address {
                octets: [frame[30], frame[31], frame[32], frame[33]],
            }),
            port: u16::from_be_bytes([frame[udp + 2], frame[udp + 3]]),
        },
    })
}

fn ipv4(addr: IpAddress) -> Option<[u8; 4]> {
    match addr {
        IpAddress::Ipv4(addr) => Some(addr.octets),
        _ => None,
    }
}

fn is_unspecified(addr: IpAddress) -> bool {
    match addr {
        IpAddress::Ipv4(Ipv4Address { octets }) => octets == [0, 0, 0, 0],
        IpAddress::Ipv6(Ipv6Address { octets }) => octets == [0; 16],
    }
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in bytes.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]]) as u32
        } else {
            (chunk[0] as u32) << 8
        };
        sum = sum.wrapping_add(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
