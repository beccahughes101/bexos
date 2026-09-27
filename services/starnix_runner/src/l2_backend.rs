use bexos_ethernet_ring::{ethernet, link::PacketLink};
use bexos_migration::{
    Error as MigrationError,
    codec::{Decoder, Encoder},
};
use bexos_netstackd::{
    config::{ActiveConfig, ConfigSource, DnsMode},
    stack::Netstack,
};
use bexos_starnix_net::{InterfaceBackend, NetworkState, SocketDomain, SocketKind};
use bexos_userspace::{Memory, Socket, live_migration::Resource};
use net_fidl::{IpAddress, Ipv4Address, Ipv6Address, SocketAddress, Status};
use std::{collections::BTreeMap, net::IpAddr};

const OBJECT_TAG: u64 = 1 << 63;

struct L2Interface {
    namespace: u64,
    stack: Netstack,
    link: PacketLink,
}

#[derive(Clone, Copy)]
enum ObjectKind {
    Tcp,
    Listener,
    Udp,
    LoopTcp,
    LoopListener,
    LoopUdp,
}

struct LoopListener {
    namespace: u64,
    local: (IpAddr, u16),
    pending: Vec<(u64, (IpAddr, u16))>,
}

struct LoopUdp {
    namespace: u64,
    local: (IpAddr, u16),
    queued: Vec<(Vec<u8>, (IpAddr, u16))>,
}

pub struct TcpConnection {
    pub stream: u64,
    pub control: u64,
    pub local: (IpAddr, u16),
}

pub struct AcceptedConnection {
    pub stream: u64,
    pub control: u64,
    pub peer: (IpAddr, u16),
}

pub struct L2Runtime {
    interfaces: BTreeMap<u64, L2Interface>,
    objects: BTreeMap<u64, (u64, ObjectKind)>,
    loop_listeners: BTreeMap<u64, LoopListener>,
    loop_udp: BTreeMap<u64, LoopUdp>,
    next_object: u64,
}

impl L2Runtime {
    pub fn connect(network: &NetworkState) -> Result<Self, i64> {
        let mut interfaces = BTreeMap::new();
        for (namespace_id, namespace) in &network.namespaces {
            for interface in namespace.interfaces.values() {
                let InterfaceBackend::L2 { device, .. } = interface.backend else {
                    continue;
                };
                let ethernet = ethernet::connect(device).map_err(|_| starnix_kernel::EIO)?;
                let link = PacketLink::new(ethernet).map_err(status_errno)?;
                interfaces.insert(
                    interface.id,
                    L2Interface {
                        namespace: *namespace_id,
                        stack: Netstack::from_active(config(network, *namespace_id, interface.id)),
                        link,
                    },
                );
            }
        }
        Ok(Self {
            interfaces,
            objects: BTreeMap::new(),
            loop_listeners: BTreeMap::new(),
            loop_udp: BTreeMap::new(),
            next_object: OBJECT_TAG | 1,
        })
    }

    pub fn snapshot(&self) -> Result<Vec<u8>, MigrationError> {
        let mut out = Encoder::new();
        out.word(2);
        out.word(self.next_object);
        out.word(self.objects.len() as u64);
        for (object, (interface, kind)) in &self.objects {
            out.word(*object);
            out.word(*interface);
            out.word(match kind {
                ObjectKind::Tcp => 1,
                ObjectKind::Listener => 2,
                ObjectKind::Udp => 3,
                ObjectKind::LoopTcp => 4,
                ObjectKind::LoopListener => 5,
                ObjectKind::LoopUdp => 6,
            });
        }
        out.word(self.interfaces.len() as u64);
        for (interface_id, runtime) in &self.interfaces {
            out.word(*interface_id);
            out.word(runtime.namespace);
            bexos_netstackd::migration::encode_stack_v7(&mut out, &runtime.stack)?;
            bexos_netstackd::migration::encode_link(&mut out, runtime.link.resources);
            out.bytes(&runtime.link.checkpoint_queues()?);
            out.word(bexos_ethernet_ring::link::BACKLOG_CHUNKS as u64);
            for chunk in 0..bexos_ethernet_ring::link::BACKLOG_CHUNKS {
                out.bytes(&runtime.link.checkpoint_backlog(chunk)?);
            }
        }
        out.word(self.loop_listeners.len() as u64);
        for (object, listener) in &self.loop_listeners {
            out.word(*object);
            out.word(listener.namespace);
            encode_address(&mut out, listener.local);
            out.word(listener.pending.len() as u64);
            for (handle, peer) in &listener.pending {
                out.word(*handle);
                encode_address(&mut out, *peer);
            }
        }
        out.word(self.loop_udp.len() as u64);
        for (object, udp) in &self.loop_udp {
            out.word(*object);
            out.word(udp.namespace);
            encode_address(&mut out, udp.local);
            out.word(udp.queued.len() as u64);
            for (bytes, source) in &udp.queued {
                out.bytes(bytes);
                encode_address(&mut out, *source);
            }
        }
        Ok(out.finish())
    }

    pub fn restore(bytes: &[u8]) -> Result<Self, MigrationError> {
        Self::restore_impl(bytes, true)
    }

    fn restore_impl(bytes: &[u8], activate: bool) -> Result<Self, MigrationError> {
        let mut input = Decoder::new(bytes);
        let version = input.word()?;
        if !matches!(version, 1 | 2) {
            return Err(MigrationError::UnsupportedVersion);
        }
        let next_object = input.word()?;
        if next_object & OBJECT_TAG == 0 {
            return Err(MigrationError::InvalidData);
        }
        let mut objects = BTreeMap::new();
        for _ in 0..input.count(4096)? {
            let object = input.word()?;
            let interface = input.word()?;
            let kind = match input.word()? {
                1 => ObjectKind::Tcp,
                2 => ObjectKind::Listener,
                3 => ObjectKind::Udp,
                4 if version >= 2 => ObjectKind::LoopTcp,
                5 if version >= 2 => ObjectKind::LoopListener,
                6 if version >= 2 => ObjectKind::LoopUdp,
                _ => return Err(MigrationError::InvalidData),
            };
            if object & OBJECT_TAG == 0 || objects.insert(object, (interface, kind)).is_some() {
                return Err(MigrationError::InvalidData);
            }
        }
        let mut interfaces = BTreeMap::new();
        for _ in 0..input.count(64)? {
            let interface_id = input.word()?;
            let namespace = input.word()?;
            let mut stack = bexos_netstackd::migration::decode_stack_v7(&mut input)?;
            let resources = bexos_netstackd::migration::decode_link(&mut input)?;
            let queues = input.bytes(2 * 1024 * 1024)?.to_vec();
            let chunks = input.count(bexos_ethernet_ring::link::BACKLOG_CHUNKS)?;
            if chunks != bexos_ethernet_ring::link::BACKLOG_CHUNKS {
                return Err(MigrationError::InvalidData);
            }
            let mut link = PacketLink::from_resources(resources);
            link.adopt_queues(&queues)?;
            for chunk in 0..chunks {
                link.adopt_backlog(chunk, input.bytes(2 * 1024 * 1024)?)?;
            }
            if activate && stack.restore_after_migration(Some(&mut link)) != Status::Ok {
                return Err(MigrationError::InvalidData);
            }
            if interface_id == 0
                || namespace == 0
                || interfaces
                    .insert(
                        interface_id,
                        L2Interface {
                            namespace,
                            stack,
                            link,
                        },
                    )
                    .is_some()
            {
                return Err(MigrationError::InvalidData);
            }
        }
        let mut loop_listeners = BTreeMap::new();
        let mut loop_udp = BTreeMap::new();
        if version >= 2 {
            for _ in 0..input.count(256)? {
                let object = input.word()?;
                let namespace = input.word()?;
                let local = decode_address(&mut input)?;
                let mut pending = Vec::new();
                for _ in 0..input.count(256)? {
                    pending.push((input.word()?, decode_address(&mut input)?));
                }
                if !matches!(objects.get(&object), Some((value, ObjectKind::LoopListener)) if *value == namespace)
                    || loop_listeners
                        .insert(
                            object,
                            LoopListener {
                                namespace,
                                local,
                                pending,
                            },
                        )
                        .is_some()
                {
                    return Err(MigrationError::InvalidData);
                }
            }
            for _ in 0..input.count(256)? {
                let object = input.word()?;
                let namespace = input.word()?;
                let local = decode_address(&mut input)?;
                let mut queued = Vec::new();
                for _ in 0..input.count(256)? {
                    queued.push((input.bytes(65_535)?.to_vec(), decode_address(&mut input)?));
                }
                if !matches!(objects.get(&object), Some((value, ObjectKind::LoopUdp)) if *value == namespace)
                    || loop_udp
                        .insert(
                            object,
                            LoopUdp {
                                namespace,
                                local,
                                queued,
                            },
                        )
                        .is_some()
                {
                    return Err(MigrationError::InvalidData);
                }
            }
        }
        input.finish()?;
        if objects.values().any(|(interface, kind)| {
            matches!(
                kind,
                ObjectKind::Tcp | ObjectKind::Listener | ObjectKind::Udp
            ) && !interfaces.contains_key(interface)
        }) {
            return Err(MigrationError::InvalidData);
        }
        Ok(Self {
            interfaces,
            objects,
            loop_listeners,
            loop_udp,
            next_object,
        })
    }

    pub fn resources_from_snapshot(bytes: &[u8]) -> Result<Vec<Resource>, MigrationError> {
        let runtime = Self::restore_without_activation(bytes)?;
        let mut resources = Vec::new();
        for interface in runtime.interfaces.values() {
            for tcp in &interface.stack.tcp {
                if let Some(stream) = tcp.stream {
                    resources.push(Resource::Handle(stream.0));
                }
                if let Some(client) = tcp.client_control {
                    resources.push(Resource::Handle(client));
                }
            }
            for listener in &interface.stack.listeners {
                for pending in &listener.pending {
                    if let Some(stream) = pending.stream {
                        resources.push(Resource::Handle(stream.0));
                    }
                    resources.push(Resource::Handle(pending.control));
                    if let Some(client) = pending.client_control {
                        resources.push(Resource::Handle(client));
                    }
                }
            }
            let link = interface.link.resources;
            resources.extend([
                Resource::Handle(link.control),
                Resource::Handle(link.fifo),
                Resource::Handle(link.rx_vmo),
                Resource::Handle(link.tx_vmo),
                Resource::Mapping {
                    handle: link.rx_vmo,
                    offset: 0,
                    va: link.rx_vaddr,
                    size: bexos_ethernet_ring::ethernet::RX_BYTES,
                    rights: 6,
                },
                Resource::Mapping {
                    handle: link.tx_vmo,
                    offset: 0,
                    va: link.tx_vaddr,
                    size: bexos_ethernet_ring::ethernet::TX_BYTES,
                    rights: 6,
                },
            ]);
        }
        for listener in runtime.loop_listeners.values() {
            resources.extend(
                listener
                    .pending
                    .iter()
                    .map(|(handle, _)| Resource::Handle(*handle)),
            );
        }
        Ok(resources)
    }

    fn restore_without_activation(bytes: &[u8]) -> Result<Self, MigrationError> {
        Self::restore_impl(bytes, false)
    }

    pub fn poll(&mut self, network: &mut NetworkState) {
        for (interface_id, runtime) in &mut self.interfaces {
            let _ = runtime.link.poll();
            let mut frame = [0u8; bexos_ethernet_ring::link::SLOT_SIZE];
            while let Ok(length) = runtime.link.receive_device(&mut frame) {
                let is_icmp =
                    (length >= 34 && frame[12..14] == 0x0800u16.to_be_bytes() && frame[23] == 1)
                        || (length >= 54
                            && frame[12..14] == 0x86ddu16.to_be_bytes()
                            && frame[20] == 58);
                for socket in network.sockets.values_mut().filter(|socket| {
                    socket.namespace == runtime.namespace
                        && ((matches!(socket.domain, SocketDomain::Packet)
                            || matches!(socket.kind, SocketKind::Raw))
                            || (is_icmp
                                && ((socket.protocol == 1
                                    && matches!(socket.domain, SocketDomain::Inet4))
                                    || (socket.protocol == 58
                                        && matches!(socket.domain, SocketDomain::Inet6)))))
                        && socket
                            .bound_interface
                            .is_none_or(|bound| bound == *interface_id)
                }) {
                    if socket.queued_packets.len() < 64 {
                        socket.queued_packets.push(frame[..length].to_vec());
                    }
                }
                let _ = runtime.link.push_rx_backlog(&frame[..length]);
            }
            runtime.stack.poll_packet_plane(Some(&mut runtime.link));
        }
    }

    pub fn is_object(control: u64) -> bool {
        control & OBJECT_TAG != 0
    }

    fn object(&mut self, interface: u64, kind: ObjectKind) -> u64 {
        let object = self.next_object;
        self.next_object = self.next_object.wrapping_add(1).max(OBJECT_TAG | 1);
        self.objects.insert(object, (interface, kind));
        object
    }

    pub fn connect_tcp(
        &mut self,
        interface: u64,
        remote: (IpAddr, u16),
    ) -> Result<TcpConnection, i64> {
        let object = self.object(interface, ObjectKind::Tcp);
        let Some(runtime) = self.interfaces.get_mut(&interface) else {
            self.objects.remove(&object);
            return Err(starnix_kernel::ENETUNREACH);
        };
        let (application, stack) = Socket::pair().map_err(|_| starnix_kernel::ENOMEM)?;
        let status = runtime.stack.connect_backend_tcp(
            object,
            wire_address(remote),
            stack.0,
            Some(&mut runtime.link),
        );
        if status != Status::Ok {
            self.objects.remove(&object);
            let _ = Memory::close(application.0);
            return Err(status_errno(status));
        }
        let local = runtime
            .stack
            .tcp_mut(object)
            .map(|tcp| native_address(tcp.local))
            .ok_or(starnix_kernel::EIO)?;
        Ok(TcpConnection {
            stream: application.0,
            control: object,
            local,
        })
    }

    pub fn connect_loopback(
        &mut self,
        namespace: u64,
        local: (IpAddr, u16),
        remote: (IpAddr, u16),
    ) -> Result<TcpConnection, i64> {
        let listener = self
            .loop_listeners
            .values_mut()
            .find(|listener| {
                listener.namespace == namespace
                    && listener.local.1 == remote.1
                    && (listener.local.0.is_unspecified() || listener.local.0 == remote.0)
            })
            .ok_or(starnix_kernel::ECONNREFUSED)?;
        if listener.pending.len() >= 128 {
            return Err(starnix_kernel::EAGAIN);
        }
        let local = (
            if local.0.is_unspecified() {
                remote.0
            } else {
                local.0
            },
            if local.1 == 0 {
                49_152 + (self.next_object as u16 % 16_383)
            } else {
                local.1
            },
        );
        let (client, server) = Socket::pair().map_err(|_| starnix_kernel::ENOMEM)?;
        listener.pending.push((server.0, local));
        let control = self.object(namespace, ObjectKind::LoopTcp);
        Ok(TcpConnection {
            stream: client.0,
            control,
            local,
        })
    }

    pub fn listen_tcp(&mut self, interface: u64, local: (IpAddr, u16)) -> Result<u64, i64> {
        let object = self.object(interface, ObjectKind::Listener);
        let Some(runtime) = self.interfaces.get_mut(&interface) else {
            self.objects.remove(&object);
            return Err(starnix_kernel::ENETUNREACH);
        };
        let status = runtime.stack.listen_tcp(object, wire_address(local));
        let status = if status == Status::Ok {
            runtime
                .stack
                .attach_listener_to_link(object, Some(&mut runtime.link))
        } else {
            status
        };
        if status != Status::Ok {
            runtime.stack.remove_listener(object);
            self.objects.remove(&object);
            return Err(status_errno(status));
        }
        Ok(object)
    }

    pub fn listen_loopback(&mut self, namespace: u64, local: (IpAddr, u16)) -> Result<u64, i64> {
        if local.1 == 0
            || self.loop_listeners.values().any(|listener| {
                listener.namespace == namespace
                    && listener.local.1 == local.1
                    && (listener.local.0.is_unspecified()
                        || local.0.is_unspecified()
                        || listener.local.0 == local.0)
            })
        {
            return Err(starnix_kernel::EADDRINUSE);
        }
        let object = self.object(namespace, ObjectKind::LoopListener);
        self.loop_listeners.insert(
            object,
            LoopListener {
                namespace,
                local,
                pending: Vec::new(),
            },
        );
        Ok(object)
    }

    pub fn accept_tcp(&mut self, listener: u64) -> Result<AcceptedConnection, i64> {
        if matches!(
            self.objects.get(&listener),
            Some((_, ObjectKind::LoopListener))
        ) {
            let (namespace, stream, peer) = {
                let listener = self
                    .loop_listeners
                    .get_mut(&listener)
                    .ok_or(starnix_kernel::EBADF)?;
                if listener.pending.is_empty() {
                    return Err(starnix_kernel::EAGAIN);
                }
                let (stream, peer) = listener.pending.remove(0);
                (listener.namespace, stream, peer)
            };
            return Ok(AcceptedConnection {
                stream,
                control: self.object(namespace, ObjectKind::LoopTcp),
                peer,
            });
        }
        let (interface, ObjectKind::Listener) = self
            .objects
            .get(&listener)
            .copied()
            .ok_or(starnix_kernel::EBADF)?
        else {
            return Err(starnix_kernel::EINVAL);
        };
        let (mut endpoint, stream, peer) = {
            let runtime = self
                .interfaces
                .get_mut(&interface)
                .ok_or(starnix_kernel::ENETUNREACH)?;
            runtime.stack.poll_packet_plane(Some(&mut runtime.link));
            let mut endpoint = runtime
                .stack
                .listener_mut(listener)
                .and_then(|listener| listener.accept().ok())
                .ok_or(starnix_kernel::EAGAIN)?;
            let stream = endpoint.attach_stream().map_err(status_errno)?;
            if let Some(client_control) = endpoint.client_control.take() {
                let _ = Memory::close(client_control);
            }
            let peer = native_address(endpoint.peer);
            (endpoint, stream, peer)
        };
        let object = self.object(interface, ObjectKind::Tcp);
        endpoint.control = object;
        self.interfaces
            .get_mut(&interface)
            .ok_or(starnix_kernel::ENETUNREACH)?
            .stack
            .tcp
            .push(endpoint);
        Ok(AcceptedConnection {
            stream: stream.0,
            control: object,
            peer,
        })
    }

    pub fn create_udp(&mut self, interface: u64, local: (IpAddr, u16)) -> Result<u64, i64> {
        let object = self.object(interface, ObjectKind::Udp);
        let Some(runtime) = self.interfaces.get_mut(&interface) else {
            self.objects.remove(&object);
            return Err(starnix_kernel::ENETUNREACH);
        };
        let status = runtime.stack.create_udp(object);
        let status = if status == Status::Ok {
            runtime.stack.bind_udp(object, wire_address(local))
        } else {
            status
        };
        if status != Status::Ok {
            runtime.stack.remove_udp(object);
            self.objects.remove(&object);
            return Err(status_errno(status));
        }
        Ok(object)
    }

    pub fn create_loopback_udp(
        &mut self,
        namespace: u64,
        mut local: (IpAddr, u16),
    ) -> Result<u64, i64> {
        if local.1 == 0 {
            local.1 = 49_152 + (self.next_object as u16 % 16_383);
        }
        if self.loop_udp.values().any(|udp| {
            udp.namespace == namespace
                && udp.local.1 == local.1
                && (udp.local.0.is_unspecified()
                    || local.0.is_unspecified()
                    || udp.local.0 == local.0)
        }) {
            return Err(starnix_kernel::EADDRINUSE);
        }
        let object = self.object(namespace, ObjectKind::LoopUdp);
        self.loop_udp.insert(
            object,
            LoopUdp {
                namespace,
                local,
                queued: Vec::new(),
            },
        );
        Ok(object)
    }

    pub fn send_udp(
        &mut self,
        object: u64,
        data: &[u8],
        destination: (IpAddr, u16),
    ) -> Result<usize, i64> {
        if matches!(self.objects.get(&object), Some((_, ObjectKind::LoopUdp))) {
            let source = self.loop_udp.get(&object).ok_or(starnix_kernel::EBADF)?;
            let namespace = source.namespace;
            let source = source.local;
            let target = self
                .loop_udp
                .values_mut()
                .find(|udp| {
                    udp.namespace == namespace
                        && udp.local.1 == destination.1
                        && (udp.local.0.is_unspecified() || udp.local.0 == destination.0)
                })
                .ok_or(starnix_kernel::ECONNREFUSED)?;
            if target.queued.len() >= 256 {
                return Err(starnix_kernel::EAGAIN);
            }
            target.queued.push((data.to_vec(), source));
            return Ok(data.len());
        }
        let (interface, ObjectKind::Udp) = self
            .objects
            .get(&object)
            .copied()
            .ok_or(starnix_kernel::EBADF)?
        else {
            return Err(starnix_kernel::EINVAL);
        };
        let runtime = self
            .interfaces
            .get_mut(&interface)
            .ok_or(starnix_kernel::ENETUNREACH)?;
        let (status, actual) = runtime
            .stack
            .udp_mut(object)
            .map(|udp| udp.send_to(data, wire_address(destination)))
            .ok_or(starnix_kernel::EBADF)?;
        if status != Status::Ok {
            return Err(status_errno(status));
        }
        runtime.stack.poll_packet_plane(Some(&mut runtime.link));
        usize::try_from(actual).map_err(|_| starnix_kernel::EIO)
    }

    pub fn recv_udp(&mut self, object: u64) -> Result<(Vec<u8>, (IpAddr, u16)), i64> {
        if matches!(self.objects.get(&object), Some((_, ObjectKind::LoopUdp))) {
            let udp = self
                .loop_udp
                .get_mut(&object)
                .ok_or(starnix_kernel::EBADF)?;
            if udp.queued.is_empty() {
                return Err(starnix_kernel::EAGAIN);
            }
            return Ok(udp.queued.remove(0));
        }
        let (interface, ObjectKind::Udp) = self
            .objects
            .get(&object)
            .copied()
            .ok_or(starnix_kernel::EBADF)?
        else {
            return Err(starnix_kernel::EINVAL);
        };
        let runtime = self
            .interfaces
            .get_mut(&interface)
            .ok_or(starnix_kernel::ENETUNREACH)?;
        runtime.stack.poll_packet_plane(Some(&mut runtime.link));
        let datagram = runtime
            .stack
            .udp_mut(object)
            .ok_or(starnix_kernel::EBADF)?
            .recv_from()
            .map_err(status_errno)?;
        Ok((
            datagram.data[..datagram.len].to_vec(),
            native_address(datagram.source),
        ))
    }

    pub fn transmit_raw(&mut self, interface: u64, frame: &[u8]) -> Result<usize, i64> {
        let runtime = self
            .interfaces
            .get_mut(&interface)
            .ok_or(starnix_kernel::ENETUNREACH)?;
        let status = runtime.link.transmit(frame);
        if status == Status::Ok {
            Ok(frame.len())
        } else {
            Err(status_errno(status))
        }
    }

    pub fn send_icmp(
        &mut self,
        interface: u64,
        destination: IpAddr,
        packet: &[u8],
    ) -> Result<usize, i64> {
        let runtime = self
            .interfaces
            .get_mut(&interface)
            .ok_or(starnix_kernel::ENETUNREACH)?;
        if packet.len() < 8 {
            return Err(starnix_kernel::EINVAL);
        }
        let mut frame = match destination {
            IpAddr::V4(destination) => {
                let Some(source) = runtime.stack.config.ipv4 else {
                    return Err(starnix_kernel::EAFNOSUPPORT);
                };
                build_icmpv4_frame(source, destination.octets(), packet, false)?
            }
            IpAddr::V6(destination) => {
                let Some(source) = runtime.stack.config.ipv6 else {
                    return Err(starnix_kernel::EAFNOSUPPORT);
                };
                build_icmpv6_frame(source, destination.octets(), packet, false)?
            }
        };
        if frame.len() > bexos_ethernet_ring::link::SLOT_SIZE {
            return Err(starnix_kernel::EINVAL);
        }
        frame[..6].fill(0xff);
        frame[6..12].copy_from_slice(&runtime.link.resources.mac);
        let status = runtime.link.transmit(&frame);
        if status == Status::Ok {
            Ok(packet.len())
        } else {
            Err(status_errno(status))
        }
    }

    pub fn send_loopback_icmp(
        &mut self,
        network: &mut NetworkState,
        socket: u64,
        destination: IpAddr,
        packet: &[u8],
    ) -> Result<usize, i64> {
        let frame = match destination {
            IpAddr::V4(destination) if packet.first() == Some(&8) => {
                build_icmpv4_frame(destination.octets(), destination.octets(), packet, true)?
            }
            IpAddr::V6(destination) if packet.first() == Some(&128) => {
                build_icmpv6_frame(destination.octets(), destination.octets(), packet, true)?
            }
            _ => return Err(starnix_kernel::EINVAL),
        };
        let state = network
            .sockets
            .get_mut(&socket)
            .ok_or(starnix_kernel::EBADF)?;
        if state.queued_packets.len() >= 64 {
            return Err(starnix_kernel::EAGAIN);
        }
        state.queued_packets.push(frame);
        Ok(packet.len())
    }

    pub fn close(&mut self, object: u64) {
        let Some((interface, kind)) = self.objects.remove(&object) else {
            return;
        };
        if let Some(runtime) = self.interfaces.get_mut(&interface) {
            match kind {
                ObjectKind::Tcp => {
                    let _ = runtime.stack.remove_backend_tcp(object);
                }
                ObjectKind::Listener => runtime.stack.remove_listener(object),
                ObjectKind::Udp => runtime.stack.remove_udp(object),
                ObjectKind::LoopTcp | ObjectKind::LoopListener | ObjectKind::LoopUdp => {}
            }
        }
        if matches!(kind, ObjectKind::LoopListener) {
            if let Some(listener) = self.loop_listeners.remove(&object) {
                for (handle, _) in listener.pending {
                    let _ = Memory::close(handle);
                }
            }
        } else if matches!(kind, ObjectKind::LoopUdp) {
            self.loop_udp.remove(&object);
        }
    }
}

fn config(network: &NetworkState, namespace: u64, interface_id: u64) -> ActiveConfig {
    let namespace = &network.namespaces[&namespace];
    let interface = &namespace.interfaces[&interface_id];
    let ipv4 = interface
        .addresses
        .iter()
        .find_map(|address| match address.address {
            IpAddr::V4(value) => Some((value.octets(), address.prefix_len)),
            _ => None,
        });
    let ipv6 = interface
        .addresses
        .iter()
        .find_map(|address| match address.address {
            IpAddr::V6(value) => Some((value.octets(), address.prefix_len)),
            _ => None,
        });
    let gateway4 = namespace.routes.iter().find_map(|route| {
        (route.interface_id == interface_id && route.destination.prefix_len == 0)
            .then_some(route.gateway)
            .flatten()
            .and_then(|value| match value {
                IpAddr::V4(value) => Some(value.octets()),
                _ => None,
            })
    });
    let gateway6 = namespace.routes.iter().find_map(|route| {
        (route.interface_id == interface_id && route.destination.prefix_len == 0)
            .then_some(route.gateway)
            .flatten()
            .and_then(|value| match value {
                IpAddr::V6(value) => Some(value.octets()),
                _ => None,
            })
    });
    let dns4 = namespace
        .dns_servers
        .iter()
        .find_map(|server| match server {
            IpAddr::V4(value) => Some(value.octets()),
            _ => None,
        });
    let dns6 = namespace
        .dns_servers
        .iter()
        .find_map(|server| match server {
            IpAddr::V6(value) => Some(value.octets()),
            _ => None,
        });
    ActiveConfig {
        source: ConfigSource::Static,
        ipv4: ipv4.map(|value| value.0),
        prefix_len: ipv4.map_or(0, |value| value.1),
        gateway: gateway4,
        dns: dns4,
        ipv6: ipv6.map(|value| value.0),
        ipv6_prefix_len: ipv6.map_or(0, |value| value.1),
        ipv6_gateway: gateway6,
        dns_ipv6: dns6,
        doh_bootstrap_ipv6: None,
        slaac_enabled: ipv6.is_none(),
        mtu: interface.mtu,
        dns_mode: DnsMode::Udp53,
        doh_host: String::new(),
        doh_path: String::new(),
        doh_bootstrap_ipv4: None,
        doh_port: 443,
        doh_strict: false,
        dns_cache_capacity: 64,
    }
}

fn wire_address(address: (IpAddr, u16)) -> SocketAddress {
    SocketAddress {
        addr: match address.0 {
            IpAddr::V4(address) => IpAddress::Ipv4(Ipv4Address {
                octets: address.octets(),
            }),
            IpAddr::V6(address) => IpAddress::Ipv6(Ipv6Address {
                octets: address.octets(),
            }),
        },
        port: address.1,
    }
}

fn native_address(address: SocketAddress) -> (IpAddr, u16) {
    (
        match address.addr {
            IpAddress::Ipv4(value) => IpAddr::V4(value.octets.into()),
            IpAddress::Ipv6(value) => IpAddr::V6(value.octets.into()),
        },
        address.port,
    )
}

fn encode_address(out: &mut Encoder, address: (IpAddr, u16)) {
    match address.0 {
        IpAddr::V4(value) => {
            out.word(4);
            out.bytes(&value.octets());
        }
        IpAddr::V6(value) => {
            out.word(6);
            out.bytes(&value.octets());
        }
    }
    out.word(u64::from(address.1));
}

fn decode_address(input: &mut Decoder<'_>) -> Result<(IpAddr, u16), MigrationError> {
    let address = match input.word()? {
        4 => IpAddr::V4(std::net::Ipv4Addr::from(
            <[u8; 4]>::try_from(input.bytes(4)?).map_err(|_| MigrationError::InvalidData)?,
        )),
        6 => IpAddr::V6(std::net::Ipv6Addr::from(
            <[u8; 16]>::try_from(input.bytes(16)?).map_err(|_| MigrationError::InvalidData)?,
        )),
        _ => return Err(MigrationError::InvalidData),
    };
    let port = u16::try_from(input.word()?).map_err(|_| MigrationError::InvalidData)?;
    Ok((address, port))
}

fn status_errno(status: Status) -> i64 {
    match status {
        Status::ErrAccessDenied => starnix_kernel::EACCES,
        Status::ErrInvalidArgs => starnix_kernel::EINVAL,
        Status::ErrResourceExhausted | Status::ErrShouldWait => starnix_kernel::EAGAIN,
        Status::ErrNetworkUnreachable => starnix_kernel::ENETUNREACH,
        Status::ErrTimedOut => starnix_kernel::ETIMEDOUT,
        Status::ErrNotFound => starnix_kernel::ENOENT,
        Status::Ok => 0,
        _ => starnix_kernel::EIO,
    }
}

fn build_icmpv4_frame(
    source: [u8; 4],
    destination: [u8; 4],
    packet: &[u8],
    echo_reply: bool,
) -> Result<Vec<u8>, i64> {
    if packet.len() < 8 {
        return Err(starnix_kernel::EINVAL);
    }
    let total = packet
        .len()
        .checked_add(20)
        .filter(|total| *total <= u16::MAX as usize)
        .ok_or(starnix_kernel::EINVAL)?;
    let mut frame = vec![0u8; 14 + total];
    frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    frame[14] = 0x45;
    frame[16..18].copy_from_slice(&(total as u16).to_be_bytes());
    frame[22] = 64;
    frame[23] = 1;
    frame[26..30].copy_from_slice(&source);
    frame[30..34].copy_from_slice(&destination);
    let ip_checksum = checksum(&frame[14..34]);
    frame[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
    frame[34..].copy_from_slice(packet);
    if echo_reply {
        frame[34] = 0;
    }
    frame[36..38].fill(0);
    let icmp_checksum = checksum(&frame[34..]);
    frame[36..38].copy_from_slice(&icmp_checksum.to_be_bytes());
    Ok(frame)
}

fn build_icmpv6_frame(
    source: [u8; 16],
    destination: [u8; 16],
    packet: &[u8],
    echo_reply: bool,
) -> Result<Vec<u8>, i64> {
    if packet.len() < 8 {
        return Err(starnix_kernel::EINVAL);
    }
    let payload = u16::try_from(packet.len()).map_err(|_| starnix_kernel::EINVAL)?;
    let mut frame = vec![0u8; 54 + packet.len()];
    frame[12..14].copy_from_slice(&0x86ddu16.to_be_bytes());
    frame[14] = 0x60;
    frame[18..20].copy_from_slice(&payload.to_be_bytes());
    frame[20] = 58;
    frame[21] = 64;
    frame[22..38].copy_from_slice(&source);
    frame[38..54].copy_from_slice(&destination);
    frame[54..].copy_from_slice(packet);
    if echo_reply {
        frame[54] = 129;
    }
    frame[56..58].fill(0);
    let mut pseudo = Vec::with_capacity(40 + packet.len());
    pseudo.extend_from_slice(&source);
    pseudo.extend_from_slice(&destination);
    pseudo.extend_from_slice(&(packet.len() as u32).to_be_bytes());
    pseudo.extend_from_slice(&[0, 0, 0, 58]);
    pseudo.extend_from_slice(&frame[54..]);
    let icmp_checksum = checksum(&pseudo);
    frame[56..58].copy_from_slice(&icmp_checksum.to_be_bytes());
    Ok(frame)
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in bytes.chunks(2) {
        sum = sum.wrapping_add(if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]]) as u32
        } else {
            (chunk[0] as u32) << 8
        });
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_icmp_frames_cover_ipv4_and_ipv6_checksums() {
        let echo4 = [8, 0, 0, 0, 0x12, 0x34, 0, 1];
        let frame4 = build_icmpv4_frame([127, 0, 0, 1], [127, 0, 0, 1], &echo4, true).unwrap();
        assert_eq!(frame4[12..14], 0x0800u16.to_be_bytes());
        assert_eq!(frame4[34], 0);
        assert_eq!(checksum(&frame4[14..34]), 0);
        assert_eq!(checksum(&frame4[34..]), 0);

        let mut loop6 = [0u8; 16];
        loop6[15] = 1;
        let echo6 = [128, 0, 0, 0, 0x12, 0x34, 0, 1];
        let frame6 = build_icmpv6_frame(loop6, loop6, &echo6, true).unwrap();
        assert_eq!(frame6[12..14], 0x86ddu16.to_be_bytes());
        assert_eq!(frame6[54], 129);
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(&loop6);
        pseudo.extend_from_slice(&loop6);
        pseudo.extend_from_slice(&8u32.to_be_bytes());
        pseudo.extend_from_slice(&[0, 0, 0, 58]);
        pseudo.extend_from_slice(&frame6[54..]);
        assert_eq!(checksum(&pseudo), 0);
    }
}
