use alloc::vec::Vec;
use bexos_d1_cmos::{Cmos, Registers, RtcError};
struct Ports;
impl Registers for Ports {
    fn read(&self, register: u8) -> Result<u8, RtcError> {
        bexos_userspace::syscall::cmos(register, None).map_err(|_| RtcError::Io)
    }
    fn write(&mut self, register: u8, value: u8) -> Result<(), RtcError> {
        bexos_userspace::syscall::cmos(register, Some(value))
            .map(|_| ())
            .map_err(|_| RtcError::Io)
    }
}
use bexos_userspace::{
    Channel, Memory, Startup,
    live_migration::Source,
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
mod migration;
mod protocol;

pub(crate) fn run(channel: u64) -> ! {
    let manager = Channel(channel);
    let startup = Startup::receive(manager).unwrap();
    if startup.migration_target {
        match bexos_userspace::live_migration::receive::<Runtime>(
            manager,
            startup.migration_generation,
        ) {
            Ok(state) => serve(state),
            Err(_) => bexos_userspace::exit(),
        }
    }
    Cmos::new(Ports)
        .read_utc_ns()
        .expect("valid Q35 CMOS clock");
    Startup::ready(manager).unwrap();
    bexos_userspace::log("cmos: RTC driver ready\n");
    serve(Runtime {
        manager,
        migration: startup.migration,
        initialized: true,
        endpoints: Vec::new(),
        requests: 0,
    })
}

pub struct Runtime {
    manager: Channel,
    migration: Option<Channel>,
    initialized: bool,
    endpoints: Vec<BoundServiceEndpoint>,
    requests: u64,
}

impl Runtime {
    fn rtc(&self) -> Option<Cmos<Ports>> {
        self.initialized.then(|| Cmos::new(Ports))
    }
}

fn serve(mut state: Runtime) -> ! {
    let mut source = Source::new(state.migration);
    loop {
        if source.poll(&state).is_err() {
            let _ = bexos_userspace::migration::abort();
        }
        if source.quiescing() {
            bexos_userspace::yield_now();
            continue;
        }
        if let Ok(message) = state.manager.try_recv() {
            if let (Some(endpoint), Ok(metadata)) = (
                message.handles.first().copied(),
                core::str::from_utf8(&message.bytes),
            ) {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("RtcHardware") {
                        state.endpoints.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                        source.changed(0);
                    }
                } else if metadata.split('|').nth(1) == Some("RtcHardware") {
                    let _ = Memory::close(endpoint);
                }
            }
            state.requests = state.requests.wrapping_add(1);
            source.changed(0);
        }
        let before = (state.requests, state.endpoints.len());
        protocol::poll_endpoints(&mut state);
        if before != (state.requests, state.endpoints.len()) {
            source.changed(0);
        }
        bexos_userspace::yield_now();
    }
}
