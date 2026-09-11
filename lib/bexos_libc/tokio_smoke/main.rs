#![no_main]

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

bexos_libc::entry!(run);

fn run(channel: u64) -> ! {
    let control = bexos_userspace::Channel(channel);
    if let Ok(startup) = bexos_userspace::Startup::receive(control) {
        bexos_libc::install_startup(&startup);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_io()
        .enable_time()
        .build()
        .expect("tokio runtime");

    runtime.block_on(async {
        let joined = tokio::spawn(async { 41usize + 1 }).await.expect("join");
        assert_eq!(joined, 42);

        tokio::time::sleep(Duration::from_millis(1)).await;

        let _ = tokio::fs::write("tokio-smoke.txt", b"tokio on bexos\n").await;
        let _ = tokio::fs::read("tokio-smoke.txt").await;

        let loopback = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));
        let _ = tokio::net::TcpListener::bind(loopback).await;
        let _ = tokio::net::UdpSocket::bind(loopback).await;
    });

    let _ = bexos_userspace::Startup::ready(control);
    loop {
        bexos_userspace::yield_now();
    }
}
