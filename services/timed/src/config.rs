use alloc::string::{String, ToString};
use bexos_userspace::{Memory, Startup, config::ConfigTable};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimedConfig {
    pub primary_server: String,
    pub poll_interval_ms: u32,
    pub initial_retry_ms: u32,
    pub use_nts: bool,
    pub network_sync_enabled: bool,
    pub slew_limit_ppm: i32,
    pub slew_step_threshold_ns: i64,
}

impl Default for TimedConfig {
    fn default() -> Self {
        Self {
            primary_server: "time.google.com".to_string(),
            poll_interval_ms: 60_000,
            initial_retry_ms: 1_000,
            use_nts: false,
            network_sync_enabled: true,
            slew_limit_ppm: 500,
            slew_step_threshold_ns: 1_000_000_000,
        }
    }
}

impl TimedConfig {
    pub fn from_startup(startup: &Startup) -> Self {
        let Some(config) = startup.config else {
            return Self::default();
        };
        if startup.config_len == 0 {
            let _ = Memory::close(config);
            return Self::default();
        }
        let mapped_len = (startup.config_len + 4095) & !4095;
        let Ok(va) = Memory::map(config, mapped_len, 2) else {
            let _ = Memory::close(config);
            return Self::default();
        };
        let bytes =
            unsafe { core::slice::from_raw_parts(va as *const u8, startup.config_len as usize) };
        let parsed = ConfigTable::parse(bytes)
            .map(Self::from_table)
            .unwrap_or_default();
        let _ = Memory::unmap(va, mapped_len);
        let _ = Memory::close(config);
        parsed
    }

    pub fn from_table(table: ConfigTable<'_>) -> Self {
        let mut config = Self::default();
        config.primary_server = table
            .get_string("primary_server")
            .ok()
            .filter(|server| !server.is_empty() && server.len() <= 128)
            .unwrap_or("time.google.com")
            .to_string();
        config.poll_interval_ms = table
            .get_u32("poll_interval_ms")
            .ok()
            .filter(|value| *value >= 1_000)
            .unwrap_or(60_000);
        config.initial_retry_ms = table
            .get_u32("initial_retry_ms")
            .ok()
            .filter(|value| *value <= 60_000)
            .unwrap_or(1_000);
        config.use_nts = table.get_bool("use_nts").unwrap_or(false);
        config.network_sync_enabled = table.get_bool("network_sync_enabled").unwrap_or(true);
        config.slew_limit_ppm = table
            .get_u32("slew_limit_ppm")
            .ok()
            .filter(|value| (1..=500).contains(value))
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(500);
        config.slew_step_threshold_ns = table
            .get_u64("slew_step_threshold_ns")
            .ok()
            .filter(|value| *value <= 60_000_000_000)
            .and_then(|value| i64::try_from(value).ok())
            .unwrap_or(1_000_000_000);
        config
    }
}
