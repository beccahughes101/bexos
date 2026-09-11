#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use bexos_d1_rtc::{Mmio, PL031_BASE, PL031_SIZE, Pl031};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::{
    Channel, Memory, Startup,
    live_migration::{Resource, Source, State},
    service_binding::{BoundServiceEndpoint, ServiceBinding},
};
use time_fidl::{
    FidlDecode, FidlEncode, HandleRef, RtcHardwareReadUtcRequest, RtcHardwareReadUtcResponse,
    RtcHardwareWriteUtcRequest, RtcHardwareWriteUtcResponse, Status,
};

bexos_userspace::entry!(run);

fn run(channel: u64) -> ! {
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
    let handle = Memory::physical(PL031_BASE, PL031_SIZE).unwrap();
    let va = Memory::map(handle, PL031_SIZE, 6).unwrap();
    Startup::ready(manager).unwrap();
    bexos_userspace::log("pl031: RTC driver ready\n");
    serve(Runtime {
        manager,
        migration: startup.migration,
        mapping: Some((handle, va, PL031_SIZE)),
        endpoints: Vec::new(),
        requests: 0,
    })
}

pub struct Runtime {
    manager: Channel,
    migration: Option<Channel>,
    mapping: Option<(u64, u64, u64)>,
    endpoints: Vec<BoundServiceEndpoint>,
    requests: u64,
}

impl Runtime {
    fn rtc(&self) -> Option<Pl031<Mmio>> {
        self.mapping
            .map(|(_, va, _)| Pl031::new(unsafe { Mmio::new(va as usize) }))
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            manager: Channel(0),
            migration: None,
            mapping: None,
            endpoints: Vec::new(),
            requests: 0,
        }
    }

    fn keys(&self) -> Vec<u64> {
        alloc::vec![0]
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.manager.0);
        w.word(self.migration.map_or(0, |c| c.0));
        w.word(self.requests);
        w.word(self.mapping.is_some() as u64);
        if let Some((handle, va, size)) = self.mapping {
            w.word(handle);
            w.word(va);
            w.word(size);
        }
        w.word(self.endpoints.len() as u64);
        for endpoint in &self.endpoints {
            w.word(endpoint.channel.0);
            w.word(endpoint.allowed_methods.len() as u64);
            for ordinal in &endpoint.allowed_methods {
                w.word(*ordinal);
            }
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        self.manager = Channel(r.word()?);
        self.migration = nonzero_channel(r.word()?);
        self.requests = r.word()?;
        self.mapping = if r.flag()? {
            Some((r.word()?, r.word()?, r.word()?))
        } else {
            None
        };
        self.endpoints.clear();
        for _ in 0..r.count(64)? {
            let channel = Channel(r.word()?);
            let mut allowed = Vec::new();
            for _ in 0..r.count(64)? {
                allowed.push(r.word()?);
            }
            self.endpoints
                .push(BoundServiceEndpoint::new(channel, allowed));
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.manager.0 == 0 || self.migration.is_none() || self.mapping.is_none() {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.manager.0)];
        if let Some(channel) = self.migration {
            out.push(Resource::Handle(channel.0));
        }
        if let Some((handle, va, size)) = self.mapping {
            out.push(Resource::Mapping {
                handle,
                offset: 0,
                va,
                size,
                rights: 6,
            });
        }
        out.extend(
            self.endpoints
                .iter()
                .map(|endpoint| Resource::Handle(endpoint.channel.0)),
        );
        out
    }

    fn activated(&mut self, generation: u64) {
        bexos_userspace::log(&alloc::format!("pl031: adopted generation={generation}\n"));
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
        }
        poll_endpoints(&mut state);
        bexos_userspace::yield_now();
    }
}

fn poll_endpoints(state: &mut Runtime) {
    let endpoints = core::mem::take(&mut state.endpoints);
    for endpoint in endpoints {
        match endpoint.channel.try_recv() {
            Ok(message) => {
                let (ordinal, req) = envelope(&message.bytes);
                let handles: Vec<_> = message
                    .handles
                    .iter()
                    .map(|raw| HandleRef { raw: *raw })
                    .collect();
                match ordinal {
                    1 if endpoint.allows(ordinal) => {
                        reply(endpoint.channel, &read_utc_response(state, req, &handles))
                    }
                    2 if endpoint.allows(ordinal) => {
                        reply(endpoint.channel, &write_utc_response(state, req, &handles))
                    }
                    _ => {}
                }
                state.requests = state.requests.wrapping_add(1);
                state.endpoints.push(endpoint);
            }
            Err(kernel_fidl::Status::ErrPeerClosed) => {}
            Err(_) => state.endpoints.push(endpoint),
        }
    }
}

fn read_utc_response(
    state: &Runtime,
    req: &[u8],
    handles: &[HandleRef],
) -> RtcHardwareReadUtcResponse {
    if RtcHardwareReadUtcRequest::decode(req, handles).is_err() {
        return RtcHardwareReadUtcResponse {
            status: Status::ErrInvalidArgs,
            utc_timestamp_ns: 0,
        };
    }
    let Some(rtc) = state.rtc() else {
        return RtcHardwareReadUtcResponse {
            status: Status::ErrIo,
            utc_timestamp_ns: 0,
        };
    };
    match rtc.read_utc_ns() {
        Ok(utc_timestamp_ns) => RtcHardwareReadUtcResponse {
            status: Status::Ok,
            utc_timestamp_ns,
        },
        Err(_) => RtcHardwareReadUtcResponse {
            status: Status::ErrInvalidArgs,
            utc_timestamp_ns: 0,
        },
    }
}

fn write_utc_response(
    state: &Runtime,
    req: &[u8],
    handles: &[HandleRef],
) -> RtcHardwareWriteUtcResponse {
    let Ok(request) = RtcHardwareWriteUtcRequest::decode(req, handles) else {
        return RtcHardwareWriteUtcResponse {
            status: Status::ErrInvalidArgs,
        };
    };
    let Some(mut rtc) = state.rtc() else {
        return RtcHardwareWriteUtcResponse {
            status: Status::ErrIo,
        };
    };
    let status = match rtc.write_utc_ns(request.utc_timestamp_ns) {
        Ok(()) => Status::Ok,
        Err(_) => Status::ErrInvalidArgs,
    };
    RtcHardwareWriteUtcResponse { status }
}

fn reply<T: FidlEncode>(channel: Channel, response: &T) {
    let mut out = [0; 64];
    let mut out_handles = [HandleRef { raw: 0 }; 1];
    if let Ok(encoded) = response.encode(&mut out, &mut out_handles) {
        let raw: Vec<_> = out_handles[..encoded.handles]
            .iter()
            .map(|handle| handle.raw)
            .collect();
        let _ = channel.send(&out[..encoded.bytes], &raw);
    }
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}

fn nonzero_channel(raw: u64) -> Option<Channel> {
    (raw != 0).then_some(Channel(raw))
}
