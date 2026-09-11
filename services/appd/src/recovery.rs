extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::driver_manager::DriverExclusions;

pub const RECOVERY_STABLE_WINDOW_MS: u64 = 60_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryDecision {
    RetrySame,
    Fallback,
    Exhausted,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverRecoveryBudget {
    counters: Vec<NodeRecoveryCounter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NodeRecoveryCounter {
    pub node_id: u64,
    pub package_id: String,
    pub process_name: String,
    pub first_failure_ms: u64,
    pub attempts: u32,
}

impl DriverRecoveryBudget {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_failure(
        &mut self,
        node_id: u64,
        package_id: &str,
        process_name: &str,
        now_ms: u64,
        exclusions: &mut DriverExclusions,
    ) -> RecoveryDecision {
        let counter = self.counter_mut(node_id, package_id, process_name, now_ms);
        if now_ms.saturating_sub(counter.first_failure_ms) >= RECOVERY_STABLE_WINDOW_MS {
            counter.first_failure_ms = now_ms;
            counter.attempts = 0;
        }
        counter.attempts += 1;
        if counter.attempts == 1 {
            RecoveryDecision::RetrySame
        } else {
            exclusions.exclude(node_id, package_id, process_name);
            RecoveryDecision::Fallback
        }
    }

    pub fn record_stable(&mut self, node_id: u64, now_ms: u64, exclusions: &mut DriverExclusions) {
        self.counters.retain(|counter| {
            counter.node_id != node_id
                || now_ms.saturating_sub(counter.first_failure_ms) < RECOVERY_STABLE_WINDOW_MS
        });
        exclusions.reset_node(node_id);
    }

    pub fn reset_node(&mut self, node_id: u64, exclusions: &mut DriverExclusions) {
        self.counters.retain(|counter| counter.node_id != node_id);
        exclusions.reset_node(node_id);
    }

    pub fn counters(&self) -> &[NodeRecoveryCounter] {
        &self.counters
    }

    pub fn from_counters(counters: Vec<NodeRecoveryCounter>) -> Self {
        Self { counters }
    }

    fn counter_mut(
        &mut self,
        node_id: u64,
        package_id: &str,
        process_name: &str,
        now_ms: u64,
    ) -> &mut NodeRecoveryCounter {
        if let Some(index) = self.counters.iter().position(|counter| {
            counter.node_id == node_id
                && counter.package_id == package_id
                && counter.process_name == process_name
        }) {
            return &mut self.counters[index];
        }
        self.counters.push(NodeRecoveryCounter {
            node_id,
            package_id: String::from(package_id),
            process_name: String::from(process_name),
            first_failure_ms: now_ms,
            attempts: 0,
        });
        self.counters.last_mut().unwrap()
    }
}
