#![no_std]

pub use bexos_tee_slots::{
    CUTOVER_DEADLINE_NS, CommitOutcome, OrchestratorError, PREPARATION_DEADLINE_NS, STATE_BYTES,
    SlotId, StateIdentity, TeeCommit, TeeSlotState, TeeUpdatePhase,
};

use bexos_kernel_core::transplant::{PreservedRegion, SnapshotHeader, SnapshotPhase};

pub const SMC_OWNER_BEXOS: u32 = 0x42;
pub const SMC_FUNC_LIVE_SYNC_READY: u32 = 0x100;
pub const SMC_FUNC_SWITCH_READY: u32 = 0x101;
pub const SMC_FUNC_REPLACE_KERNEL: u32 = 0x102;
pub const SMC_FUNC_RESTORE_READY: u32 = 0x103;
pub const SMC_FUNC_HEARTBEAT: u32 = 0x104;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmcFunction(pub u32);

impl SmcFunction {
    pub const LIVE_SYNC_READY: Self = Self((SMC_OWNER_BEXOS << 24) | SMC_FUNC_LIVE_SYNC_READY);
    pub const SWITCH_READY: Self = Self((SMC_OWNER_BEXOS << 24) | SMC_FUNC_SWITCH_READY);
    pub const REPLACE_KERNEL: Self = Self((SMC_OWNER_BEXOS << 24) | SMC_FUNC_REPLACE_KERNEL);
    pub const RESTORE_READY: Self = Self((SMC_OWNER_BEXOS << 24) | SMC_FUNC_RESTORE_READY);
    pub const HEARTBEAT: Self = Self((SMC_OWNER_BEXOS << 24) | SMC_FUNC_HEARTBEAT);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SmcRequest {
    pub function: SmcFunction,
    pub arg0: u64,
    pub arg1: u64,
    pub arg2: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageSlot {
    pub id: SlotId,
    pub image_hash: [u8; 32],
    pub last_known_good: bool,
    pub bootable: bool,
    pub failures: u8,
}

impl ImageSlot {
    pub const fn new(id: SlotId, image_hash: [u8; 32], last_known_good: bool) -> Self {
        Self {
            id,
            image_hash,
            last_known_good,
            bootable: true,
            failures: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlotTable {
    pub active: SlotId,
    pub slots: [ImageSlot; 2],
}

impl SlotTable {
    pub const fn new(active: SlotId, a_hash: [u8; 32], b_hash: [u8; 32]) -> Self {
        Self {
            active,
            slots: [
                ImageSlot::new(SlotId::A, a_hash, true),
                ImageSlot::new(SlotId::B, b_hash, false),
            ],
        }
    }

    pub fn active_slot(&self) -> &ImageSlot {
        self.slot(self.active)
    }

    pub fn choose_boot_slot(&self) -> SlotId {
        if self.active_slot().bootable {
            return self.active;
        }

        if let Some(slot) = self
            .slots
            .iter()
            .find(|slot| slot.bootable && slot.last_known_good)
        {
            return slot.id;
        }

        self.slots
            .iter()
            .find(|slot| slot.bootable)
            .map_or(self.active, |slot| slot.id)
    }

    pub fn mark_active_failed(&mut self, max_failures: u8) -> SlotId {
        let active = self.active;
        let slot = self.slot_mut(active);
        slot.failures = slot.failures.saturating_add(1);
        if slot.failures >= max_failures {
            slot.bootable = false;
            slot.last_known_good = false;
            self.active = self.choose_boot_slot();
        }
        self.active
    }

    fn slot(&self, id: SlotId) -> &ImageSlot {
        match id {
            SlotId::A => &self.slots[0],
            SlotId::B => &self.slots[1],
        }
    }

    fn slot_mut(&mut self, id: SlotId) -> &mut ImageSlot {
        match id {
            SlotId::A => &mut self.slots[0],
            SlotId::B => &mut self.slots[1],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Watchdog {
    pub last_heartbeat_tick: u64,
    pub missed_heartbeats: u8,
    pub max_missed_heartbeats: u8,
}

impl Watchdog {
    pub const fn new(max_missed_heartbeats: u8) -> Self {
        Self {
            last_heartbeat_tick: 0,
            missed_heartbeats: 0,
            max_missed_heartbeats,
        }
    }

    pub fn heartbeat(&mut self, tick: u64) {
        self.last_heartbeat_tick = tick;
        self.missed_heartbeats = 0;
    }

    pub fn observe(&mut self, tick: u64, expected_interval: u64) -> WatchdogDecision {
        if tick.saturating_sub(self.last_heartbeat_tick) <= expected_interval {
            return WatchdogDecision::Healthy;
        }

        self.missed_heartbeats = self.missed_heartbeats.saturating_add(1);
        self.last_heartbeat_tick = tick;
        if self.missed_heartbeats >= self.max_missed_heartbeats {
            WatchdogDecision::Expired
        } else {
            WatchdogDecision::MissedHeartbeat
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchdogDecision {
    Healthy,
    MissedHeartbeat,
    Expired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransplantState {
    Idle,
    LiveSyncing,
    LiveSyncComplete,
    SwitchMode,
    ReplacementPending,
    RestorePending,
    Restored,
    Fallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeartTransplantOrchestrator {
    pub state: TransplantState,
    pub slots: SlotTable,
    pub watchdog: Watchdog,
    pub preserved_region: Option<PreservedRegion>,
    pub live_snapshot_header: Option<SnapshotHeader>,
    pub switch_snapshot_header: Option<SnapshotHeader>,
}

impl HeartTransplantOrchestrator {
    pub const fn new(slots: SlotTable, watchdog: Watchdog) -> Self {
        Self {
            state: TransplantState::Idle,
            slots,
            watchdog,
            preserved_region: None,
            live_snapshot_header: None,
            switch_snapshot_header: None,
        }
    }

    pub fn begin_live_sync(
        &mut self,
        preserved_region: PreservedRegion,
        snapshot_header: SnapshotHeader,
    ) -> Result<SmcRequest, OrchestratorError> {
        if self.state != TransplantState::Idle {
            return Err(OrchestratorError::InvalidTransition);
        }
        if snapshot_header.phase != SnapshotPhase::LiveBulk {
            return Err(OrchestratorError::WrongSnapshotPhase);
        }

        self.state = TransplantState::LiveSyncing;
        self.preserved_region = Some(preserved_region);
        self.live_snapshot_header = Some(snapshot_header);
        Ok(SmcRequest {
            function: SmcFunction::LIVE_SYNC_READY,
            arg0: preserved_region.start,
            arg1: preserved_region.len,
            arg2: u64::from(snapshot_header.checksum),
        })
    }

    pub fn complete_live_sync(&mut self) -> Result<(), OrchestratorError> {
        if self.state != TransplantState::LiveSyncing {
            return Err(OrchestratorError::InvalidTransition);
        }

        self.state = TransplantState::LiveSyncComplete;
        Ok(())
    }

    pub fn enter_switch_mode(
        &mut self,
        snapshot_header: SnapshotHeader,
    ) -> Result<SmcRequest, OrchestratorError> {
        if self.state != TransplantState::LiveSyncComplete {
            return Err(OrchestratorError::InvalidTransition);
        }
        if snapshot_header.phase != SnapshotPhase::SwitchDelta {
            return Err(OrchestratorError::WrongSnapshotPhase);
        }

        self.state = TransplantState::SwitchMode;
        self.switch_snapshot_header = Some(snapshot_header);
        Ok(SmcRequest {
            function: SmcFunction::SWITCH_READY,
            arg0: self.preserved_region.map_or(0, |region| region.start),
            arg1: snapshot_header.preserved_ptr,
            arg2: u64::from(snapshot_header.checksum),
        })
    }

    pub fn request_replacement(&mut self) -> Result<SmcRequest, OrchestratorError> {
        if self.state != TransplantState::SwitchMode {
            return Err(OrchestratorError::InvalidTransition);
        }

        self.state = TransplantState::ReplacementPending;
        Ok(SmcRequest {
            function: SmcFunction::REPLACE_KERNEL,
            arg0: self.preserved_region.map_or(0, |region| region.start),
            arg1: self
                .switch_snapshot_header
                .map_or(0, |header| header.preserved_ptr),
            arg2: self
                .switch_snapshot_header
                .map_or(0, |header| u64::from(header.checksum)),
        })
    }

    pub fn kernel_replaced(&mut self) -> Result<(), OrchestratorError> {
        if self.state != TransplantState::ReplacementPending {
            return Err(OrchestratorError::InvalidTransition);
        }

        self.state = TransplantState::RestorePending;
        Ok(())
    }

    pub fn restore_complete(&mut self) -> Result<SmcRequest, OrchestratorError> {
        if self.state != TransplantState::RestorePending {
            return Err(OrchestratorError::InvalidTransition);
        }

        self.state = TransplantState::Restored;
        Ok(SmcRequest {
            function: SmcFunction::RESTORE_READY,
            arg0: 0,
            arg1: 0,
            arg2: 0,
        })
    }

    pub fn expire_watchdog(&mut self, max_failures: u8) -> SlotId {
        self.state = TransplantState::Fallback;
        self.slots.mark_active_failed(max_failures)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentKind {
    Kernel,
    D1Driver,
    D2Driver,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentRecord {
    pub kind: ComponentKind,
    pub active_hash: [u8; 32],
    pub last_known_good_hash: [u8; 32],
    pub bootable: bool,
    pub failures: u8,
}

impl ComponentRecord {
    pub const fn new(
        kind: ComponentKind,
        active_hash: [u8; 32],
        last_known_good_hash: [u8; 32],
    ) -> Self {
        Self {
            kind,
            active_hash,
            last_known_good_hash,
            bootable: true,
            failures: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RollbackStage {
    Idle,
    Pending {
        generation: u64,
        artifact_hash: [u8; 32],
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootRollbackState {
    pub min_generation: u64,
    pub active_hash: [u8; 32],
    pub last_known_good_hash: [u8; 32],
    pub stage: RollbackStage,
}

impl BootRollbackState {
    pub const fn new(min_generation: u64, active_hash: [u8; 32], lkg_hash: [u8; 32]) -> Self {
        Self {
            min_generation,
            active_hash,
            last_known_good_hash: lkg_hash,
            stage: RollbackStage::Idle,
        }
    }

    pub fn verify_boot(
        &self,
        generation: u64,
        artifact_hash: [u8; 32],
    ) -> Result<(), OrchestratorError> {
        if generation < self.min_generation {
            return Err(OrchestratorError::Rollback);
        }
        if artifact_hash != self.active_hash && artifact_hash != self.last_known_good_hash {
            return Err(OrchestratorError::InvalidState);
        }
        Ok(())
    }

    pub fn stage_update(
        &mut self,
        generation: u64,
        artifact_hash: [u8; 32],
    ) -> Result<(), OrchestratorError> {
        if generation <= self.min_generation {
            return Err(OrchestratorError::Rollback);
        }
        self.stage = RollbackStage::Pending {
            generation,
            artifact_hash,
        };
        Ok(())
    }

    pub fn commit_update(&mut self) -> Result<(), OrchestratorError> {
        let RollbackStage::Pending {
            generation,
            artifact_hash,
        } = self.stage
        else {
            return Err(OrchestratorError::InvalidState);
        };
        self.min_generation = generation;
        self.last_known_good_hash = self.active_hash;
        self.active_hash = artifact_hash;
        self.stage = RollbackStage::Idle;
        Ok(())
    }

    pub fn abort_update(&mut self) {
        self.stage = RollbackStage::Idle;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RollbackDecision {
    pub kind: ComponentKind,
    pub selected_hash: [u8; 32],
    pub kernel_slot_changed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComponentManifest<const N: usize> {
    pub kernel_slots: SlotTable,
    pub components: [ComponentRecord; N],
}

impl<const N: usize> ComponentManifest<N> {
    pub const fn new(kernel_slots: SlotTable, components: [ComponentRecord; N]) -> Self {
        Self {
            kernel_slots,
            components,
        }
    }

    pub fn active_component(&self, kind: ComponentKind) -> Option<&ComponentRecord> {
        self.components
            .iter()
            .find(|component| component.kind == kind)
    }

    pub fn mark_component_failed(
        &mut self,
        kind: ComponentKind,
        max_kernel_failures: u8,
    ) -> Result<RollbackDecision, OrchestratorError> {
        if kind == ComponentKind::Kernel {
            let before = self.kernel_slots.active;
            let after = self.kernel_slots.mark_active_failed(max_kernel_failures);
            return Ok(RollbackDecision {
                kind,
                selected_hash: self.kernel_slots.active_slot().image_hash,
                kernel_slot_changed: before != after,
            });
        }

        let component = self
            .components
            .iter_mut()
            .find(|component| component.kind == kind)
            .ok_or(OrchestratorError::ComponentNotFound)?;
        component.failures = component.failures.saturating_add(1);
        component.active_hash = component.last_known_good_hash;
        component.bootable = true;

        Ok(RollbackDecision {
            kind,
            selected_hash: component.active_hash,
            kernel_slot_changed: false,
        })
    }
}
