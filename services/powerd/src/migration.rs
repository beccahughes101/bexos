use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::{Resource, State};
use bexos_userspace::service_binding::BoundServiceEndpoint;
use power_fidl::{PerformanceLevel, SystemPowerState};

use crate::{PowerPolicy, PowerPolicyConfig, TelemetrySample, WakeLease};

pub struct Runtime {
    pub control: Channel,
    pub migration: Option<Channel>,
    pub clients: Vec<BoundServiceEndpoint>,
    pub watchers: Vec<u64>,
    pub policy: PowerPolicy,
    pub next_telemetry_poll_ms: u64,
}

impl Runtime {
    pub fn new(control: Channel, migration: Option<Channel>, policy: PowerPolicy) -> Self {
        Self {
            control,
            migration,
            clients: Vec::new(),
            watchers: Vec::new(),
            policy,
            next_telemetry_poll_ms: 0,
        }
    }
}

impl State for Runtime {
    fn empty() -> Self {
        Self {
            control: Channel(0),
            migration: None,
            clients: Vec::new(),
            watchers: Vec::new(),
            policy: PowerPolicy::new(),
            next_telemetry_poll_ms: 0,
        }
    }

    fn keys(&self) -> Vec<u64> {
        (0..7).collect()
    }

    fn encode_record(&self, key: u64) -> Result<Option<Vec<u8>>, Error> {
        match key {
            0 => {
                let mut w = Encoder::new();
                w.word(1);
                w.word(self.control.0);
                w.word(self.migration.map(|c| c.0).unwrap_or(0));
                w.word(self.next_telemetry_poll_ms);
                Ok(Some(w.finish()))
            }
            1 => {
                let mut w = Encoder::new();
                w.word(2);
                w.word(self.clients.len() as u64);
                for client in &self.clients {
                    w.word(client.channel.0);
                    w.word(client.allowed_methods.len() as u64);
                    for ordinal in &client.allowed_methods {
                        w.word(*ordinal);
                    }
                }
                Ok(Some(w.finish()))
            }
            2 => {
                let mut w = Encoder::new();
                w.word(self.policy.device_endpoints().len() as u64);
                for endpoint in self.policy.device_endpoints() {
                    w.word(*endpoint);
                }
                Ok(Some(w.finish()))
            }
            3 => {
                let mut w = Encoder::new();
                w.word(self.policy.transitions.len() as u64);
                for transition in &self.policy.transitions {
                    w.word(system_state_to_wire(*transition));
                }
                Ok(Some(w.finish()))
            }
            4 => {
                let mut w = Encoder::new();
                w.word(self.policy.leases().len() as u64);
                for lease in self.policy.leases() {
                    w.word(lease.handle);
                    w.bytes(lease.reason.as_bytes());
                }
                Ok(Some(w.finish()))
            }
            5 => {
                let mut w = Encoder::new();
                w.word(self.watchers.len() as u64);
                for watcher in &self.watchers {
                    w.word(*watcher);
                }
                Ok(Some(w.finish()))
            }
            6 => {
                let mut w = Encoder::new();
                w.word(1);
                w.word(self.policy.telemetry_provider.unwrap_or(0));
                w.word(self.policy.performance_provider.unwrap_or(0));
                encode_telemetry(&mut w, self.policy.telemetry);
                w.word(self.policy.telemetry_stale as u64);
                w.word(self.policy.thermal_throttled as u64);
                w.word(self.policy.critical_throttled as u64);
                w.word(performance_level_to_wire(
                    self.policy.applied_performance_level,
                ));
                w.word(
                    self.policy
                        .baseline_background_cpu_cap_permille
                        .unwrap_or(u16::MAX) as u64,
                );
                encode_config(&mut w, self.policy.config());
                Ok(Some(w.finish()))
            }
            _ => Ok(None),
        }
    }

    fn adopt_record(&mut self, key: u64, bytes: Option<&[u8]>) -> Result<(), Error> {
        let bytes = bytes.ok_or(Error::InvalidData)?;
        match key {
            0 => {
                let mut r = Decoder::new(bytes);
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                self.control = Channel(r.word()?);
                let migration = r.word()?;
                self.migration = (migration != 0).then_some(Channel(migration));
                self.next_telemetry_poll_ms = r.word().unwrap_or(0);
                r.finish()?;
            }
            1 => {
                let mut r = Decoder::new(bytes);
                if r.word()? != 2 {
                    return Err(Error::UnsupportedVersion);
                }
                let count = r.word()?;
                self.clients.clear();
                for _ in 0..count {
                    let channel = Channel(r.word()?);
                    let mut allowed_methods = Vec::new();
                    for _ in 0..r.count(64)? {
                        allowed_methods.push(r.word()?);
                    }
                    self.clients
                        .push(BoundServiceEndpoint::new(channel, allowed_methods));
                }
                r.finish()?;
            }
            2 => {
                let mut r = Decoder::new(bytes);
                let count = r.word()?;
                let mut devices = Vec::new();
                for _ in 0..count {
                    devices.push(r.word()?);
                }
                r.finish()?;
                self.policy = PowerPolicy::from_parts(
                    self.policy.leases().to_vec(),
                    devices,
                    self.policy.transitions.clone(),
                );
            }
            3 => {
                let mut r = Decoder::new(bytes);
                let count = r.word()?;
                let mut transitions = Vec::new();
                for _ in 0..count {
                    transitions.push(system_state_from_wire(r.word()?)?);
                }
                r.finish()?;
                self.policy = PowerPolicy::from_parts(
                    self.policy.leases().to_vec(),
                    self.policy.device_endpoints().to_vec(),
                    transitions,
                );
            }
            4 => {
                let mut r = Decoder::new(bytes);
                let count = r.word()?;
                let mut leases = Vec::new();
                for _ in 0..count {
                    leases.push(decode_lease(&mut r)?);
                }
                r.finish()?;
                self.policy = PowerPolicy::from_parts(
                    leases,
                    self.policy.device_endpoints().to_vec(),
                    self.policy.transitions.clone(),
                );
            }
            5 => {
                let mut r = Decoder::new(bytes);
                self.watchers.clear();
                for _ in 0..r.count(64)? {
                    self.watchers.push(r.word()?);
                }
                r.finish()?;
            }
            6 => {
                let mut r = Decoder::new(bytes);
                if r.word()? != 1 {
                    return Err(Error::UnsupportedVersion);
                }
                let telemetry_provider = nonzero(r.word()?);
                let performance_provider = nonzero(r.word()?);
                let telemetry = decode_telemetry(&mut r)?;
                let telemetry_stale = decode_bool(r.word()?)?;
                let thermal_throttled = decode_bool(r.word()?)?;
                let critical_throttled = decode_bool(r.word()?)?;
                let applied = performance_level_from_wire(r.word()?)?;
                let raw_baseline = r.word()?;
                let baseline = (raw_baseline != u16::MAX as u64).then_some(raw_baseline as u16);
                let config = decode_config(&mut r)?;
                r.finish()?;
                self.policy = PowerPolicy::from_parts_full(
                    self.policy.leases().to_vec(),
                    self.policy.device_endpoints().to_vec(),
                    self.policy.transitions.clone(),
                    telemetry_provider,
                    performance_provider,
                    telemetry,
                    telemetry_stale,
                    thermal_throttled,
                    critical_throttled,
                    applied,
                    baseline,
                    config,
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        if self.control.0 == 0 || self.migration.is_none() {
            return Err(Error::InvalidData);
        }
        if self.clients.iter().any(|client| client.channel.0 == 0)
            || self
                .policy
                .device_endpoints()
                .iter()
                .any(|endpoint| *endpoint == 0)
            || self.policy.leases().iter().any(|lease| lease.handle == 0)
        {
            return Err(Error::InvalidData);
        }
        Ok(())
    }

    fn resources(&self) -> Vec<Resource> {
        let mut handles = vec![self.control.0];
        if let Some(migration) = self.migration {
            handles.push(migration.0);
        }
        handles.extend(self.clients.iter().map(|client| client.channel.0));
        handles.extend(self.watchers.iter().copied());
        handles.extend(self.policy.device_endpoints());
        if let Some(endpoint) = self.policy.telemetry_provider {
            handles.push(endpoint);
        }
        if let Some(endpoint) = self.policy.performance_provider {
            handles.push(endpoint);
        }
        handles.extend(self.policy.leases().iter().map(|lease| lease.handle));
        handles.into_iter().map(Resource::Handle).collect()
    }

    fn activated(&mut self, _generation: u64) {}
}

fn decode_lease(r: &mut Decoder<'_>) -> Result<WakeLease, Error> {
    let handle = r.word()?;
    let reason = core::str::from_utf8(r.bytes(64)?)
        .map_err(|_| Error::InvalidData)?
        .to_string();
    Ok(WakeLease { handle, reason })
}

fn encode_telemetry(w: &mut Encoder, telemetry: TelemetrySample) {
    w.word(telemetry.battery_available as u64);
    w.word(telemetry.charging as u64);
    w.word(telemetry.battery_percent as u64);
    w.word(telemetry.thermal_available as u64);
    w.word(telemetry.temperature_celsius as i64 as u64);
}

fn decode_telemetry(r: &mut Decoder<'_>) -> Result<TelemetrySample, Error> {
    Ok(TelemetrySample {
        battery_available: decode_bool(r.word()?)?,
        charging: decode_bool(r.word()?)?,
        battery_percent: r.word()? as u8,
        thermal_available: decode_bool(r.word()?)?,
        temperature_celsius: r.word()? as i64 as i32,
    })
}

fn encode_config(w: &mut Encoder, config: PowerPolicyConfig) {
    w.word(config.low_battery_percent as u64);
    w.word(config.thermal_throttle_celsius as i64 as u64);
    w.word(config.thermal_critical_celsius as i64 as u64);
    w.word(config.thermal_recovery_hysteresis_celsius as i64 as u64);
}

fn decode_config(r: &mut Decoder<'_>) -> Result<PowerPolicyConfig, Error> {
    Ok(PowerPolicyConfig {
        low_battery_percent: r.word()? as u8,
        thermal_throttle_celsius: r.word()? as i64 as i32,
        thermal_critical_celsius: r.word()? as i64 as i32,
        thermal_recovery_hysteresis_celsius: r.word()? as i64 as i32,
    })
}

const fn performance_level_to_wire(level: PerformanceLevel) -> u64 {
    match level {
        PerformanceLevel::Nominal => 1,
        PerformanceLevel::Reduced => 2,
        PerformanceLevel::Minimum => 3,
    }
}

const fn performance_level_from_wire(raw: u64) -> Result<PerformanceLevel, Error> {
    match raw {
        1 => Ok(PerformanceLevel::Nominal),
        2 => Ok(PerformanceLevel::Reduced),
        3 => Ok(PerformanceLevel::Minimum),
        _ => Err(Error::InvalidData),
    }
}

fn decode_bool(value: u64) -> Result<bool, Error> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::InvalidData),
    }
}

fn nonzero(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}

const fn system_state_to_wire(state: SystemPowerState) -> u64 {
    match state {
        SystemPowerState::Active => 0,
        SystemPowerState::SuspendToRam => 1,
        SystemPowerState::SuspendToDisk => 2,
        SystemPowerState::Reboot => 3,
        SystemPowerState::Poweroff => 4,
    }
}

const fn system_state_from_wire(raw: u64) -> Result<SystemPowerState, Error> {
    match raw {
        0 => Ok(SystemPowerState::Active),
        1 => Ok(SystemPowerState::SuspendToRam),
        2 => Ok(SystemPowerState::SuspendToDisk),
        3 => Ok(SystemPowerState::Reboot),
        4 => Ok(SystemPowerState::Poweroff),
        _ => Err(Error::InvalidData),
    }
}
