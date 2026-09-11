//! State shared by services whose post-initialization loop is a control endpoint.
use crate::{
    Channel,
    live_migration::{Resource, Source, State},
    service_binding::{BoundServiceEndpoint, ServiceBinding},
    yield_now,
};
use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use power_fidl::{
    DevicePowerControlSetPowerStateRequest, DevicePowerControlSetPowerStateResponse,
    DevicePowerState, FidlDecode, FidlEncode, HandleRef, Status,
};

pub struct ControlState {
    pub manager: Channel,
    pub migration: Option<Channel>,
    pub mapping: Option<(u64, u64, u64)>,
    pub requests: u64,
}
impl State for ControlState {
    fn empty() -> Self {
        Self {
            manager: Channel(0),
            migration: None,
            mapping: None,
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
        let channel = r.word()?;
        self.migration = (channel != 0).then_some(Channel(channel));
        self.requests = r.word()?;
        self.mapping = if r.flag()? {
            Some((r.word()?, r.word()?, r.word()?))
        } else {
            None
        };
        r.finish()
    }
    fn validate(&self) -> Result<(), Error> {
        if self.manager.0 == 0 || self.migration.is_none() {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }
    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.manager.0)];
        if let Some(c) = self.migration {
            out.push(Resource::Handle(c.0));
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
        out
    }
    fn activated(&mut self, generation: u64) {
        crate::log(&alloc::format!(
            "service-control: adopted generation={generation}; initialization skipped\n"
        ));
    }
}
pub fn serve(mut state: ControlState) -> ! {
    let mut source = Source::new(state.migration);
    let mut power_endpoints = Vec::new();
    loop {
        if source.poll(&state).is_err() {
            let _ = crate::migration::abort();
        }
        if source.quiescing() {
            yield_now();
            continue;
        }
        if let Ok(m) = state.manager.try_recv() {
            let replied = if let (Some(endpoint), Ok(metadata)) =
                (m.handles.first().copied(), core::str::from_utf8(&m.bytes))
            {
                if let Some(binding) = ServiceBinding::parse(metadata) {
                    if binding.protocol_is("DevicePowerControl") && binding.allows(1) {
                        power_endpoints.push(BoundServiceEndpoint::new(
                            Channel(endpoint),
                            binding.method_ordinals,
                        ));
                        true
                    } else {
                        false
                    }
                } else if metadata.split('|').nth(1) == Some("DevicePowerControl") {
                    let _ = crate::Memory::close(endpoint);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !replied && state.manager.send(&m.bytes, &m.handles).is_err() {
                for h in m.handles {
                    let _ = crate::Memory::close(h);
                }
            }
            state.requests = state.requests.wrapping_add(1);
            source.changed(0);
        }
        poll_device_power_endpoints(&mut power_endpoints);
        yield_now();
    }
}

fn poll_device_power_endpoints(endpoints: &mut Vec<BoundServiceEndpoint>) {
    endpoints.retain(|endpoint| match serve_device_power_endpoint(endpoint) {
        Ok(()) | Err(kernel_fidl::Status::ErrTimedOut) => true,
        Err(kernel_fidl::Status::ErrPeerClosed) => false,
        Err(_) => true,
    });
}

fn serve_device_power_endpoint(endpoint: &BoundServiceEndpoint) -> Result<(), kernel_fidl::Status> {
    let channel = endpoint.channel;
    if let Ok(message) = channel.try_recv() {
        let (ordinal, req) = envelope(&message.bytes);
        let handles: Vec<_> = message
            .handles
            .iter()
            .map(|raw| HandleRef { raw: *raw })
            .collect();
        let status = if endpoint.allows(ordinal) && ordinal == 1 {
            match DevicePowerControlSetPowerStateRequest::decode(req, &handles) {
                Ok(request)
                    if matches!(
                        request.state,
                        DevicePowerState::D0FullPower | DevicePowerState::D3Off
                    ) =>
                {
                    Status::Ok
                }
                Ok(_) => Status::ErrInvalidArgs,
                Err(_) => Status::ErrInvalidArgs,
            }
        } else {
            Status::ErrInvalidArgs
        };
        let mut out = [0; 16];
        let mut out_handles = [HandleRef { raw: 0 }; 1];
        if let Ok(encoded) =
            (DevicePowerControlSetPowerStateResponse { status }).encode(&mut out, &mut out_handles)
        {
            let raw: Vec<_> = out_handles[..encoded.handles]
                .iter()
                .map(|handle| handle.raw)
                .collect();
            let _ = channel.send(&out[..encoded.bytes], &raw);
        }
    }
    Ok(())
}

fn envelope(bytes: &[u8]) -> (u64, &[u8]) {
    if bytes.len() < 8 {
        return (0, &[]);
    }
    let mut raw = [0; 8];
    raw.copy_from_slice(&bytes[..8]);
    (u64::from_le_bytes(raw), &bytes[8..])
}
