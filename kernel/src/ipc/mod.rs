use core::fmt::Write;

use bexos_kernel_core::ipc::{Capability, Channel, Endpoint, Message};

use crate::state::UART;

pub fn run_demo() {
    let mut channel = Channel::<4>::new();
    let capability = Capability {
        object_id: 0xcafe,
        rights: 0b11,
    };
    let payload = b"kernel-ipc".as_slice();
    let message = Message::new(payload, Some(capability)).expect("demo message fits");
    channel
        .send(Endpoint::A, message)
        .expect("demo channel has space");
    let received = channel
        .receive(Endpoint::B)
        .expect("demo receiver has message");

    UART.with(|slot| {
        if let Some(uart) = slot.as_mut() {
            let _ = writeln!(
                uart,
                "kernel: ipc transfer bytes={} cap=0x{:x} rights=0b{:b}",
                received.len,
                received.capability.map(|cap| cap.object_id).unwrap_or(0),
                received.capability.map(|cap| cap.rights).unwrap_or(0)
            );
        }
    });
}
