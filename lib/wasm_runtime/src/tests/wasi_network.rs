use super::*;
use crate::resources::{Kind, READ, WRITE};
use crate::wasi::{
    sockets::*,
    state::Pollable,
    wasi::{
        io::poll::HostPollable,
        sockets::{
            instance_network, ip_name_lookup,
            network::*,
            tcp::HostTcpSocket,
            udp::{
                HostIncomingDatagramStream, HostOutgoingDatagramStream, HostUdpSocket,
                OutgoingDatagram,
            },
        },
    },
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use wasmtime::component::Resource;
struct Endpoint;
impl Handle for Endpoint {
    fn kind(&self) -> Kind {
        Kind::Channel
    }
    fn rights(&self) -> u32 {
        READ | WRITE
    }
    fn native(&self) -> u64 {
        77
    }
}
fn entry() -> Entry {
    Entry {
        name: String::new(),
        handle: Arc::new(Endpoint),
    }
}
#[derive(Default)]
struct NetworkHost {
    calls: AtomicUsize,
    ready: AtomicBool,
}
impl crate::host::Host for NetworkHost {
    fn monotonic_ns(&self) -> u64 {
        0
    }
    fn random(&self, n: usize) -> wasmtime::Result<Vec<u8>> {
        Ok(vec![42; n])
    }
    fn listen_tcp(&self, _: &dyn Handle, _: &IpSocketAddress) -> NetResult<Entry> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(entry())
    }
    fn log(&self, _: &[u8]) {}
    fn channel_write(&self, _: &dyn Handle, _: &[u8], _: &[Entry]) -> wasmtime::Result<()> {
        panic!()
    }
    fn channel_read(
        &self,
        _: &dyn Handle,
        _: usize,
        _: usize,
    ) -> wasmtime::Result<(Vec<u8>, Vec<Arc<dyn Handle>>)> {
        panic!()
    }
    fn resolve_addresses(&self, _: &dyn Handle, _: &str) -> NetResult<Vec<IpAddress>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(vec![IpAddress::Ipv4((127, 0, 0, 1))])
    }
    fn connect_tcp(
        &self,
        _: &dyn Handle,
        _: &IpSocketAddress,
    ) -> NetResult<(Entry, IpSocketAddress)> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok((entry(), address()))
    }
    fn bind_udp(&self, _: &dyn Handle, _: &IpSocketAddress) -> NetResult<Entry> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(entry())
    }
    fn accept_tcp(&self, _: &dyn Handle) -> NetResult<(Entry, IpSocketAddress)> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok((entry(), address()))
    }
    fn network_ready(&self, _: &dyn Handle, _: bool, _: bool) -> NetResult<bool> {
        Ok(self.ready.load(Ordering::Relaxed))
    }
    fn send_udp(
        &self,
        _: &dyn Handle,
        d: &[OutgoingDatagram],
        _: Option<&IpSocketAddress>,
    ) -> NetResult<u64> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(d.len() as u64)
    }
}
fn address() -> IpSocketAddress {
    IpSocketAddress::Ipv4(Ipv4SocketAddress {
        address: (127, 0, 0, 1),
        port: 1234,
    })
}
fn tcp(phase: TcpPhase) -> Tcp {
    Tcp {
        family: IpAddressFamily::Ipv4,
        phase,
        endpoint: Some(entry()),
        network: None,
        local: Some(address()),
        remote: None,
    }
}
fn udp() -> Udp {
    Udp {
        family: IpAddressFamily::Ipv4,
        endpoint: Some(entry()),
        local: Some(address()),
        remote: None,
        binding: false,
        streamed: false,
    }
}
fn setup(max_handles: u64) -> (Context, Arc<NetworkHost>) {
    let host = Arc::new(NetworkHost::default());
    let mut options = context().options;
    options.limits.max_handles = max_handles;
    let budget = Budget::for_limits(&options.limits);
    (
        Context::new(options, host.clone(), Origin::Signed, budget),
        host,
    )
}
#[test]
fn missing_network_grant_denies_tcp_udp_and_dns_before_host_calls() {
    let (mut ctx, host) = setup(16);
    let network = instance_network::Host::instance_network(&mut ctx).unwrap();
    let n = network.rep();
    let socket = ctx.push(tcp(TcpPhase::Initial)).unwrap();
    assert!(matches!(
        ctx.start_connect(socket, Resource::new_borrow(n), address())
            .unwrap(),
        Err(ErrorCode::AccessDenied)
    ));
    let socket = ctx.push(udp()).unwrap();
    assert!(matches!(
        HostUdpSocket::start_bind(&mut ctx, socket, Resource::new_borrow(n), address()).unwrap(),
        Err(ErrorCode::AccessDenied)
    ));
    assert!(matches!(
        ip_name_lookup::Host::resolve_addresses(&mut ctx, network, "example.test".into()).unwrap(),
        Err(ErrorCode::AccessDenied)
    ));
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
}
#[test]
fn socket_stream_allocation_is_atomic_and_accept_reserves_before_consuming() {
    let (mut ctx, host) = setup(2);
    let socket = ctx.push(tcp(TcpPhase::Connecting)).unwrap();
    let id = socket.rep();
    assert!(ctx.finish_connect(Resource::new_borrow(id)).is_err());
    assert_eq!(ctx.wasi.count, 1);
    assert!(matches!(
        ctx.wasi
            .table
            .get(&Resource::<Tcp>::new_borrow(id))
            .unwrap()
            .phase,
        TcpPhase::Connecting
    ));
    ctx.wasi
        .table
        .get_mut(&Resource::<Tcp>::new_borrow(id))
        .unwrap()
        .phase = TcpPhase::Listening;
    assert!(ctx.accept(socket).is_err());
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
    assert_eq!(ctx.wasi.count, 1);
    let spare = ctx.push(Pollable::Ready).unwrap();
    ctx.delete(spare).unwrap();
    ctx.delete(Resource::<Tcp>::new_own(id)).unwrap();
    let socket = ctx.push(udp()).unwrap();
    let id = socket.rep();
    assert!(HostUdpSocket::stream(&mut ctx, socket, None).is_err());
    assert!(
        !ctx.wasi
            .table
            .get(&Resource::<Udp>::new_borrow(id))
            .unwrap()
            .streamed
    );
    assert_eq!(ctx.wasi.count, 1);
}
#[test]
fn network_polling_tracks_readiness_and_udp_requires_fresh_send_credit() {
    let (mut ctx, host) = setup(16);
    let listener = ctx.push(tcp(TcpPhase::Listening)).unwrap();
    let poll = HostTcpSocket::subscribe(&mut ctx, listener).unwrap();
    let p = poll.rep();
    assert!(!ctx.ready(Resource::new_borrow(p)).unwrap());
    host.ready.store(true, Ordering::Relaxed);
    assert!(ctx.ready(Resource::new_borrow(p)).unwrap());
    let socket = ctx.push(udp()).unwrap();
    let (input, output) = HostUdpSocket::stream(&mut ctx, socket, None)
        .unwrap()
        .unwrap();
    let out = output.rep();
    let poll = HostIncomingDatagramStream::subscribe(&mut ctx, input).unwrap();
    host.ready.store(false, Ordering::Relaxed);
    assert!(!ctx.ready(poll).unwrap());
    assert_eq!(
        ctx.check_send(Resource::new_borrow(out)).unwrap().unwrap(),
        0
    );
    let datagram = || {
        vec![OutgoingDatagram {
            data: vec![1],
            remote_address: Some(address()),
        }]
    };
    assert!(ctx.send(Resource::new_borrow(out), datagram()).is_err());
    assert_eq!(host.calls.load(Ordering::Relaxed), 0);
    host.ready.store(true, Ordering::Relaxed);
    assert_eq!(
        ctx.check_send(Resource::new_borrow(out)).unwrap().unwrap(),
        16
    );
    assert_eq!(
        ctx.send(Resource::new_borrow(out), datagram())
            .unwrap()
            .unwrap(),
        1
    );
    assert!(ctx.send(output, datagram()).is_err());
    assert_eq!(host.calls.load(Ordering::Relaxed), 1);
}

#[test]
fn network_name_does_not_authorize_wrong_type_or_insufficient_rights() {
    struct Forged(Kind, u32);
    impl Handle for Forged {
        fn kind(&self) -> Kind {
            self.0
        }
        fn rights(&self) -> u32 {
            self.1
        }
        fn native(&self) -> u64 {
            88
        }
    }
    for (kind, rights) in [
        (Kind::File, READ | WRITE),
        (Kind::Channel, READ),
        (Kind::Channel, WRITE),
    ] {
        let (mut ctx, host) = setup(16);
        ctx.resources
            .insert(Entry {
                name: "bexos.net.Netstack".into(),
                handle: Arc::new(Forged(kind, rights)),
            })
            .unwrap();
        let network = instance_network::Host::instance_network(&mut ctx).unwrap();
        assert!(matches!(
            ip_name_lookup::Host::resolve_addresses(&mut ctx, network, "example.test".into())
                .unwrap(),
            Err(ErrorCode::AccessDenied)
        ));
        assert_eq!(host.calls.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn real_component_checks_tcp_udp_and_dns_capability_boundaries() {
    let _lock = TEST_ENGINE.lock().unwrap();
    let engine = crate::engine::engine().unwrap();
    let bytes = wat::parse_str(include_str!("../../../../testing/wasm/network.wat")).unwrap();
    for granted in [false, true] {
        let (mut ctx, host) = setup(32);
        if granted {
            ctx.resources
                .insert(Entry {
                    name: "bexos.net.Netstack".into(),
                    handle: Arc::new(Endpoint),
                })
                .unwrap();
        }
        let mut command = run(crate::component::CommandInstance::instantiate(
            &engine,
            bytes.clone().into(),
            ctx,
        ))
        .unwrap();
        assert_eq!(run(command.run()).unwrap(), 0);
        assert_eq!(
            host.calls.load(Ordering::Relaxed),
            if granted { 3 } else { 0 }
        );
    }
}
