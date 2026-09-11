//! Non-secure, local echo fixture exercises actual QEMU virtio-net packets.
use bexos_userspace::{Channel, Startup, log};
use std::io::{Read, Write};
pub fn verify(startup: &Startup) {
    let grant = startup
        .service_grants
        .iter()
        .find(|grant| grant.service == "bexos.net.Netstack")
        .expect("network fixture grant");
    let socket = bexos_net::secure::connect_tcp_addr(
        Channel(grant.endpoint),
        net_fidl::IpAddress::Ipv4(net_fidl::Ipv4Address {
            octets: [10, 0, 2, 2],
        }),
        u16::try_from(startup.arg0 & 0xffff).unwrap(),
    )
    .expect("connect to host echo fixture");
    let mut stream = bexos_net::secure::BexosSocketIo::new(socket);
    for block in 0..if startup.arg0 >> 32 == 1 { 16u8 } else { 8u8 } {
        let payload: Vec<_> = (0..4096)
            .map(|index| (index as u8).wrapping_add(block))
            .collect();
        let deadline = bexos_userspace::syscall::ticks()
            + (if startup.arg0 >> 32 == 1 { 300 } else { 15 })
                * bexos_userspace::syscall::frequency();
        let mut written = 0;
        while written < payload.len() {
            match stream.write(&payload[written..]) {
                Ok(0) => panic!("TCP fixture closed during write"),
                Ok(n) => written += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("send TCP fixture: {e}"),
            }
            assert!(
                bexos_userspace::syscall::ticks() < deadline,
                "TCP write deadline"
            );
            bexos_userspace::yield_now();
        }
        let mut reply = vec![0; payload.len()];
        let mut received = 0;
        while received < reply.len() {
            match stream.read(&mut reply[received..]) {
                Ok(0) => panic!("TCP fixture closed during read"),
                Ok(n) => received += n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("receive TCP fixture: {e}"),
            }
            assert!(
                bexos_userspace::syscall::ticks() < deadline,
                "TCP echo deadline"
            );
            bexos_userspace::yield_now();
        }
        assert_eq!(reply, payload);
        if block == 7 {
            log("elf-probe: 32 KiB TCP echo over virtio-net verified\n");
        }
    }
    if startup.arg0 >> 32 == 1 {
        log("elf-probe: retained TCP stream survived network replacements\n");
    }
}
