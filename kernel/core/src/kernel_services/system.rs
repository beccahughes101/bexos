use super::handle::ObjectKind;
use super::{
    CheckpointResult, ControlPlane, Handle, HardwareAccess, KernelServiceStatus, RIGHT_ADMIN,
    RIGHT_EXECUTE, RIGHT_MANAGE_TASK, RIGHT_MAP, RIGHT_READ, RIGHT_SET_POLICY, RIGHT_SIGNAL,
    RIGHT_TRANSFER, RIGHT_WRITE, SIGNAL_TERMINATED,
};

pub const RESOURCE_GROUP_SYSTEM: u32 = 1;
pub const RESOURCE_GROUP_FOREGROUND: u32 = 2;
pub const RESOURCE_GROUP_BACKGROUND: u32 = 3;
pub const RESOURCE_GROUP_DRIVER: u32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessState {
    Created,
    Running,
    Suspended,
    Exited,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessRecord {
    pub id: u64,
    pub name: [u8; 64],
    pub name_len: usize,
    pub package_id: [u8; 96],
    pub package_id_len: usize,
    pub hardware_access: HardwareAccess,
    pub resource_group_id: u32,
    pub vm_space_id: u64,
    pub main_thread_id: u64,
    pub state: ProcessState,
    pub root_channel_handle: Option<Handle>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VmSpaceRecord {
    pub id: u64,
    pub process_id: u64,
    pub root_table_phys: u64,
    pub asid: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceGroupRecord {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub name: [u8; 24],
    pub name_len: usize,
    pub cpu_shares: u32,
    pub memory_limit_pages: u64,
    pub max_cpu_utilization_permille: u16,
    pub allow_realtime: bool,
    pub memory_low_watermark_bytes: u64,
    pub memory_high_watermark_bytes: u64,
    pub memory_used_bytes: u64,
    pub max_render_budget_percent: u8,
    pub max_vram_bytes: u64,
    pub reserved_render_budget_percent: u8,
    pub reserved_vram_bytes: u64,
}

impl ResourceGroupRecord {
    pub fn name_str(&self) -> Result<&str, KernelServiceStatus> {
        core::str::from_utf8(&self.name[..self.name_len])
            .map_err(|_| KernelServiceStatus::InvalidArgs)
    }
}

const SYSTEM_NAME: [u8; 24] = [
    b's', b'y', b's', b't', b'e', b'm', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];
const FOREGROUND_NAME: [u8; 24] = [
    b'f', b'o', b'r', b'e', b'g', b'r', b'o', b'u', b'n', b'd', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];
const BACKGROUND_NAME: [u8; 24] = [
    b'b', b'a', b'c', b'k', b'g', b'r', b'o', b'u', b'n', b'd', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0,
];
const DRIVER_NAME: [u8; 24] = [
    b'd', b'r', b'i', b'v', b'e', b'r', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterruptRecord {
    pub id: u64,
    pub irq_number: u32,
    pub flags: u32,
    pub masked: bool,
    pub awaiting_ack: bool,
    pub window_started_ns: u64,
    pub events_in_window: u32,
    pub flood_limit_per_second: u32,
}

pub const DEFAULT_INTERRUPT_FLOOD_LIMIT_PER_SECOND: u32 = 100_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptDelivery {
    Signaled { interrupt_id: u64 },
    Masked,
    FloodLimited { interrupt_id: u64 },
    NotBound,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessTable {
    pub(super) entries: Vec<ProcessRecord>,
    next_process_id: u64,
}

impl ProcessTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_process_id: 1,
        }
    }

    pub fn insert(
        &mut self,
        name: &str,
        resource_group_id: u32,
        vm_space_id: u64,
        package_id: &str,
        hardware_access: HardwareAccess,
    ) -> Result<u64, KernelServiceStatus> {
        if name.is_empty() || name.len() > 64 || package_id.len() > 96 || vm_space_id == 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let id = self.next_process_id;
        self.next_process_id = self.next_process_id.saturating_add(1);
        let mut stored = [0u8; 64];
        stored[..name.len()].copy_from_slice(name.as_bytes());
        let mut stored_package_id = [0u8; 96];
        stored_package_id[..package_id.len()].copy_from_slice(package_id.as_bytes());
        self.entries.push(ProcessRecord {
            id,
            name: stored,
            name_len: name.len(),
            package_id: stored_package_id,
            package_id_len: package_id.len(),
            hardware_access,
            resource_group_id,
            vm_space_id,
            main_thread_id: 0,
            state: ProcessState::Created,
            root_channel_handle: None,
        });
        Ok(id)
    }

    pub fn get(&self, id: u64) -> Option<ProcessRecord> {
        self.entries
            .iter()
            .find(|process| process.id == id)
            .copied()
    }

    pub fn first(&self) -> Option<ProcessRecord> {
        self.entries.first().copied()
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut ProcessRecord> {
        self.entries.iter_mut().find(|process| process.id == id)
    }

    pub fn set_main_thread(&mut self, id: u64, thread_id: u64) -> Result<(), KernelServiceStatus> {
        let Some(process) = self.get_mut(id) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        process.main_thread_id = thread_id;
        process.state = ProcessState::Running;
        Ok(())
    }

    pub fn set_state(&mut self, id: u64, state: ProcessState) -> Result<(), KernelServiceStatus> {
        let Some(process) = self.get_mut(id) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        process.state = state;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> core::slice::Iter<'_, ProcessRecord> {
        self.entries.iter()
    }
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VmSpaceTable {
    pub(super) entries: Vec<VmSpaceRecord>,
    next_vm_space_id: u64,
}

impl VmSpaceTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_vm_space_id: 1,
        }
    }

    pub fn insert(
        &mut self,
        process_id: u64,
        root_table_phys: u64,
        asid: u16,
    ) -> Result<u64, KernelServiceStatus> {
        if process_id == 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let id = self.next_vm_space_id;
        self.next_vm_space_id = self.next_vm_space_id.saturating_add(1);
        self.entries.push(VmSpaceRecord {
            id,
            process_id,
            root_table_phys,
            asid,
        });
        Ok(id)
    }

    pub fn get(&self, id: u64) -> Option<VmSpaceRecord> {
        self.entries
            .iter()
            .find(|vm_space| vm_space.id == id)
            .copied()
    }

    pub fn for_process(&self, process_id: u64) -> Option<VmSpaceRecord> {
        self.entries
            .iter()
            .find(|vm_space| vm_space.process_id == process_id)
            .copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Default for VmSpaceTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceGroupTable {
    entries: Vec<ResourceGroupRecord>,
    next_resource_group_id: u32,
    reservations: Vec<GpuReservationRecord>,
    next_reservation_id: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuReservationRecord {
    pub id: u64,
    pub group_id: u32,
    pub render_budget_percent: u8,
    pub vram_bytes: u64,
}

impl ResourceGroupTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_resource_group_id: RESOURCE_GROUP_DRIVER + 1,
            reservations: Vec::new(),
            next_reservation_id: 1,
        }
    }

    pub fn ancestor_ids(&self, id: u32) -> Vec<u32> {
        let mut result = Vec::new();
        let mut next = Some(id);
        while let Some(current) = next {
            let Some(group) = self.get(current) else {
                return Vec::new();
            };
            result.push(current);
            next = group.parent_id;
            if result.len() > 32 {
                return Vec::new();
            }
        }
        result
    }

    pub fn with_defaults() -> Self {
        let mut entries = Vec::new();
        entries.push(ResourceGroupRecord {
            id: RESOURCE_GROUP_SYSTEM,
            parent_id: None,
            name: SYSTEM_NAME,
            name_len: 6,
            cpu_shares: 2048,
            memory_limit_pages: 0,
            max_cpu_utilization_permille: 1000,
            allow_realtime: true,
            memory_low_watermark_bytes: 0,
            memory_high_watermark_bytes: 0,
            memory_used_bytes: 0,
            max_render_budget_percent: 0,
            max_vram_bytes: 0,
            reserved_render_budget_percent: 0,
            reserved_vram_bytes: 0,
        });
        entries.push(ResourceGroupRecord {
            id: RESOURCE_GROUP_FOREGROUND,
            parent_id: Some(RESOURCE_GROUP_SYSTEM),
            name: FOREGROUND_NAME,
            name_len: 10,
            cpu_shares: 1024,
            memory_limit_pages: 0,
            max_cpu_utilization_permille: 0,
            allow_realtime: true,
            memory_low_watermark_bytes: 0,
            memory_high_watermark_bytes: 0,
            memory_used_bytes: 0,
            max_render_budget_percent: 0,
            max_vram_bytes: 0,
            reserved_render_budget_percent: 0,
            reserved_vram_bytes: 0,
        });
        entries.push(ResourceGroupRecord {
            id: RESOURCE_GROUP_BACKGROUND,
            parent_id: Some(RESOURCE_GROUP_SYSTEM),
            name: BACKGROUND_NAME,
            name_len: 10,
            cpu_shares: 256,
            memory_limit_pages: 0,
            max_cpu_utilization_permille: 0,
            allow_realtime: false,
            memory_low_watermark_bytes: 0,
            memory_high_watermark_bytes: 0,
            memory_used_bytes: 0,
            max_render_budget_percent: 0,
            max_vram_bytes: 0,
            reserved_render_budget_percent: 0,
            reserved_vram_bytes: 0,
        });
        entries.push(ResourceGroupRecord {
            id: RESOURCE_GROUP_DRIVER,
            parent_id: Some(RESOURCE_GROUP_SYSTEM),
            name: DRIVER_NAME,
            name_len: 6,
            cpu_shares: 1536,
            memory_limit_pages: 0,
            max_cpu_utilization_permille: 0,
            allow_realtime: true,
            memory_low_watermark_bytes: 0,
            memory_high_watermark_bytes: 0,
            memory_used_bytes: 0,
            max_render_budget_percent: 0,
            max_vram_bytes: 0,
            reserved_render_budget_percent: 0,
            reserved_vram_bytes: 0,
        });
        Self {
            entries,
            next_resource_group_id: RESOURCE_GROUP_DRIVER + 1,
            reservations: Vec::new(),
            next_reservation_id: 1,
        }
    }

    pub fn install_defaults(&mut self) {
        let _ = self.insert(RESOURCE_GROUP_SYSTEM, "system", 2048, 0);
        let _ = self.insert(RESOURCE_GROUP_FOREGROUND, "foreground", 1024, 0);
        let _ = self.insert(RESOURCE_GROUP_BACKGROUND, "background", 256, 0);
        let _ = self.insert(RESOURCE_GROUP_DRIVER, "driver", 1536, 0);
    }

    pub fn insert(
        &mut self,
        id: u32,
        name: &str,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<(), KernelServiceStatus> {
        if id == 0 || name.is_empty() || name.len() > 24 || cpu_shares == 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        if self.entries.iter().any(|group| {
            group.name_len == name.len() && &group.name[..group.name_len] == name.as_bytes()
        }) {
            return Err(KernelServiceStatus::AlreadyExists);
        }
        if self.entries.iter().any(|group| group.id == id) {
            return Err(KernelServiceStatus::AlreadyExists);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let mut stored = [0; 24];
        stored[..name.len()].copy_from_slice(name.as_bytes());
        self.entries.push(ResourceGroupRecord {
            id,
            parent_id: Some(RESOURCE_GROUP_FOREGROUND),
            name: stored,
            name_len: name.len(),
            cpu_shares,
            memory_limit_pages,
            max_cpu_utilization_permille: 0,
            allow_realtime: false,
            memory_low_watermark_bytes: 0,
            memory_high_watermark_bytes: memory_limit_pages.saturating_mul(4096),
            memory_used_bytes: 0,
            max_render_budget_percent: 0,
            max_vram_bytes: 0,
            reserved_render_budget_percent: 0,
            reserved_vram_bytes: 0,
        });
        Ok(())
    }

    pub fn create(
        &mut self,
        name: &str,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<ResourceGroupRecord, KernelServiceStatus> {
        let id = self.next_resource_group_id;
        self.next_resource_group_id = self.next_resource_group_id.saturating_add(1);
        self.insert(id, name, cpu_shares, memory_limit_pages)?;
        self.get(id).ok_or(KernelServiceStatus::InvalidHandle)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_v2(
        &mut self,
        name: &str,
        parent_id: Option<u32>,
        cpu_shares: u32,
        max_cpu_utilization_permille: u16,
        allow_realtime: bool,
        memory_low_watermark_bytes: u64,
        memory_high_watermark_bytes: u64,
        max_render_budget_percent: u8,
        max_vram_bytes: u64,
    ) -> Result<ResourceGroupRecord, KernelServiceStatus> {
        if cpu_shares == 0
            || max_cpu_utilization_permille > 1000
            || max_render_budget_percent > 100
            || memory_high_watermark_bytes != 0
                && memory_low_watermark_bytes > memory_high_watermark_bytes
            || parent_id.is_some_and(|id| self.get(id).is_none())
        {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let id = self.next_resource_group_id;
        self.next_resource_group_id = self.next_resource_group_id.saturating_add(1);
        self.insert(id, name, cpu_shares, 0)?;
        let group = self
            .entries
            .last_mut()
            .ok_or(KernelServiceStatus::NoMemory)?;
        group.parent_id = parent_id.or(Some(RESOURCE_GROUP_FOREGROUND));
        group.max_cpu_utilization_permille = max_cpu_utilization_permille;
        group.allow_realtime = allow_realtime;
        group.memory_low_watermark_bytes = memory_low_watermark_bytes;
        group.memory_high_watermark_bytes = memory_high_watermark_bytes;
        group.max_render_budget_percent = max_render_budget_percent;
        group.max_vram_bytes = max_vram_bytes;
        Ok(*group)
    }

    pub fn set_limits(
        &mut self,
        id: u32,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<ResourceGroupRecord, KernelServiceStatus> {
        if cpu_shares == 0 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let Some(group) = self.entries.iter_mut().find(|group| group.id == id) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        group.cpu_shares = cpu_shares;
        group.memory_limit_pages = memory_limit_pages;
        Ok(*group)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_limits_v2(
        &mut self,
        id: u32,
        cpu_shares: u32,
        max_cpu_utilization_permille: u16,
        allow_realtime: bool,
        memory_low_watermark_bytes: u64,
        memory_high_watermark_bytes: u64,
        max_render_budget_percent: u8,
        max_vram_bytes: u64,
    ) -> Result<ResourceGroupRecord, KernelServiceStatus> {
        if cpu_shares == 0
            || max_cpu_utilization_permille > 1000
            || max_render_budget_percent > 100
            || memory_high_watermark_bytes != 0
                && memory_low_watermark_bytes > memory_high_watermark_bytes
        {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let Some(group) = self.entries.iter_mut().find(|group| group.id == id) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        group.cpu_shares = cpu_shares;
        group.max_cpu_utilization_permille = max_cpu_utilization_permille;
        group.allow_realtime = allow_realtime;
        group.memory_low_watermark_bytes = memory_low_watermark_bytes;
        group.memory_high_watermark_bytes = memory_high_watermark_bytes;
        group.max_render_budget_percent = max_render_budget_percent;
        group.max_vram_bytes = max_vram_bytes;
        Ok(*group)
    }

    pub fn get(&self, id: u32) -> Option<ResourceGroupRecord> {
        self.entries.iter().find(|group| group.id == id).copied()
    }

    pub fn get_by_name(&self, name: &str) -> Option<ResourceGroupRecord> {
        self.entries
            .iter()
            .find(|group| {
                group.name_len == name.len() && &group.name[..group.name_len] == name.as_bytes()
            })
            .copied()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> core::slice::Iter<'_, ResourceGroupRecord> {
        self.entries.iter()
    }

    pub fn reserve_gpu(
        &mut self,
        group_id: u32,
        render_budget_percent: u8,
        vram_bytes: u64,
    ) -> Result<GpuReservationRecord, KernelServiceStatus> {
        if render_budget_percent > 100 || self.get(group_id).is_none() {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let mut ancestor = Some(group_id);
        while let Some(id) = ancestor {
            let group = self.get(id).ok_or(KernelServiceStatus::InvalidHandle)?;
            if group.max_render_budget_percent != 0
                && u16::from(group.reserved_render_budget_percent)
                    + u16::from(render_budget_percent)
                    > u16::from(group.max_render_budget_percent)
                || group.max_vram_bytes != 0
                    && group.reserved_vram_bytes.saturating_add(vram_bytes) > group.max_vram_bytes
            {
                return Err(KernelServiceStatus::ResourceExhausted);
            }
            ancestor = group.parent_id;
        }
        let reservation = GpuReservationRecord {
            id: self.next_reservation_id,
            group_id,
            render_budget_percent,
            vram_bytes,
        };
        self.next_reservation_id = self.next_reservation_id.saturating_add(1);
        self.reservations
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let mut ancestor = Some(group_id);
        while let Some(id) = ancestor {
            let group = self
                .entries
                .iter_mut()
                .find(|group| group.id == id)
                .ok_or(KernelServiceStatus::InvalidHandle)?;
            group.reserved_render_budget_percent = group
                .reserved_render_budget_percent
                .saturating_add(render_budget_percent);
            group.reserved_vram_bytes = group.reserved_vram_bytes.saturating_add(vram_bytes);
            ancestor = group.parent_id;
        }
        self.reservations.push(reservation);
        Ok(reservation)
    }

    pub fn charge_memory(&mut self, group_id: u32, bytes: u64) -> Result<(), KernelServiceStatus> {
        if bytes == 0 || self.get(group_id).is_none() {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let mut ancestor = Some(group_id);
        while let Some(id) = ancestor {
            let group = self.get(id).ok_or(KernelServiceStatus::InvalidHandle)?;
            if group.memory_high_watermark_bytes != 0
                && group.memory_used_bytes.saturating_add(bytes) > group.memory_high_watermark_bytes
            {
                return Err(KernelServiceStatus::ResourceExhausted);
            }
            ancestor = group.parent_id;
        }
        let mut ancestor = Some(group_id);
        while let Some(id) = ancestor {
            let group = self
                .entries
                .iter_mut()
                .find(|group| group.id == id)
                .ok_or(KernelServiceStatus::InvalidHandle)?;
            group.memory_used_bytes = group.memory_used_bytes.saturating_add(bytes);
            ancestor = group.parent_id;
        }
        Ok(())
    }

    pub fn release_memory(&mut self, group_id: u32, bytes: u64) -> Result<(), KernelServiceStatus> {
        if bytes == 0 || self.get(group_id).is_none() {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let mut ancestor = Some(group_id);
        while let Some(id) = ancestor {
            let group = self
                .entries
                .iter_mut()
                .find(|group| group.id == id)
                .ok_or(KernelServiceStatus::InvalidHandle)?;
            group.memory_used_bytes = group.memory_used_bytes.saturating_sub(bytes);
            ancestor = group.parent_id;
        }
        Ok(())
    }

    pub fn release_gpu(&mut self, reservation_id: u64) -> Result<(), KernelServiceStatus> {
        let index = self
            .reservations
            .iter()
            .position(|reservation| reservation.id == reservation_id)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        let reservation = self.reservations.swap_remove(index);
        let mut ancestor = Some(reservation.group_id);
        while let Some(id) = ancestor {
            let group = self
                .entries
                .iter_mut()
                .find(|group| group.id == id)
                .ok_or(KernelServiceStatus::InvalidHandle)?;
            group.reserved_render_budget_percent = group
                .reserved_render_budget_percent
                .saturating_sub(reservation.render_budget_percent);
            group.reserved_vram_bytes = group
                .reserved_vram_bytes
                .saturating_sub(reservation.vram_bytes);
            ancestor = group.parent_id;
        }
        Ok(())
    }
}

impl Default for ResourceGroupTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterruptTable {
    entries: Vec<InterruptRecord>,
    next_interrupt_id: u64,
}

impl InterruptTable {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_interrupt_id: 1,
        }
    }

    pub fn insert(&mut self, irq_number: u32, flags: u32) -> Result<u64, KernelServiceStatus> {
        if self
            .entries
            .iter()
            .any(|record| record.irq_number == irq_number)
        {
            return Err(KernelServiceStatus::AlreadyExists);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| KernelServiceStatus::NoMemory)?;
        let id = self.next_interrupt_id;
        self.next_interrupt_id = self.next_interrupt_id.saturating_add(1);
        self.entries.push(InterruptRecord {
            id,
            irq_number,
            flags,
            masked: false,
            awaiting_ack: false,
            window_started_ns: 0,
            events_in_window: 0,
            flood_limit_per_second: DEFAULT_INTERRUPT_FLOOD_LIMIT_PER_SECOND,
        });
        Ok(id)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn record(&self, id: u64) -> Option<&InterruptRecord> {
        self.entries.iter().find(|record| record.id == id)
    }

    pub fn set_flood_limit(&mut self, id: u64, limit: u32) -> Result<(), KernelServiceStatus> {
        if limit == 0 || limit > 10_000_000 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        let record = self
            .entries
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        record.flood_limit_per_second = limit;
        Ok(())
    }

    pub fn deliver(&mut self, irq_number: u32, now_ns: u64) -> InterruptDelivery {
        let Some(record) = self
            .entries
            .iter_mut()
            .find(|record| record.irq_number == irq_number)
        else {
            return InterruptDelivery::NotBound;
        };
        if record.masked || record.awaiting_ack {
            return InterruptDelivery::Masked;
        }
        if now_ns.saturating_sub(record.window_started_ns) >= 1_000_000_000 {
            record.window_started_ns = now_ns;
            record.events_in_window = 0;
        }
        record.events_in_window = record.events_in_window.saturating_add(1);
        if record.events_in_window > record.flood_limit_per_second {
            record.masked = true;
            return InterruptDelivery::FloodLimited {
                interrupt_id: record.id,
            };
        }
        record.masked = true;
        record.awaiting_ack = true;
        InterruptDelivery::Signaled {
            interrupt_id: record.id,
        }
    }

    pub fn acknowledge(&mut self, id: u64) -> Result<(), KernelServiceStatus> {
        let record = self
            .entries
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        if !record.awaiting_ack {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        record.awaiting_ack = false;
        record.masked = false;
        Ok(())
    }

    pub fn mask(&mut self, id: u64, masked: bool) -> Result<(), KernelServiceStatus> {
        let record = self
            .entries
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(KernelServiceStatus::InvalidHandle)?;
        if !masked && record.awaiting_ack {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        record.masked = masked;
        Ok(())
    }
}

impl Default for InterruptTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlPlane {
    pub fn create_resource_group(
        &mut self,
        name: &str,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<(u32, Handle), KernelServiceStatus> {
        let group = self
            .resource_groups
            .create(name, cpu_shares, memory_limit_pages)?;
        self.scheduler
            .ensure_resource_group_with_shares(group.id, group.cpu_shares)
            .map_err(super::scheduler::scheduler_error)?;
        let handle = self.handles.insert(
            u64::from(group.id),
            ObjectKind::ResourceGroup,
            RIGHT_ADMIN | RIGHT_READ | RIGHT_SET_POLICY | RIGHT_TRANSFER,
            0,
            None,
        )?;
        Ok((group.id, handle))
    }

    pub fn set_resource_group_limits(
        &mut self,
        group: Handle,
        cpu_shares: u32,
        memory_limit_pages: u64,
    ) -> Result<ResourceGroupRecord, KernelServiceStatus> {
        let record = self.resource_group_for_handle(group, RIGHT_SET_POLICY)?;
        let updated = self
            .resource_groups
            .set_limits(record.id, cpu_shares, memory_limit_pages)?;
        self.scheduler
            .set_resource_group_shares(updated.id, updated.cpu_shares)
            .map_err(super::scheduler::scheduler_error)?;
        Ok(updated)
    }

    pub fn get_resource_group(
        &self,
        group: Handle,
    ) -> Result<ResourceGroupRecord, KernelServiceStatus> {
        self.resource_group_for_handle(group, RIGHT_READ)
    }

    pub fn resource_group_for_handle(
        &self,
        group: Handle,
        rights: u32,
    ) -> Result<ResourceGroupRecord, KernelServiceStatus> {
        let Some(record) = self.handles.get(group.raw) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::ResourceGroup || !record.has_rights(rights) {
            return Err(KernelServiceStatus::AccessDenied);
        }
        self.resource_groups
            .get(record.object_id as u32)
            .ok_or(KernelServiceStatus::InvalidHandle)
    }

    pub fn create_resource_group_v2(
        &mut self,
        name: &str,
        parent: Option<Handle>,
        cpu_shares: u32,
        max_cpu_utilization_permille: u16,
        allow_realtime: bool,
        memory_low_watermark_bytes: u64,
        memory_high_watermark_bytes: u64,
        max_render_budget_percent: u8,
        max_vram_bytes: u64,
    ) -> Result<(u32, Handle), KernelServiceStatus> {
        let parent_id = match parent {
            Some(handle) => Some(self.resource_group_for_handle(handle, RIGHT_READ)?.id),
            None => None,
        };
        let group = self.resource_groups.create_v2(
            name,
            parent_id,
            cpu_shares,
            max_cpu_utilization_permille,
            allow_realtime,
            memory_low_watermark_bytes,
            memory_high_watermark_bytes,
            max_render_budget_percent,
            max_vram_bytes,
        )?;
        self.scheduler
            .ensure_resource_group_with_limits(
                group.id,
                group.parent_id,
                group.cpu_shares,
                group.max_cpu_utilization_permille,
                group.allow_realtime,
            )
            .map_err(super::scheduler::scheduler_error)?;
        let handle = self.handles.insert(
            u64::from(group.id),
            ObjectKind::ResourceGroup,
            RIGHT_ADMIN | RIGHT_READ | RIGHT_SET_POLICY | RIGHT_TRANSFER,
            0,
            None,
        )?;
        Ok((group.id, handle))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_resource_group_limits_v2(
        &mut self,
        group: Handle,
        cpu_shares: u32,
        max_cpu_utilization_permille: u16,
        allow_realtime: bool,
        memory_low_watermark_bytes: u64,
        memory_high_watermark_bytes: u64,
        max_render_budget_percent: u8,
        max_vram_bytes: u64,
    ) -> Result<ResourceGroupRecord, KernelServiceStatus> {
        let record = self.resource_group_for_handle(group, RIGHT_SET_POLICY)?;
        let updated = self.resource_groups.set_limits_v2(
            record.id,
            cpu_shares,
            max_cpu_utilization_permille,
            allow_realtime,
            memory_low_watermark_bytes,
            memory_high_watermark_bytes,
            max_render_budget_percent,
            max_vram_bytes,
        )?;
        self.scheduler
            .set_resource_group_limits(
                updated.id,
                updated.cpu_shares,
                updated.max_cpu_utilization_permille,
                updated.allow_realtime,
            )
            .map_err(super::scheduler::scheduler_error)?;
        Ok(updated)
    }

    pub fn reserve_gpu_resources(
        &mut self,
        group: Handle,
        render_budget_percent: u8,
        vram_bytes: u64,
    ) -> Result<Handle, KernelServiceStatus> {
        let group = self.resource_group_for_handle(group, RIGHT_SET_POLICY)?;
        let reservation =
            self.resource_groups
                .reserve_gpu(group.id, render_budget_percent, vram_bytes)?;
        let (process, _) = self.ensure_current_process()?;
        let owner_process_id = self.process_for_handle(process)?.id;
        self.handles.insert(
            reservation.id,
            ObjectKind::GpuReservation,
            RIGHT_TRANSFER,
            owner_process_id,
            None,
        )
    }

    pub fn close_handle(&mut self, handle: Handle) -> Result<(), KernelServiceStatus> {
        let Some(record) = self.handles.remove(handle.raw) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if record.kind == ObjectKind::GpuReservation {
            self.resource_groups.release_gpu(record.object_id)?;
        }
        Ok(())
    }

    pub(super) fn close_handles_for_process(&mut self, process_id: u64) {
        while let Some(record) = self.handles.remove_for_owner(process_id) {
            if record.kind == ObjectKind::GpuReservation {
                let _ = self.resource_groups.release_gpu(record.object_id);
            }
        }
    }

    pub fn create_process(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccess,
    ) -> Result<(Handle, Handle), KernelServiceStatus> {
        self.create_process_with_realtime(
            name,
            resource_group_id,
            package_id,
            hardware_access,
            false,
        )
    }

    pub fn create_process_with_realtime(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccess,
        realtime_scheduling: bool,
    ) -> Result<(Handle, Handle), KernelServiceStatus> {
        self.create_process_with_realtime_and_root(
            name,
            resource_group_id,
            package_id,
            hardware_access,
            realtime_scheduling,
        )
        .map(|(process, vm_space, _root_vmar)| (process, vm_space))
    }

    pub fn create_process_with_realtime_and_root(
        &mut self,
        name: &str,
        resource_group_id: u32,
        package_id: &str,
        hardware_access: HardwareAccess,
        realtime_scheduling: bool,
    ) -> Result<(Handle, Handle, Handle), KernelServiceStatus> {
        if name.is_empty() || name.len() > 64 || package_id.len() > 96 {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        if self.resource_groups.get(resource_group_id).is_none() {
            return Err(KernelServiceStatus::InvalidArgs);
        }
        if realtime_scheduling
            && !self
                .resource_groups
                .ancestor_ids(resource_group_id)
                .iter()
                .all(|id| {
                    self.resource_groups
                        .get(*id)
                        .is_some_and(|group| group.allow_realtime)
                })
        {
            return Err(KernelServiceStatus::AccessDenied);
        }
        let asid = self.asids.allocate().ok_or(KernelServiceStatus::NoMemory)?;
        let vm_space_id = self.vm_spaces.insert(1, 0, asid)?;
        let root_vmar_id = self.vmars.insert_root(vm_space_id)?;
        let process_id = self.processes.insert(
            name,
            resource_group_id,
            vm_space_id,
            package_id,
            hardware_access,
        )?;
        if let Some(vm_space) = self
            .vm_spaces
            .entries
            .iter_mut()
            .find(|vm_space| vm_space.id == vm_space_id)
        {
            vm_space.process_id = process_id;
        }
        let process_handle = self.handles.insert(
            process_id,
            ObjectKind::Process,
            RIGHT_ADMIN | RIGHT_MANAGE_PROCESS,
            0,
            None,
        )?;
        let vm_space_handle = self.handles.insert(
            vm_space_id,
            ObjectKind::VmSpace,
            RIGHT_READ | RIGHT_WRITE | RIGHT_MAP | RIGHT_ADMIN | RIGHT_TRANSFER,
            0,
            None,
        )?;
        let root_vmar_handle = self.handles.insert(
            root_vmar_id,
            ObjectKind::Vmar,
            RIGHT_READ | RIGHT_WRITE | RIGHT_EXECUTE | RIGHT_MAP | RIGHT_ADMIN | RIGHT_TRANSFER,
            0,
            None,
        )?;
        Ok((process_handle, vm_space_handle, root_vmar_handle))
    }

    pub fn process_for_handle(
        &self,
        process: Handle,
    ) -> Result<ProcessRecord, KernelServiceStatus> {
        let Some(record) = self.handles.get(process.raw) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Process
            || !record.has_rights(RIGHT_ADMIN) && !record.has_rights(RIGHT_MANAGE_TASK)
        {
            return Err(KernelServiceStatus::AccessDenied);
        }
        self.processes
            .get(record.object_id)
            .ok_or(KernelServiceStatus::InvalidHandle)
    }

    pub fn terminate_process(&mut self, process: Handle, exit_code: i32) -> KernelServiceStatus {
        let process_record = match self.process_for_handle(process) {
            Ok(record) => record,
            Err(status) => return status,
        };
        if process_record.state == ProcessState::Exited {
            return KernelServiceStatus::Ok;
        }
        for thread_id in self.threads.exit_process(process_record.id, exit_code) {
            self.handles
                .add_signals_for_object(thread_id, ObjectKind::Thread, SIGNAL_TERMINATED);
            self.wake_handle_waiters_for_object(thread_id, ObjectKind::Thread, SIGNAL_TERMINATED);
            let _ = self.scheduler.exit_task(thread_id, exit_code);
            for owner in self.futexes.remove_thread(thread_id) {
                self.restore_inherited_priority(owner);
            }
        }
        let _ = self
            .processes
            .set_state(process_record.id, ProcessState::Exited);
        self.handles.add_signals_for_object(
            process_record.id,
            ObjectKind::Process,
            SIGNAL_TERMINATED,
        );
        self.wake_handle_waiters_for_object(
            process_record.id,
            ObjectKind::Process,
            SIGNAL_TERMINATED,
        );
        self.close_handles_for_process(process_record.id);
        KernelServiceStatus::Ok
    }

    pub fn vm_space_for_handle(
        &self,
        vm_space: Handle,
    ) -> Result<VmSpaceRecord, KernelServiceStatus> {
        let Some(record) = self.handles.get(vm_space.raw) else {
            return Err(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::VmSpace || !record.has_rights(RIGHT_MAP) {
            return Err(KernelServiceStatus::AccessDenied);
        }
        self.vm_spaces
            .get(record.object_id)
            .ok_or(KernelServiceStatus::InvalidHandle)
    }

    pub fn bind_interrupt(
        &mut self,
        irq_number: u32,
        flags: u32,
    ) -> Result<Handle, KernelServiceStatus> {
        if self.current_hardware_access()? != HardwareAccess::Direct {
            return Err(KernelServiceStatus::AccessDenied);
        }
        let interrupt_id = self.interrupts.insert(irq_number, flags)?;
        self.handles.insert(
            interrupt_id,
            ObjectKind::Interrupt,
            RIGHT_READ
                | RIGHT_WRITE
                | RIGHT_SIGNAL
                | RIGHT_ADMIN
                | RIGHT_TRANSFER
                | super::RIGHT_DUPLICATE,
            0,
            None,
        )
    }

    pub fn deliver_interrupt(&mut self, irq_number: u32, now_ns: u64) -> InterruptDelivery {
        let delivery = self.interrupts.deliver(irq_number, now_ns);
        match delivery {
            InterruptDelivery::Signaled { interrupt_id }
            | InterruptDelivery::FloodLimited { interrupt_id } => {
                self.handles.add_signals_for_object(
                    interrupt_id,
                    ObjectKind::Interrupt,
                    super::SIGNAL_READABLE,
                )
            }
            InterruptDelivery::Masked | InterruptDelivery::NotBound => {}
        }
        delivery
    }

    pub fn acknowledge_interrupt(
        &mut self,
        interrupt_handle: Handle,
    ) -> Result<(), KernelServiceStatus> {
        let record = self
            .handles
            .get(interrupt_handle.raw)
            .filter(|record| record.kind == ObjectKind::Interrupt && record.has_rights(RIGHT_SIGNAL))
            .ok_or(KernelServiceStatus::AccessDenied)?;
        self.interrupts.acknowledge(record.object_id)?;
        self.handles.remove_signals_for_object(
            record.object_id,
            ObjectKind::Interrupt,
            super::SIGNAL_READABLE,
        );
        Ok(())
    }

    pub fn mask_interrupt(
        &mut self,
        interrupt_handle: Handle,
        masked: bool,
    ) -> Result<(), KernelServiceStatus> {
        let record = self
            .handles
            .get(interrupt_handle.raw)
            .filter(|record| record.kind == ObjectKind::Interrupt && record.has_rights(RIGHT_SIGNAL))
            .ok_or(KernelServiceStatus::AccessDenied)?;
        self.interrupts.mask(record.object_id, masked)
    }

    pub fn checkpoint_system_state(&self, target_vmo: Handle) -> CheckpointResult {
        let Some(record) = self.handles.get(target_vmo.raw) else {
            return checkpoint_status(KernelServiceStatus::InvalidHandle);
        };
        if record.kind != ObjectKind::Vmo || !record.has_rights(RIGHT_WRITE) {
            return checkpoint_status(KernelServiceStatus::AccessDenied);
        }
        CheckpointResult {
            status: KernelServiceStatus::Ok,
            serialized_bytes: ((self.handles.len()
                + self.vmos.len()
                + self.mappings.len()
                + self.threads.len()
                + self.processes.len()
                + self.vm_spaces.len()
                + self.interrupts.len()) as u64)
                * 32,
        }
    }

    pub fn current_hardware_access(&mut self) -> Result<HardwareAccess, KernelServiceStatus> {
        let (process_handle, _) = self.ensure_current_process()?;
        Ok(self.process_for_handle(process_handle)?.hardware_access)
    }
}

const RIGHT_MANAGE_PROCESS: u32 = RIGHT_ADMIN;

const fn checkpoint_status(status: KernelServiceStatus) -> CheckpointResult {
    CheckpointResult {
        status,
        serialized_bytes: 0,
    }
}
use alloc::vec::Vec;
