use super::{
    state::{Input, IoError, Output, Pollable},
    wasi::sockets::{
        instance_network, ip_name_lookup, network::*, tcp, tcp_create_socket, udp,
        udp_create_socket,
    },
};
use crate::{context::Context, resources::Entry};
use wasmtime::{Result, component::Resource};
pub type NetResult<T> = Result<T, ErrorCode>;
#[derive(Clone)]
pub struct Network(pub Option<Entry>);
#[derive(Clone)]
pub struct Addresses(pub std::collections::VecDeque<IpAddress>);
#[derive(Clone)]
pub enum TcpPhase {
    Initial,
    Binding,
    Bound,
    Connecting,
    Connected,
    ListeningStart,
    Listening,
}
#[derive(Clone)]
pub struct Tcp {
    pub family: IpAddressFamily,
    pub phase: TcpPhase,
    pub network: Option<Entry>,
    pub endpoint: Option<Entry>,
    pub local: Option<IpSocketAddress>,
    pub remote: Option<IpSocketAddress>,
}
#[derive(Clone)]
pub struct Udp {
    pub family: IpAddressFamily,
    pub endpoint: Option<Entry>,
    pub local: Option<IpSocketAddress>,
    pub remote: Option<IpSocketAddress>,
    pub binding: bool,
    pub streamed: bool,
}
#[derive(Clone)]
pub struct Incoming {
    pub entry: Entry,
    pub remote: Option<IpSocketAddress>,
}
#[derive(Clone)]
pub struct Outgoing {
    pub permit: u64,
    pub entry: Entry,
    pub remote: Option<IpSocketAddress>,
}
fn family_matches(f: IpAddressFamily, a: &IpSocketAddress) -> bool {
    matches!(
        (f, a),
        (IpAddressFamily::Ipv4, IpSocketAddress::Ipv4(_))
            | (IpAddressFamily::Ipv6, IpSocketAddress::Ipv6(_))
    )
}
impl Host for Context {
    fn network_error_code(&mut self, r: Resource<IoError>) -> Result<Option<ErrorCode>> {
        self.wasi.table.get(&r)?;
        Ok(None)
    }
}
impl HostNetwork for Context {
    fn drop(&mut self, r: Resource<Network>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl instance_network::Host for Context {
    fn instance_network(&mut self) -> Result<Resource<Network>> {
        let grant = self
            .resources
            .entries()
            .find(|(_, e)| {
                e.name == "bexos.net.SocketProvider"
                    && e.handle.kind() == crate::resources::Kind::Channel
                    && e.handle.rights() & (crate::resources::READ | crate::resources::WRITE)
                        == (crate::resources::READ | crate::resources::WRITE)
            })
            .or_else(|| {
                self.resources.entries().find(|(_, e)| {
                    e.name == "bexos.net.Netstack"
                        && e.handle.kind() == crate::resources::Kind::Channel
                        && e.handle.rights() & (crate::resources::READ | crate::resources::WRITE)
                            == (crate::resources::READ | crate::resources::WRITE)
                })
            })
            .map(|(_, e)| e.clone());
        self.push(Network(grant))
    }
}
impl ip_name_lookup::Host for Context {
    fn resolve_addresses(
        &mut self,
        r: Resource<Network>,
        name: String,
    ) -> Result<NetResult<Resource<Addresses>>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let Some(grant) = self.wasi.table.get(&r)?.0.clone() else {
            return Ok(Err(ErrorCode::AccessDenied));
        };
        if name.is_empty() || name.len() > 255 || name.contains('\0') {
            return Ok(Err(ErrorCode::InvalidArgument));
        }
        match self.host.resolve_addresses(&*grant.handle, &name) {
            Ok(v) => Ok(Ok(self.push(Addresses(v.into()))?)),
            Err(e) => Ok(Err(e)),
        }
    }
}
impl ip_name_lookup::HostResolveAddressStream for Context {
    fn resolve_next_address(
        &mut self,
        r: Resource<Addresses>,
    ) -> Result<NetResult<Option<IpAddress>>> {
        Ok(Ok(self.wasi.table.get_mut(&r)?.0.pop_front()))
    }
    fn subscribe(&mut self, r: Resource<Addresses>) -> Result<Resource<Pollable>> {
        self.wasi.table.get(&r)?;
        self.push(Pollable::Ready)
    }
    fn drop(&mut self, r: Resource<Addresses>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl tcp::Host for Context {}
impl tcp_create_socket::Host for Context {
    fn create_tcp_socket(&mut self, family: IpAddressFamily) -> Result<NetResult<Resource<Tcp>>> {
        Ok(Ok(self.push(Tcp {
            family,
            phase: TcpPhase::Initial,
            network: None,
            endpoint: None,
            local: None,
            remote: None,
        })?))
    }
}
macro_rules! tcp_unsupported {($($name:ident($($arg:ident:$ty:ty),*)->$ret:ty;)+)=>{$(fn $name(&mut self,r:Resource<Tcp>,$($arg:$ty),*)->Result<NetResult<$ret>>{self.wasi.table.get(&r)?;let _=($($arg,)*);Ok(Err(ErrorCode::NotSupported))})+};}
impl tcp::HostTcpSocket for Context {
    fn start_bind(
        &mut self,
        r: Resource<Tcp>,
        n: Resource<Network>,
        address: IpSocketAddress,
    ) -> Result<NetResult<()>> {
        let Some(grant) = self.wasi.table.get(&n)?.0.clone() else {
            return Ok(Err(ErrorCode::AccessDenied));
        };
        let socket = self.wasi.table.get_mut(&r)?;
        if !matches!(socket.phase, TcpPhase::Initial) {
            return Ok(Err(ErrorCode::InvalidState));
        }
        if !family_matches(socket.family, &address) {
            return Ok(Err(ErrorCode::InvalidArgument));
        }
        socket.network = Some(grant);
        socket.local = Some(address);
        socket.phase = TcpPhase::Binding;
        Ok(Ok(()))
    }
    fn finish_bind(&mut self, r: Resource<Tcp>) -> Result<NetResult<()>> {
        let s = self.wasi.table.get_mut(&r)?;
        if !matches!(s.phase, TcpPhase::Binding) {
            return Ok(Err(ErrorCode::NotInProgress));
        }
        s.phase = TcpPhase::Bound;
        Ok(Ok(()))
    }
    fn start_connect(
        &mut self,
        r: Resource<Tcp>,
        n: Resource<Network>,
        address: IpSocketAddress,
    ) -> Result<NetResult<()>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let Some(grant) = self.wasi.table.get(&n)?.0.clone() else {
            return Ok(Err(ErrorCode::AccessDenied));
        };
        let s = self.wasi.table.get_mut(&r)?;
        if !matches!(s.phase, TcpPhase::Initial) {
            return Ok(Err(ErrorCode::NotSupported));
        }
        if !family_matches(s.family, &address) {
            return Ok(Err(ErrorCode::InvalidArgument));
        }
        match self.host.connect_tcp(&*grant.handle, &address) {
            Ok((entry, local)) => {
                s.endpoint = Some(entry);
                s.local = Some(local);
                s.remote = Some(address);
                s.phase = TcpPhase::Connecting;
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e)),
        }
    }
    fn finish_connect(
        &mut self,
        r: Resource<Tcp>,
    ) -> Result<NetResult<(Resource<Input>, Resource<Output>)>> {
        let s = self.wasi.table.get_mut(&r)?;
        if !matches!(s.phase, TcpPhase::Connecting) {
            return Ok(Err(ErrorCode::NotInProgress));
        }
        let e = s.endpoint.as_ref().unwrap().clone();
        let mut reservation = self.reserve_resources(2)?;
        let (input, output) = self.push_pair(
            Input::Socket(e.handle.clone()),
            Output::socket(e.handle),
            &mut reservation,
        )?;
        self.wasi.table.get_mut(&r)?.phase = TcpPhase::Connected;
        Ok(Ok((input, output)))
    }
    fn start_listen(&mut self, r: Resource<Tcp>) -> Result<NetResult<()>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let s = self.wasi.table.get_mut(&r)?;
        if !matches!(s.phase, TcpPhase::Bound) {
            return Ok(Err(ErrorCode::InvalidState));
        }
        match self.host.listen_tcp(
            &*s.network.as_ref().unwrap().handle,
            s.local.as_ref().unwrap(),
        ) {
            Ok(e) => {
                s.endpoint = Some(e);
                s.phase = TcpPhase::ListeningStart;
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e)),
        }
    }
    fn finish_listen(&mut self, r: Resource<Tcp>) -> Result<NetResult<()>> {
        let s = self.wasi.table.get_mut(&r)?;
        if !matches!(s.phase, TcpPhase::ListeningStart) {
            return Ok(Err(ErrorCode::NotInProgress));
        }
        s.phase = TcpPhase::Listening;
        Ok(Ok(()))
    }
    fn accept(
        &mut self,
        r: Resource<Tcp>,
    ) -> Result<NetResult<(Resource<Tcp>, Resource<Input>, Resource<Output>)>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let mut reservation = self.reserve_resources(3)?;
        let s = self.wasi.table.get(&r)?;
        if !matches!(s.phase, TcpPhase::Listening) {
            return Ok(Err(ErrorCode::InvalidState));
        }
        let (entry, remote) = match self.host.accept_tcp(&*s.endpoint.as_ref().unwrap().handle) {
            Ok(v) => v,
            Err(e) => return Ok(Err(e)),
        };
        let new = Tcp {
            family: s.family,
            phase: TcpPhase::Connected,
            network: None,
            endpoint: Some(entry.clone()),
            local: s.local.clone(),
            remote: Some(remote),
        };
        let socket = self.push_reserved(new, &mut reservation)?;
        match self.push_pair(
            Input::Socket(entry.handle.clone()),
            Output::socket(entry.handle),
            &mut reservation,
        ) {
            Ok((input, output)) => Ok(Ok((socket, input, output))),
            Err(error) => {
                self.delete(socket)?;
                Err(error)
            }
        }
    }
    fn local_address(&mut self, r: Resource<Tcp>) -> Result<NetResult<IpSocketAddress>> {
        Ok(self
            .wasi
            .table
            .get(&r)?
            .local
            .clone()
            .ok_or(ErrorCode::InvalidState))
    }
    fn remote_address(&mut self, r: Resource<Tcp>) -> Result<NetResult<IpSocketAddress>> {
        Ok(self
            .wasi
            .table
            .get(&r)?
            .remote
            .clone()
            .ok_or(ErrorCode::InvalidState))
    }
    fn is_listening(&mut self, r: Resource<Tcp>) -> Result<bool> {
        Ok(matches!(
            self.wasi.table.get(&r)?.phase,
            TcpPhase::Listening
        ))
    }
    fn address_family(&mut self, r: Resource<Tcp>) -> Result<IpAddressFamily> {
        Ok(self.wasi.table.get(&r)?.family)
    }
    fn subscribe(&mut self, r: Resource<Tcp>) -> Result<Resource<Pollable>> {
        let s = self.wasi.table.get(&r)?;
        let p = if matches!(s.phase, TcpPhase::Connected) {
            Pollable::Socket(s.endpoint.as_ref().unwrap().handle.clone(), false)
        } else if matches!(s.phase, TcpPhase::Listening) {
            Pollable::Network(s.endpoint.as_ref().unwrap().handle.clone(), false, false)
        } else {
            Pollable::Ready
        };
        self.push(p)
    }
    fn shutdown(&mut self, r: Resource<Tcp>, how: tcp::ShutdownType) -> Result<NetResult<()>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let s = self.wasi.table.get(&r)?;
        let Some(e) = &s.endpoint else {
            return Ok(Err(ErrorCode::InvalidState));
        };
        Ok(self.host.shutdown_socket(&*e.handle, how))
    }
    tcp_unsupported! {
     set_listen_backlog_size(value:u64)->();keep_alive_enabled()->bool;set_keep_alive_enabled(value:bool)->();keep_alive_idle_time()->u64;set_keep_alive_idle_time(value:u64)->();keep_alive_interval()->u64;set_keep_alive_interval(value:u64)->();keep_alive_count()->u32;set_keep_alive_count(value:u32)->();hop_limit()->u8;set_hop_limit(value:u8)->();receive_buffer_size()->u64;set_receive_buffer_size(value:u64)->();send_buffer_size()->u64;set_send_buffer_size(value:u64)->();
    }
    fn drop(&mut self, r: Resource<Tcp>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl udp::Host for Context {}
impl udp_create_socket::Host for Context {
    fn create_udp_socket(&mut self, family: IpAddressFamily) -> Result<NetResult<Resource<Udp>>> {
        Ok(Ok(self.push(Udp {
            family,
            endpoint: None,
            local: None,
            remote: None,
            binding: false,
            streamed: false,
        })?))
    }
}
macro_rules! udp_unsupported {($($name:ident($($arg:ident:$ty:ty),*)->$ret:ty;)+)=>{$(fn $name(&mut self,r:Resource<Udp>,$($arg:$ty),*)->Result<NetResult<$ret>>{self.wasi.table.get(&r)?;let _=($($arg,)*);Ok(Err(ErrorCode::NotSupported))})+};}
impl udp::HostUdpSocket for Context {
    fn start_bind(
        &mut self,
        r: Resource<Udp>,
        n: Resource<Network>,
        address: IpSocketAddress,
    ) -> Result<NetResult<()>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let Some(grant) = self.wasi.table.get(&n)?.0.clone() else {
            return Ok(Err(ErrorCode::AccessDenied));
        };
        let s = self.wasi.table.get_mut(&r)?;
        if s.endpoint.is_some() {
            return Ok(Err(ErrorCode::InvalidState));
        }
        if !family_matches(s.family, &address) {
            return Ok(Err(ErrorCode::InvalidArgument));
        }
        match self.host.bind_udp(&*grant.handle, &address) {
            Ok(entry) => {
                s.endpoint = Some(entry);
                s.local = Some(address);
                s.binding = true;
                Ok(Ok(()))
            }
            Err(e) => Ok(Err(e)),
        }
    }
    fn finish_bind(&mut self, r: Resource<Udp>) -> Result<NetResult<()>> {
        let s = self.wasi.table.get_mut(&r)?;
        if !s.binding {
            return Ok(Err(ErrorCode::NotInProgress));
        }
        s.binding = false;
        Ok(Ok(()))
    }
    fn stream(
        &mut self,
        r: Resource<Udp>,
        remote: Option<IpSocketAddress>,
    ) -> Result<NetResult<(Resource<Incoming>, Resource<Outgoing>)>> {
        let s = self.wasi.table.get_mut(&r)?;
        if s.binding || s.streamed {
            return Ok(Err(ErrorCode::InvalidState));
        }
        let Some(entry) = s.endpoint.clone() else {
            return Ok(Err(ErrorCode::InvalidState));
        };
        if remote
            .as_ref()
            .is_some_and(|a| !family_matches(s.family, a))
        {
            return Ok(Err(ErrorCode::InvalidArgument));
        }
        let mut reservation = self.reserve_resources(2)?;
        let (input, output) = self.push_pair(
            Incoming {
                entry: entry.clone(),
                remote: remote.clone(),
            },
            Outgoing {
                entry,
                remote: remote.clone(),
                permit: 0,
            },
            &mut reservation,
        )?;
        let s = self.wasi.table.get_mut(&r)?;
        s.streamed = true;
        s.remote = remote;
        Ok(Ok((input, output)))
    }
    fn local_address(&mut self, r: Resource<Udp>) -> Result<NetResult<IpSocketAddress>> {
        Ok(self
            .wasi
            .table
            .get(&r)?
            .local
            .clone()
            .ok_or(ErrorCode::InvalidState))
    }
    fn remote_address(&mut self, r: Resource<Udp>) -> Result<NetResult<IpSocketAddress>> {
        Ok(self
            .wasi
            .table
            .get(&r)?
            .remote
            .clone()
            .ok_or(ErrorCode::InvalidState))
    }
    fn address_family(&mut self, r: Resource<Udp>) -> Result<IpAddressFamily> {
        Ok(self.wasi.table.get(&r)?.family)
    }
    fn subscribe(&mut self, r: Resource<Udp>) -> Result<Resource<Pollable>> {
        self.wasi.table.get(&r)?;
        self.push(Pollable::Ready)
    }
    udp_unsupported! {unicast_hop_limit()->u8;set_unicast_hop_limit(value:u8)->();receive_buffer_size()->u64;set_receive_buffer_size(value:u64)->();send_buffer_size()->u64;set_send_buffer_size(value:u64)->();}
    fn drop(&mut self, r: Resource<Udp>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl udp::HostIncomingDatagramStream for Context {
    fn receive(
        &mut self,
        r: Resource<Incoming>,
        maximum: u64,
    ) -> Result<NetResult<Vec<udp::IncomingDatagram>>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let s = self.wasi.table.get(&r)?;
        if maximum == 0 {
            return Ok(Ok(Vec::new()));
        }
        Ok(self.host.receive_udp(
            &*s.entry.handle,
            maximum.min(16) as usize,
            s.remote.as_ref(),
        ))
    }
    fn subscribe(&mut self, r: Resource<Incoming>) -> Result<Resource<Pollable>> {
        let handle = self.wasi.table.get(&r)?.entry.handle.clone();
        self.push(Pollable::Network(handle, true, false))
    }
    fn drop(&mut self, r: Resource<Incoming>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
impl udp::HostOutgoingDatagramStream for Context {
    fn check_send(&mut self, r: Resource<Outgoing>) -> Result<NetResult<u64>> {
        let s = self.wasi.table.get_mut(&r)?;
        match self.host.network_ready(&*s.entry.handle, true, true) {
            Ok(ready) => {
                s.permit = if ready { 16 } else { 0 };
                Ok(Ok(s.permit))
            }
            Err(error) => {
                s.permit = 0;
                Ok(Err(error))
            }
        }
    }
    fn send(
        &mut self,
        r: Resource<Outgoing>,
        datagrams: Vec<udp::OutgoingDatagram>,
    ) -> Result<NetResult<u64>> {
        if self.restoring {
            wasmtime::bail!("external WASI operation during restore");
        }

        let s = self.wasi.table.get_mut(&r)?;
        let permit = std::mem::take(&mut s.permit);
        if datagrams.len() as u64 > permit {
            wasmtime::bail!("send exceeds check-send permit");
        }
        if datagrams.iter().any(|d| d.data.len() > 8192) {
            return Ok(Err(ErrorCode::DatagramTooLarge));
        }
        let s = self.wasi.table.get(&r)?;
        Ok(self
            .host
            .send_udp(&*s.entry.handle, &datagrams, s.remote.as_ref()))
    }
    fn subscribe(&mut self, r: Resource<Outgoing>) -> Result<Resource<Pollable>> {
        let handle = self.wasi.table.get(&r)?.entry.handle.clone();
        self.push(Pollable::Network(handle, true, true))
    }
    fn drop(&mut self, r: Resource<Outgoing>) -> Result<()> {
        self.delete(r)?;
        Ok(())
    }
}
