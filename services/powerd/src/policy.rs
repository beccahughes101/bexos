use alloc::string::{String, ToString};
use alloc::vec::Vec;
use power_fidl::{PerformanceLevel, PowerSnapshot, Status, SystemPowerState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerPolicyConfig {
    pub low_battery_percent: u8,
    pub thermal_throttle_celsius: i32,
    pub thermal_critical_celsius: i32,
    pub thermal_recovery_hysteresis_celsius: i32,
}

impl Default for PowerPolicyConfig {
    fn default() -> Self {
        Self {
            low_battery_percent: 15,
            thermal_throttle_celsius: 85,
            thermal_critical_celsius: 95,
            thermal_recovery_hysteresis_celsius: 5,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TelemetrySample {
    pub battery_available: bool,
    pub charging: bool,
    pub battery_percent: u8,
    pub thermal_available: bool,
    pub temperature_celsius: i32,
}

impl TelemetrySample {
    pub const fn unavailable() -> Self {
        Self {
            battery_available: false,
            charging: false,
            battery_percent: 0,
            thermal_available: false,
            temperature_celsius: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppliedPolicy {
    pub snapshot: PowerSnapshot,
    pub background_cpu_cap_permille: u16,
    pub performance_level_changed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WakeLease {
    pub handle: u64,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PowerPolicy {
    leases: Vec<WakeLease>,
    devices: Vec<u64>,
    pub telemetry_provider: Option<u64>,
    pub performance_provider: Option<u64>,
    pub telemetry: TelemetrySample,
    pub telemetry_stale: bool,
    pub thermal_throttled: bool,
    pub critical_throttled: bool,
    pub applied_performance_level: PerformanceLevel,
    pub baseline_background_cpu_cap_permille: Option<u16>,
    pub transitions: Vec<SystemPowerState>,
    config: PowerPolicyConfig,
}

impl Default for PowerPolicy {
    fn default() -> Self {
        Self {
            leases: Vec::new(),
            devices: Vec::new(),
            telemetry_provider: None,
            performance_provider: None,
            telemetry: TelemetrySample::unavailable(),
            telemetry_stale: false,
            thermal_throttled: false,
            critical_throttled: false,
            applied_performance_level: PerformanceLevel::Nominal,
            baseline_background_cpu_cap_permille: None,
            transitions: Vec::new(),
            config: PowerPolicyConfig::default(),
        }
    }
}

impl PowerPolicy {
    pub fn new() -> Self {
        Self {
            telemetry: TelemetrySample::unavailable(),
            applied_performance_level: PerformanceLevel::Nominal,
            config: PowerPolicyConfig::default(),
            ..Self::default()
        }
    }

    pub fn from_parts(
        leases: Vec<WakeLease>,
        devices: Vec<u64>,
        transitions: Vec<SystemPowerState>,
    ) -> Self {
        Self {
            leases,
            devices,
            transitions,
            ..Self::new()
        }
    }

    pub fn from_parts_full(
        leases: Vec<WakeLease>,
        devices: Vec<u64>,
        transitions: Vec<SystemPowerState>,
        telemetry_provider: Option<u64>,
        performance_provider: Option<u64>,
        telemetry: TelemetrySample,
        telemetry_stale: bool,
        thermal_throttled: bool,
        critical_throttled: bool,
        applied_performance_level: PerformanceLevel,
        baseline_background_cpu_cap_permille: Option<u16>,
        config: PowerPolicyConfig,
    ) -> Self {
        Self {
            leases,
            devices,
            telemetry_provider,
            performance_provider,
            telemetry,
            telemetry_stale,
            thermal_throttled,
            critical_throttled,
            applied_performance_level,
            baseline_background_cpu_cap_permille,
            transitions,
            config,
        }
    }

    pub fn register_device(&mut self, endpoint: u64) -> Status {
        if endpoint == 0 {
            return Status::ErrInvalidHandle;
        }
        if self.devices.contains(&endpoint) {
            return Status::Ok;
        }
        if self.devices.try_reserve(1).is_err() {
            return Status::ErrNoMemory;
        }
        self.devices.push(endpoint);
        Status::Ok
    }

    pub fn register_telemetry_provider(&mut self, endpoint: u64) -> Status {
        if endpoint == 0 {
            return Status::ErrInvalidHandle;
        }
        self.telemetry_provider = Some(endpoint);
        Status::Ok
    }

    pub fn register_performance_provider(&mut self, endpoint: u64) -> Status {
        if endpoint == 0 {
            return Status::ErrInvalidHandle;
        }
        self.performance_provider = Some(endpoint);
        Status::Ok
    }

    pub fn add_lease(&mut self, handle: u64, reason: &str) -> Status {
        if handle == 0 || reason.is_empty() || reason.len() > 64 {
            return Status::ErrInvalidArgs;
        }
        if self.leases.try_reserve(1).is_err() {
            return Status::ErrNoMemory;
        }
        self.leases.push(WakeLease {
            handle,
            reason: reason.to_string(),
        });
        Status::Ok
    }

    pub fn remove_lease(&mut self, handle: u64) -> bool {
        let before = self.leases.len();
        self.leases.retain(|lease| lease.handle != handle);
        self.leases.len() != before
    }

    pub fn active_lease_count(&self) -> usize {
        self.leases.len()
    }

    pub fn leases(&self) -> &[WakeLease] {
        &self.leases
    }

    pub fn device_endpoints(&self) -> &[u64] {
        &self.devices
    }

    pub fn config(&self) -> PowerPolicyConfig {
        self.config
    }

    pub fn set_config(&mut self, config: PowerPolicyConfig) {
        self.config = config;
    }

    pub fn request_system_state(&mut self, state: SystemPowerState) -> Status {
        match state {
            SystemPowerState::Active => Status::Ok,
            SystemPowerState::SuspendToDisk => Status::ErrUnsupported,
            SystemPowerState::SuspendToRam if !self.leases.is_empty() => Status::ErrAccessDenied,
            SystemPowerState::SuspendToRam
            | SystemPowerState::Reboot
            | SystemPowerState::Poweroff => {
                if self.transitions.try_reserve(1).is_err() {
                    return Status::ErrNoMemory;
                }
                self.transitions.push(state);
                Status::Ok
            }
        }
    }

    pub fn apply_telemetry_result(
        &mut self,
        sample: Result<TelemetrySample, Status>,
    ) -> AppliedPolicy {
        match sample {
            Ok(sample) => {
                self.telemetry = sample;
                self.telemetry_stale = false;
                self.update_temperature_latches(sample);
            }
            Err(_) if self.telemetry_provider.is_some() => {
                self.telemetry_stale = true;
            }
            Err(_) => {
                self.telemetry = TelemetrySample::unavailable();
                self.telemetry_stale = false;
                self.thermal_throttled = false;
                self.critical_throttled = false;
            }
        }
        let desired = self.desired_performance_level();
        let changed = desired != self.applied_performance_level;
        self.applied_performance_level = desired;
        AppliedPolicy {
            snapshot: self.snapshot(),
            background_cpu_cap_permille: self.background_cpu_cap_permille(),
            performance_level_changed: changed,
        }
    }

    pub fn snapshot(&self) -> PowerSnapshot {
        let battery_low = self.telemetry.battery_available
            && !self.telemetry.charging
            && self.telemetry.battery_percent <= self.config.low_battery_percent;
        PowerSnapshot {
            charging: self.telemetry.battery_available && self.telemetry.charging,
            device_idle: self.active_lease_count() == 0,
            battery_low,
            thermal_throttled: self.thermal_throttled || self.critical_throttled,
            active_wake_leases: self.active_lease_count() as u32,
            battery_available: self.telemetry.battery_available,
            thermal_available: self.telemetry.thermal_available,
            battery_percent: if self.telemetry.battery_available {
                self.telemetry.battery_percent
            } else {
                0
            },
            temperature_celsius: if self.telemetry.thermal_available {
                self.telemetry.temperature_celsius
            } else {
                0
            },
            telemetry_stale: self.telemetry_stale,
            dvfs_available: self.performance_provider.is_some(),
            applied_performance_level: self.applied_performance_level,
        }
    }

    pub fn background_cpu_cap_permille(&self) -> u16 {
        match self.applied_performance_level {
            PerformanceLevel::Nominal => self.baseline_background_cpu_cap_permille.unwrap_or(0),
            PerformanceLevel::Reduced => 500,
            PerformanceLevel::Minimum => 250,
        }
    }

    fn update_temperature_latches(&mut self, sample: TelemetrySample) {
        if !sample.thermal_available {
            self.thermal_throttled = false;
            self.critical_throttled = false;
            return;
        }
        if sample.temperature_celsius >= self.config.thermal_critical_celsius {
            self.critical_throttled = true;
        } else if sample.temperature_celsius
            <= self.config.thermal_critical_celsius
                - self.config.thermal_recovery_hysteresis_celsius
        {
            self.critical_throttled = false;
        }
        if sample.temperature_celsius >= self.config.thermal_throttle_celsius {
            self.thermal_throttled = true;
        } else if sample.temperature_celsius
            <= self.config.thermal_throttle_celsius
                - self.config.thermal_recovery_hysteresis_celsius
        {
            self.thermal_throttled = false;
        }
    }

    fn desired_performance_level(&self) -> PerformanceLevel {
        let battery_low = self.telemetry.battery_available
            && !self.telemetry.charging
            && self.telemetry.battery_percent <= self.config.low_battery_percent;
        if self.critical_throttled {
            PerformanceLevel::Minimum
        } else if battery_low || self.thermal_throttled {
            PerformanceLevel::Reduced
        } else {
            PerformanceLevel::Nominal
        }
    }
}
