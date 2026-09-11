use bexos_debug_client::{DebugClient, UnixSocketTransport};
use std::io::{self, Write};
use std::time::{Duration, Instant};

const APP_PACKAGE: &str = "bexos.app.dioxus_demo";
const APP_PROCESS: &str = "dioxus_demo";

fn main() {
    let socket = std::env::args().nth(1).expect("debug socket");
    let mut client = DebugClient::new(UnixSocketTransport::connect(socket).unwrap());
    println!("dioxus-probe: connected to debug relay");
    io::stdout().flush().unwrap();
    client
        .health_check()
        .expect("debug health before Dioxus launch");
    println!("dioxus-probe: health ok");
    io::stdout().flush().unwrap();

    client.clear_received_trace();
    println!("dioxus-probe: launching {APP_PACKAGE}/{APP_PROCESS}");
    io::stdout().flush().unwrap();
    client
        .launch_app(APP_PACKAGE, APP_PROCESS, 0, 0)
        .expect("launch Dioxus demo");
    println!("dioxus-probe: launch accepted");
    io::stdout().flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        client.drain_for(Duration::from_millis(250)).unwrap();
        let trace = client.received_trace();
        assert!(
            !contains(trace, b"wasm_runner: failed:"),
            "Dioxus runner failed after launch:\n{}",
            String::from_utf8_lossy(trace)
        );
        let rendered = contains(
            trace,
            b"wasm_runner: composed component graph dependencies=1",
        ) && contains(trace, b"wasm_runner: service component instantiated")
            && contains(
                trace,
                b"wasm_runner: native UI first frame submitted backend=",
            );
        if rendered {
            println!("dioxus-probe: Dioxus WASM app launched and rendered a native UI frame");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Dioxus launch timed out:\n{}",
            String::from_utf8_lossy(client.received_trace())
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
