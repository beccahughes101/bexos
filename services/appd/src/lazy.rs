use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use bexos_kernel_core::ipc::Capability;
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

use crate::broker::BoundCapability;
use crate::manifest::{Lifecycle, ServiceActivation};

pub const MAX_PENDING_BINDS_PER_PROVIDER: usize = 64;
pub const STARTUP_TIMEOUT_MS: u32 = 5_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LazyProviderPhase {
    Dormant,
    Starting,
    Running,
    Stopping,
}

#[derive(Clone, Debug)]
pub struct LazyProvider {
    pub package: String,
    pub process: String,
    pub uid: u64,
    pub phase: LazyProviderPhase,
    pub generation: u64,
    pub idle_timeout_ms: u32,
    pub pending: Vec<BoundCapability>,
    pub intentional_stop: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LazyActivationState {
    providers: Vec<LazyProvider>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LazyActivationError {
    NotLazy,
    MissingProviderProcess,
    QueueFull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LazyDemand {
    LaunchRequired,
    AlreadyRunning,
    QueuedWhileStarting,
    QueuedWhileStopping,
}

impl LazyActivationState {
    pub fn providers(&self) -> &[LazyProvider] {
        &self.providers
    }

    pub fn handles(&self) -> Vec<u64> {
        let mut handles = Vec::new();
        for provider in &self.providers {
            for binding in &provider.pending {
                handles.extend([
                    binding.provider_manager.object_id,
                    binding.client_endpoint.object_id,
                    binding.provider_endpoint.object_id,
                ]);
            }
        }
        handles
    }

    pub fn declare_manifest(&mut self, manifest: &crate::Manifest, uid: u64) {
        for service in &manifest.services_exposed {
            if service.activation != ServiceActivation::Lazy {
                continue;
            }
            let Ok(process) = manifest.service_provider_process(service) else {
                continue;
            };
            let idle_timeout_ms = manifest.service_idle_timeout_ms(service).unwrap_or(0);
            self.declare(
                manifest.package_name.clone(),
                process.name.clone(),
                uid,
                idle_timeout_ms,
            );
        }
    }

    pub fn declare(&mut self, package: String, process: String, uid: u64, idle_timeout_ms: u32) {
        if let Some(provider) = self.provider_mut(&package, &process, uid) {
            provider.idle_timeout_ms = idle_timeout_ms;
            return;
        }
        self.providers.push(LazyProvider {
            package,
            process,
            uid,
            phase: LazyProviderPhase::Dormant,
            generation: 0,
            idle_timeout_ms,
            pending: Vec::new(),
            intentional_stop: false,
        });
    }

    pub fn demand(
        &mut self,
        binding: BoundCapability,
        uid: u64,
    ) -> Result<LazyDemand, LazyActivationError> {
        if binding.activation != ServiceActivation::Lazy {
            return Ok(LazyDemand::AlreadyRunning);
        }
        let process = binding
            .provider_process
            .clone()
            .ok_or(LazyActivationError::MissingProviderProcess)?;
        let provider = self.ensure_provider(
            binding.provider_package.clone(),
            process,
            uid,
            binding.idle_timeout_ms,
        );
        if provider.pending.len() >= MAX_PENDING_BINDS_PER_PROVIDER {
            return Err(LazyActivationError::QueueFull);
        }
        let phase = provider.phase;
        provider.pending.push(binding);
        Ok(match phase {
            LazyProviderPhase::Dormant => {
                provider.phase = LazyProviderPhase::Starting;
                LazyDemand::LaunchRequired
            }
            LazyProviderPhase::Starting => LazyDemand::QueuedWhileStarting,
            LazyProviderPhase::Running => LazyDemand::AlreadyRunning,
            LazyProviderPhase::Stopping => LazyDemand::QueuedWhileStopping,
        })
    }

    pub fn running(&mut self, package: &str, process: &str, uid: u64) -> Vec<BoundCapability> {
        if let Some(provider) = self.provider_mut(package, process, uid) {
            provider.phase = LazyProviderPhase::Running;
            provider.intentional_stop = false;
            return core::mem::take(&mut provider.pending);
        }
        Vec::new()
    }

    pub fn take_starting_pending(
        &mut self,
        package: &str,
        process: &str,
        uid: u64,
    ) -> Vec<BoundCapability> {
        if let Some(provider) = self.provider_mut(package, process, uid) {
            if provider.phase == LazyProviderPhase::Starting {
                return core::mem::take(&mut provider.pending);
            }
        }
        Vec::new()
    }

    pub fn failed_launch(
        &mut self,
        package: &str,
        process: &str,
        uid: u64,
    ) -> Vec<BoundCapability> {
        if let Some(provider) = self.provider_mut(package, process, uid) {
            provider.phase = LazyProviderPhase::Dormant;
            return core::mem::take(&mut provider.pending);
        }
        Vec::new()
    }

    pub fn begin_idle_stop(
        &mut self,
        package: &str,
        process: &str,
        uid: u64,
        generation: u64,
    ) -> bool {
        let Some(provider) = self.provider_mut(package, process, uid) else {
            return false;
        };
        if provider.phase != LazyProviderPhase::Running || provider.generation != generation {
            return false;
        }
        provider.phase = LazyProviderPhase::Stopping;
        provider.intentional_stop = true;
        true
    }

    pub fn stopped(&mut self, package: &str, process: &str, uid: u64) -> bool {
        let Some(provider) = self.provider_mut(package, process, uid) else {
            return false;
        };
        let had_pending = !provider.pending.is_empty();
        provider.phase = if had_pending {
            LazyProviderPhase::Starting
        } else {
            LazyProviderPhase::Dormant
        };
        provider.intentional_stop = false;
        provider.generation = provider.generation.saturating_add(1);
        had_pending
    }

    pub fn is_intentional_stop(&self, package: &str, process: &str, uid: u64) -> bool {
        self.provider(package, process, uid)
            .is_some_and(|provider| provider.intentional_stop)
    }

    pub fn has_pending_demand(&self, package: &str, process: &str, uid: u64) -> bool {
        self.provider(package, process, uid)
            .is_some_and(|provider| !provider.pending.is_empty())
    }

    pub fn generation(&self, package: &str, process: &str, uid: u64) -> u64 {
        self.provider(package, process, uid)
            .map_or(0, |provider| provider.generation)
    }

    pub fn remove_package(&mut self, package: &str) -> Vec<BoundCapability> {
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.providers.len() {
            if self.providers[index].package == package {
                let provider = self.providers.remove(index);
                removed.extend(provider.pending);
            } else {
                index += 1;
            }
        }
        removed
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(1);
        w.word(self.providers.len() as u64);
        for provider in &self.providers {
            w.text(&provider.package);
            w.text(&provider.process);
            w.word(provider.uid);
            w.word(phase_to_u64(provider.phase));
            w.word(provider.generation);
            w.word(provider.idle_timeout_ms as u64);
            w.word(provider.intentional_stop as u64);
            w.word(provider.pending.len() as u64);
            for binding in &provider.pending {
                encode_binding(&mut w, binding);
            }
        }
        w.finish()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        if r.word()? != 1 {
            return Err(Error::UnsupportedVersion);
        }
        let mut providers = Vec::new();
        for _ in 0..r.count(64)? {
            let package = r.text(128)?.to_string();
            let process = r.text(64)?.to_string();
            let uid = r.word()?;
            let phase = phase_from_u64(r.word()?)?;
            let generation = r.word()?;
            let idle_timeout_ms = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
            let intentional_stop = r.flag()?;
            let mut pending = Vec::new();
            for _ in 0..r.count(MAX_PENDING_BINDS_PER_PROVIDER)? {
                pending.push(decode_binding(&mut r)?);
            }
            providers.push(LazyProvider {
                package,
                process,
                uid,
                phase,
                generation,
                idle_timeout_ms,
                pending,
                intentional_stop,
            });
        }
        r.finish()?;
        Ok(Self { providers })
    }

    fn ensure_provider(
        &mut self,
        package: String,
        process: String,
        uid: u64,
        idle_timeout_ms: u32,
    ) -> &mut LazyProvider {
        if self.provider(&package, &process, uid).is_none() {
            self.declare(package.clone(), process.clone(), uid, idle_timeout_ms);
        }
        self.provider_mut(&package, &process, uid).unwrap()
    }

    fn provider(&self, package: &str, process: &str, uid: u64) -> Option<&LazyProvider> {
        self.providers.iter().find(|provider| {
            provider.package == package && provider.process == process && provider.uid == uid
        })
    }

    fn provider_mut(
        &mut self,
        package: &str,
        process: &str,
        uid: u64,
    ) -> Option<&mut LazyProvider> {
        self.providers.iter_mut().find(|provider| {
            provider.package == package && provider.process == process && provider.uid == uid
        })
    }
}

fn encode_binding(w: &mut Encoder, binding: &BoundCapability) {
    w.text(&binding.caller_package);
    match binding.caller_uid {
        Some(uid) => {
            w.word(1);
            w.word(uid);
        }
        None => {
            w.word(0);
            w.word(0);
        }
    }
    w.word(binding.caller_foreground as u64);
    w.text(&binding.provider_package);
    match &binding.provider_instance_id {
        Some(instance_id) => {
            w.word(1);
            w.text(instance_id);
        }
        None => w.word(0),
    }
    w.text(&binding.service_name);
    w.text(&binding.protocol);
    w.word(match binding.lifecycle {
        Lifecycle::Unspecified => 0,
        Lifecycle::Singleton => 1,
        Lifecycle::UserScopedSingleton => 2,
        Lifecycle::MultipleInstance => 3,
    });
    w.text(&binding.capability);
    match &binding.permission {
        Some(permission) => {
            w.word(1);
            w.text(permission);
        }
        None => w.word(0),
    }
    w.word(binding.method_ordinals.len() as u64);
    for ordinal in &binding.method_ordinals {
        w.word(*ordinal);
    }
    w.word(binding.permission_values.len() as u64);
    for value in &binding.permission_values {
        w.text(value);
    }
    encode_capability(w, binding.provider_manager);
    encode_capability(w, binding.client_endpoint);
    encode_capability(w, binding.provider_endpoint);
    match &binding.provider_process {
        Some(process) => {
            w.word(1);
            w.text(process);
        }
        None => w.word(0),
    }
    w.word(binding.idle_timeout_ms as u64);
}

fn decode_binding(r: &mut Decoder<'_>) -> Result<BoundCapability, Error> {
    let caller_package = r.text(128)?.to_string();
    let caller_uid = if r.flag()? {
        Some(r.word()?)
    } else {
        let _ = r.word()?;
        None
    };
    let caller_foreground = r.flag()?;
    let provider_package = r.text(128)?.to_string();
    let provider_instance_id = if r.flag()? {
        Some(r.text(128)?.to_string())
    } else {
        None
    };
    let service_name = r.text(128)?.to_string();
    let protocol = r.text(128)?.to_string();
    let lifecycle = match r.word()? {
        1 => Lifecycle::Singleton,
        2 => Lifecycle::UserScopedSingleton,
        3 => Lifecycle::MultipleInstance,
        _ => Lifecycle::Unspecified,
    };
    let capability = r.text(64)?.to_string();
    let permission = if r.flag()? {
        Some(r.text(128)?.to_string())
    } else {
        None
    };
    let mut method_ordinals = Vec::new();
    for _ in 0..r.count(64)? {
        method_ordinals.push(r.word()?);
    }
    let mut permission_values = Vec::new();
    for _ in 0..r.count(64)? {
        permission_values.push(r.text(128)?.to_string());
    }
    let provider_manager = decode_capability(r)?;
    let client_endpoint = decode_capability(r)?;
    let provider_endpoint = decode_capability(r)?;
    let provider_process = if r.flag()? {
        Some(r.text(64)?.to_string())
    } else {
        None
    };
    let idle_timeout_ms = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
    Ok(BoundCapability {
        caller_package,
        caller_uid,
        caller_foreground,
        provider_package,
        provider_instance_id,
        service_name,
        protocol,
        lifecycle,
        capability,
        permission,
        method_ordinals,
        permission_values,
        provider_manager,
        client_endpoint,
        provider_endpoint,
        activation: ServiceActivation::Lazy,
        provider_process,
        idle_timeout_ms,
        dormant_provider: true,
    })
}

fn encode_capability(w: &mut Encoder, capability: Capability) {
    w.word(capability.object_id);
    w.word(capability.rights as u64);
}

fn decode_capability(r: &mut Decoder<'_>) -> Result<Capability, Error> {
    Ok(Capability {
        object_id: r.word()?,
        rights: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
    })
}

fn phase_to_u64(phase: LazyProviderPhase) -> u64 {
    match phase {
        LazyProviderPhase::Dormant => 0,
        LazyProviderPhase::Starting => 1,
        LazyProviderPhase::Running => 2,
        LazyProviderPhase::Stopping => 3,
    }
}

fn phase_from_u64(value: u64) -> Result<LazyProviderPhase, Error> {
    match value {
        0 => Ok(LazyProviderPhase::Dormant),
        1 => Ok(LazyProviderPhase::Starting),
        2 => Ok(LazyProviderPhase::Running),
        3 => Ok(LazyProviderPhase::Stopping),
        _ => Err(Error::InvalidData),
    }
}
