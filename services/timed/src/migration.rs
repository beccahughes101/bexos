extern crate alloc;

use alloc::string::ToString;
use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;
use time_fidl::{ClockSource, Status, SyncState, TimeQuality};

use crate::config::TimedConfig;
use crate::nts::NtsCookieState;
use crate::service::TimedService;

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub service: TimedService,
    pub generation: u64,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>, service: TimedService) -> Self {
        Self {
            control,
            migration,
            service,
            generation: 1,
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            service: TimedService::empty_for_migration(),
            generation: 0,
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
        w.word(4);
        w.word(self.control.0);
        w.word(self.migration.map_or(0, |c| c.0));
        w.word(self.generation);
        w.word(self.service.data.0);
        encode_config(&mut w, &self.service.config);
        encode_quality(&mut w, &self.service.quality);
        w.word(self.service.netstack.map_or(0, |c| c.0));
        w.word(self.service.tls_trust.map_or(0, |c| c.0));
        w.word(self.service.rtc.map_or(0, |c| c.0));
        w.word(self.service.has_clock as u64);
        w.word(self.service.has_set_time as u64);
        w.word(self.service.next_sync_ms);
        w.word(self.service.pending_slew_ns as u64);
        w.word(self.service.slew_started_monotonic_ns);
        w.word(self.service.slew_rate_ppm as i64 as u64);
        encode_nts(&mut w, &self.service.nts_state);
        w.word(self.service.clients.len() as u64);
        for client in &self.service.clients {
            w.word(client.channel.0);
            w.word(client.allowed_methods.len() as u64);
            for ordinal in &client.allowed_methods {
                w.word(*ordinal);
            }
        }
        w.word(self.service.quality_watchers.len() as u64);
        for watcher in &self.service.quality_watchers {
            w.word(watcher.0);
        }
        Ok(Some(w.finish()))
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        if key != 0 {
            return Err(Error::InvalidData);
        }
        let mut r = Decoder::new(bytes.ok_or(Error::InvalidData)?);
        let version = r.word()?;
        if !(1..=4).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        self.control = Channel(r.word()?);
        self.migration = Some(Channel(r.word()?));
        self.generation = r.word()?;
        self.service.data = Channel(r.word()?);
        self.service.config = decode_config(&mut r, version)?;
        self.service.quality = decode_quality(&mut r)?;
        self.service.netstack = nonzero_channel(r.word()?);
        self.service.tls_trust = nonzero_channel(r.word()?);
        if version >= 2 {
            self.service.rtc = nonzero_channel(r.word()?);
        }
        self.service.has_clock = decode_bool(r.word()?)?;
        self.service.has_set_time = decode_bool(r.word()?)?;
        self.service.next_sync_ms = r.word()?;
        if version >= 2 {
            self.service.pending_slew_ns = r.word()? as i64;
            self.service.slew_started_monotonic_ns = r.word()?;
            self.service.slew_rate_ppm = r.word()? as i64 as i32;
        }
        self.service.nts_state = decode_nts(&mut r, version)?;
        self.service.clients.clear();
        for _ in 0..r.count(64)? {
            let channel = Channel(r.word()?);
            let mut allowed_methods = Vec::new();
            for _ in 0..r.count(64)? {
                allowed_methods.push(r.word()?);
            }
            self.service
                .clients
                .push(BoundServiceEndpoint::new(channel, allowed_methods));
        }
        self.service.quality_watchers.clear();
        for _ in 0..r.count(64)? {
            self.service.quality_watchers.push(Channel(r.word()?));
        }
        r.finish()
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() || self.generation == 0 {
            Err(Error::InvalidData)
        } else {
            Ok(())
        }
    }

    fn resources(&self) -> Vec<Resource> {
        let mut out = alloc::vec![Resource::Handle(self.control.0)];
        if let Some(channel) = self.migration {
            out.push(Resource::Handle(channel.0));
        }
        if self.service.data.0 != 0 {
            out.push(Resource::Handle(self.service.data.0));
        }
        if let Some(channel) = self.service.netstack {
            out.push(Resource::Handle(channel.0));
        }
        if let Some(channel) = self.service.tls_trust {
            out.push(Resource::Handle(channel.0));
        }
        if let Some(channel) = self.service.rtc {
            out.push(Resource::Handle(channel.0));
        }
        out.extend(
            self.service
                .clients
                .iter()
                .map(|client| Resource::Handle(client.channel.0)),
        );
        out.extend(
            self.service
                .quality_watchers
                .iter()
                .map(|watcher| Resource::Handle(watcher.0)),
        );
        out
    }

    fn activated(&mut self, generation: u64) {
        self.generation = generation;
        bexos_userspace::log(&alloc::format!("timed: adopted generation={generation}\n"));
    }
}

fn encode_config(w: &mut Encoder, config: &TimedConfig) {
    w.text(&config.primary_server);
    w.word(config.poll_interval_ms as u64);
    w.word(config.initial_retry_ms as u64);
    w.word(config.use_nts as u64);
    w.word(config.slew_limit_ppm as i64 as u64);
    w.word(config.slew_step_threshold_ns as u64);
    w.word(config.network_sync_enabled as u64);
}

fn decode_config(r: &mut Decoder<'_>, version: u64) -> Result<TimedConfig, Error> {
    let primary_server = r.text(128)?.to_string();
    let mut config = TimedConfig {
        primary_server,
        poll_interval_ms: r.word()? as u32,
        initial_retry_ms: r.word()? as u32,
        use_nts: decode_bool(r.word()?)?,
        ..TimedConfig::default()
    };
    if version >= 2 {
        config.slew_limit_ppm = r.word()? as i64 as i32;
        config.slew_step_threshold_ns = r.word()? as i64;
    }
    if version >= 4 {
        config.network_sync_enabled = decode_bool(r.word()?)?;
    }
    Ok(config)
}

fn encode_quality(w: &mut Encoder, quality: &TimeQuality) {
    w.word(quality.source as u64);
    w.word(quality.state as u64);
    w.word(quality.stratum as u64);
    w.word(quality.root_dispersion_ns);
    w.word(quality.last_synced_timestamp_ns);
    w.word(quality.utc_offset_ns as u64);
    w.word(quality.last_error as i32 as u64);
}

fn decode_quality(r: &mut Decoder<'_>) -> Result<TimeQuality, Error> {
    Ok(TimeQuality {
        source: decode_source(r.word()?)?,
        state: decode_state(r.word()?)?,
        stratum: r.word()? as u8,
        root_dispersion_ns: r.word()?,
        last_synced_timestamp_ns: r.word()?,
        utc_offset_ns: r.word()? as i64,
        last_error: decode_status(r.word()? as i32)?,
    })
}

fn encode_nts(w: &mut Encoder, state: &NtsCookieState) {
    w.bytes(&state.c2s_key);
    w.bytes(&state.s2c_key);
    w.bytes(&state.server);
    w.word(state.port as u64);
    w.word(state.cookies.len() as u64);
    for cookie in &state.cookies {
        w.bytes(cookie);
    }
    w.word(state.replay_window.len() as u64);
    for unique in &state.replay_window {
        w.bytes(unique);
    }
}

fn decode_nts(r: &mut Decoder<'_>, version: u64) -> Result<NtsCookieState, Error> {
    let c2s_key = r.bytes(32)?.to_vec();
    let s2c_key = r.bytes(32)?.to_vec();
    let (server, port) = if version >= 3 {
        (r.bytes(255)?.to_vec(), r.count(65535)? as u16)
    } else {
        (Vec::new(), 123)
    };
    let mut cookies = Vec::new();
    for _ in 0..r.count(8)? {
        cookies.push(r.bytes(1024)?.to_vec());
    }
    let mut replay_window = Vec::new();
    if version >= 3 {
        for _ in 0..r.count(16)? {
            replay_window.push(r.bytes(32)?.to_vec());
        }
    }
    Ok(NtsCookieState {
        c2s_key,
        s2c_key,
        cookies,
        server,
        port,
        replay_window,
    })
}

fn nonzero_channel(raw: u64) -> Option<Channel> {
    (raw != 0).then_some(Channel(raw))
}

fn decode_bool(value: u64) -> Result<bool, Error> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::InvalidData),
    }
}

fn decode_source(value: u64) -> Result<ClockSource, Error> {
    match value {
        1 => Ok(ClockSource::RtcHardware),
        2 => Ok(ClockSource::SntpNetwork),
        3 => Ok(ClockSource::NtsSecure),
        4 => Ok(ClockSource::CellularNitz),
        5 => Ok(ClockSource::ManualUser),
        _ => Err(Error::InvalidData),
    }
}

fn decode_state(value: u64) -> Result<SyncState, Error> {
    match value {
        1 => Ok(SyncState::Unsynced),
        2 => Ok(SyncState::Synced),
        3 => Ok(SyncState::Failed),
        4 => Ok(SyncState::Manual),
        _ => Err(Error::InvalidData),
    }
}

fn decode_status(value: i32) -> Result<Status, Error> {
    match value {
        0 => Ok(Status::Ok),
        -6 => Ok(Status::ErrTimedOut),
        -8 => Ok(Status::ErrInvalidArgs),
        -10 => Ok(Status::ErrNotFound),
        -12 => Ok(Status::ErrNetworkUnreachable),
        -13 => Ok(Status::ErrUnsupported),
        -14 => Ok(Status::ErrIo),
        _ => Err(Error::InvalidData),
    }
}
