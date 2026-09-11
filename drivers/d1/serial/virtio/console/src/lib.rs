mod serial;
extern crate alloc;
mod exchange;
mod hardware;
mod migration;
mod transport;
use bexos_userspace::live_migration::Source;
use bexos_userspace::{Channel, HardwareResourceKind, Memory, Startup, log};
use kernel_fidl::Status;
use rpmb_fidl::{
    FidlDecode, FidlEncode, RpmbTransportExchangeRequest, RpmbTransportExchangeResponse,
};

pub fn main(channel: u64) -> ! {
    std::panic::set_hook(Box::new(|info| {
        log(&format!("virtio-console: panic: {info}\n"));
    }));
    log("virtio-console: startup entered\n");
    let control = Channel(channel);
    let startup = Startup::receive(control).expect("virtio-console startup");
    let mut state = if startup.migration_target {
        bexos_userspace::live_migration::receive::<migration::Runtime>(
            control,
            startup.migration_generation,
        )
        .expect("virtio-console adoption")
    } else {
        let domain = startup
            .driver_resources
            .iter()
            .find(|r| r.kind == HardwareResourceKind::IommuDomain)
            .expect("virtio-console IOMMU domain")
            .handle;
        bexos_virtio_hal::set_iommu_domain(domain);
        let mut hardware =
            hardware::Hardware::connect(startup.arg1).expect("virtio-console device");
        let end = transport::deadline();
        while !hardware.console.ready() {
            hardware
                .console
                .poll_control()
                .expect("virtio-console port negotiation");
            assert!(!transport::expired(end), "virtio-console rpmb0 missing");
            bexos_userspace::yield_now();
        }
        log(if hardware.console.debug_port() {
            "virtio-console: debug0 ready\n"
        } else {
            "virtio-console: rpmb0 ready\n"
        });
        Startup::ready(control).unwrap();
        migration::Runtime::new(
            control,
            startup.migration,
            startup.driver_lifecycle,
            hardware,
        )
    };
    let mut source = Source::new(state.migration);
    loop {
        if !source.quiescing() {
            if let Some(hardware) = &mut state.hardware {
                if hardware.console.tx_pending() {
                    match hardware.console.poll_send() {
                        Ok(true) => source.changed_keys([1, 2]),
                        Ok(false) => {}
                        Err(_) => {
                            hardware.healthy = false;
                            source.changed(1);
                        }
                    }
                }
            }
        }
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if let Ok(message) = control.try_recv() {
            if message.bytes == b"bexos.serial.role" && message.handles.is_empty() {
                let _ = control.send(state.hardware.as_ref().unwrap().console.port_name(), &[]);
                continue;
            }
            // Only appd holds this bootstrap endpoint. It delegates a private
            // endpoint to teed; arbitrary public raw-frame access is absent.
            if message.bytes
                == if state.hardware.as_ref().unwrap().console.debug_port() {
                    b"bexos.serial.bind".as_slice()
                } else {
                    b"bexos.rpmb.bind".as_slice()
                }
                && message.handles.len() == 1
            {
                state.clients.push(Channel(message.handles[0]));
            } else {
                for handle in message.handles {
                    let _ = Memory::close(handle);
                }
            }
            source.changed(0);
        }
        let clients = core::mem::take(&mut state.clients);
        for client in clients {
            match client.try_recv() {
                Ok(message) => {
                    if state.hardware.as_ref().unwrap().console.debug_port() {
                        serial::request(state.hardware.as_mut().unwrap(), client, &message.bytes);
                        for handle in message.handles {
                            let _ = Memory::close(handle);
                        }
                        source.changed_keys([0, 1, 2]);
                        state.clients.push(client);
                        continue;
                    }
                    let mut bytes = alloc::vec![0; 8192];
                    let result = if !message.handles.is_empty()
                        || message.bytes.len() < 8
                        || message.bytes[..8] != 1u64.to_le_bytes()
                    {
                        Err(Status::ErrInvalidArgs)
                    } else {
                        RpmbTransportExchangeRequest::decode(&message.bytes[8..], &[])
                            .map_err(|_| Status::ErrInvalidArgs)
                            .and_then(|request| {
                                transfer(
                                    state.hardware.as_mut().unwrap(),
                                    request.frames,
                                    request.read_frames as usize,
                                )
                            })
                    };
                    for handle in message.handles {
                        let _ = Memory::close(handle);
                    }
                    let (status, frames) = match result {
                        Ok(frames) => (0, frames),
                        Err(status) => (status as i32, alloc::vec![]),
                    };
                    let response = RpmbTransportExchangeResponse {
                        status,
                        frames: &frames,
                    };
                    if let Ok(encoded) = response.encode(&mut bytes, &mut []) {
                        let _ = client.send(&bytes[..encoded.bytes], &[]);
                    }
                    source.changed_keys([0, 1, 2]);
                    state.clients.push(client);
                }
                Err(Status::ErrPeerClosed) => {
                    let _ = Memory::close(client.0);
                    source.changed(0);
                }
                _ => state.clients.push(client),
            }
        }
        if let Some(lifecycle) = state.lifecycle {
            poll_lifecycle(lifecycle);
        }
        bexos_userspace::yield_now();
    }
}

fn transfer(
    hardware: &mut hardware::Hardware,
    frames: &[u8],
    count: usize,
) -> Result<alloc::vec::Vec<u8>, Status> {
    bexos_rpmb_proxy::validate_frames(frames, count).map_err(|_| Status::ErrInvalidArgs)?;
    if !hardware.healthy {
        return Err(Status::ErrPeerClosed);
    }
    struct Live<'a> {
        console: &'a mut transport::Console,
        end: u64,
    }
    impl exchange::Stream for Live<'_> {
        fn send(&mut self, bytes: &[u8]) -> Result<(), Status> {
            self.console.send(bytes)
        }
        fn progress(&mut self) -> Result<bool, Status> {
            self.console.poll_control()?;
            if !self.console.ready() {
                return Err(Status::ErrPeerClosed);
            }
            self.console.poll_send()
        }
        fn receive(&mut self, out: &mut [u8]) -> Result<usize, Status> {
            self.console.receive(out)
        }
        fn expired(&self) -> bool {
            transport::expired(self.end)
        }
        fn yield_once(&self) {
            bexos_userspace::yield_now();
        }
    }
    let result = exchange::exchange(
        &mut Live {
            console: &mut hardware.console,
            end: transport::deadline(),
        },
        frames,
        count,
    );
    // After submission, any transport/protocol failure can leave an unread
    // reply. Never retry a write or attribute its reply to the next exchange.
    // Invalid caller frames were rejected before touching the device above.
    if result.is_err() {
        hardware.healthy = false;
    }
    result
}

fn poll_lifecycle(channel: Channel) {
    use hardware_manager_fidl::{
        DriverLifecyclePrepareStopRequest, DriverLifecyclePrepareStopResponse, FidlDecode,
        FidlEncode, Status,
    };
    if let Ok(message) = channel.try_recv() {
        let has_handles = !message.handles.is_empty();
        for handle in message.handles {
            let _ = Memory::close(handle);
        }
        if has_handles || message.bytes.len() < 8 || message.bytes[..8] != 1u64.to_le_bytes() {
            return;
        }
        if DriverLifecyclePrepareStopRequest::decode(&message.bytes[8..], &[]).is_ok() {
            let response = DriverLifecyclePrepareStopResponse { status: Status::Ok };
            let mut bytes = [0; 64];
            if let Ok(encoded) = response.encode(&mut bytes, &mut []) {
                let _ = channel.send(&bytes[..encoded.bytes], &[]);
            }
        }
    }
}
