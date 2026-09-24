use alloc::string::String;
use alloc::vec::Vec;
use bexos_app_registry::{HealthCheckStatus, SemVer};
use bexos_migration::{
    Error,
    codec::{Decoder, Encoder},
};

use crate::platform_config::AppLifecyclePolicy;

/// Commands finish normally; selected shells have a session-aware supervisor.
pub fn automatic_restart(manifest: &crate::Manifest, process: &str) -> bool {
    manifest
        .processes
        .iter()
        .any(|p| p.name == process && p.service && p.shell_role == crate::manifest::ShellRole::None)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchdogPhase {
    Probation,
    Healthy,
    CrashLoop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WatchdogRecord {
    pub package_id: String,
    pub process_name: String,
    pub instance_id: String,
    pub uid: u64,
    pub version: SemVer,
    pub phase: WatchdogPhase,
    pub activated_at_ns: u64,
    pub first_crash_at_ns: u64,
    pub crash_count: u32,
    pub restart_after_ns: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WatchdogAction {
    PromoteHealthy {
        package_id: String,
    },
    Restart {
        package_id: String,
        process_name: String,
        instance_id: String,
        uid: u64,
    },
    RollbackAndRestart {
        package_id: String,
        process_name: String,
        instance_id: String,
        uid: u64,
    },
    StopCrashLoop {
        package_id: String,
    },
}

impl WatchdogRecord {
    pub fn new(
        package_id: String,
        process_name: String,
        instance_id: String,
        uid: u64,
        version: SemVer,
        now_ns: u64,
    ) -> Self {
        Self {
            package_id,
            process_name,
            instance_id,
            uid,
            version,
            phase: WatchdogPhase::Probation,
            activated_at_ns: now_ns,
            first_crash_at_ns: 0,
            crash_count: 0,
            restart_after_ns: None,
        }
    }

    pub fn ready(&mut self, now_ns: u64, policy: AppLifecyclePolicy) -> Option<WatchdogAction> {
        if self.phase != WatchdogPhase::Probation {
            return None;
        }
        let probation_ns = policy
            .probation_window_seconds
            .saturating_mul(1_000_000_000);
        if now_ns.saturating_sub(self.activated_at_ns) < probation_ns {
            return None;
        }
        self.phase = WatchdogPhase::Healthy;
        Some(WatchdogAction::PromoteHealthy {
            package_id: self.package_id.clone(),
        })
    }

    pub fn exit(
        &mut self,
        now_ns: u64,
        policy: AppLifecyclePolicy,
        has_rollback: bool,
    ) -> WatchdogAction {
        let delay_ns = policy.restart_delay_seconds.saturating_mul(1_000_000_000);
        self.restart_after_ns = Some(now_ns.saturating_add(delay_ns));
        if self.phase == WatchdogPhase::Probation {
            self.phase = WatchdogPhase::CrashLoop;
            return rollback_or_stop(self, has_rollback);
        }
        let window_ns = policy.crash_window_seconds.saturating_mul(1_000_000_000);
        if self.first_crash_at_ns == 0 || now_ns.saturating_sub(self.first_crash_at_ns) > window_ns
        {
            self.first_crash_at_ns = now_ns;
            self.crash_count = 1;
        } else {
            self.crash_count = self.crash_count.saturating_add(1);
        }
        if self.crash_count >= policy.crash_threshold {
            self.phase = WatchdogPhase::CrashLoop;
            rollback_or_stop(self, has_rollback)
        } else {
            WatchdogAction::Restart {
                package_id: self.package_id.clone(),
                process_name: self.process_name.clone(),
                instance_id: self.instance_id.clone(),
                uid: self.uid,
            }
        }
    }

    pub fn restart_due(&self, now_ns: u64) -> bool {
        self.restart_after_ns
            .is_some_and(|deadline| now_ns >= deadline)
    }

    pub fn mark_restarted(&mut self, now_ns: u64) {
        self.restart_after_ns = None;
        self.activated_at_ns = now_ns;
        if self.phase == WatchdogPhase::CrashLoop {
            self.phase = WatchdogPhase::Probation;
        }
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut w = Encoder::new();
        w.word(2);
        w.text(&self.package_id);
        w.text(&self.process_name);
        w.text(&self.instance_id);
        w.word(self.uid);
        encode_semver(&mut w, &self.version);
        w.word(match self.phase {
            WatchdogPhase::Probation => 1,
            WatchdogPhase::Healthy => 2,
            WatchdogPhase::CrashLoop => 3,
        });
        w.word(self.activated_at_ns);
        w.word(self.first_crash_at_ns);
        w.word(self.crash_count as u64);
        if let Some(deadline) = self.restart_after_ns {
            w.word(1);
            w.word(deadline);
        } else {
            w.word(0);
        }
        w.finish()
    }

    pub fn from_checkpoint(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Decoder::new(bytes);
        let version = r.word()?;
        if !(1..=2).contains(&version) {
            return Err(Error::UnsupportedVersion);
        }
        let package_id = r.text(128)?.to_string();
        let process_name = r.text(64)?.to_string();
        let instance_id = if version >= 2 {
            r.text(128)?.to_string()
        } else {
            String::new()
        };
        let uid = r.word()?;
        let package_version = decode_semver(&mut r)?;
        let phase = match r.word()? {
            1 => WatchdogPhase::Probation,
            2 => WatchdogPhase::Healthy,
            3 => WatchdogPhase::CrashLoop,
            _ => return Err(Error::InvalidData),
        };
        let activated_at_ns = r.word()?;
        let first_crash_at_ns = r.word()?;
        let crash_count = u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?;
        let restart_after_ns = if r.flag()? { Some(r.word()?) } else { None };
        r.finish()?;
        if package_id.is_empty() || process_name.is_empty() {
            return Err(Error::InvalidData);
        }
        Ok(Self {
            package_id,
            process_name,
            instance_id,
            uid,
            version: package_version,
            phase,
            activated_at_ns,
            first_crash_at_ns,
            crash_count,
            restart_after_ns,
        })
    }
}

pub fn health_for_phase(phase: WatchdogPhase) -> HealthCheckStatus {
    match phase {
        WatchdogPhase::Probation => HealthCheckStatus::Probation,
        WatchdogPhase::Healthy => HealthCheckStatus::Healthy,
        WatchdogPhase::CrashLoop => HealthCheckStatus::CrashLoop,
    }
}

fn rollback_or_stop(record: &WatchdogRecord, has_rollback: bool) -> WatchdogAction {
    if has_rollback {
        WatchdogAction::RollbackAndRestart {
            package_id: record.package_id.clone(),
            process_name: record.process_name.clone(),
            instance_id: record.instance_id.clone(),
            uid: record.uid,
        }
    } else {
        WatchdogAction::StopCrashLoop {
            package_id: record.package_id.clone(),
        }
    }
}

fn encode_semver(w: &mut Encoder, version: &SemVer) {
    w.word(version.major as u64);
    w.word(version.minor as u64);
    w.word(version.patch as u64);
    w.word(version.build as u64);
    w.text(&version.prerelease);
}

fn decode_semver(r: &mut Decoder<'_>) -> Result<SemVer, Error> {
    Ok(SemVer {
        major: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        minor: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        patch: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        build: u32::try_from(r.word()?).map_err(|_| Error::InvalidData)?,
        prerelease: r.text(64)?.to_string(),
    })
}
