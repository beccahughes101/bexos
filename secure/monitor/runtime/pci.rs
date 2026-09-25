//! Fixed Q35 polling-device assignment. VT-d owns DMA translations and rejects
//! all physical MSI delivery; INTx is disabled. Guests see only assigned PCI
//! functions and validated BAR windows, never host bridge/IOMMU controls.
use bexos_secure_monitor::{
    dma_tables::DmaTables, iommu, npt_tables::RamBank, pci_config, pci_policy::Bar,
    pci_state::Function,
};
#[path = "pci_state.rs"]
mod state;
#[unsafe(link_section = ".resident.dma")]
static mut TABLES: DmaTables = DmaTables::empty();
#[cfg(not(feature = "secure_product"))]
const REQUESTERS: [u8; 3] = [24, 32, 40];
#[cfg(not(feature = "secure_product"))]
const IDENTITIES: [u32; 3] = [0x00101b36, 0x10431af4, 0x10411af4];
#[cfg(feature = "secure_product")]
const REQUESTERS: [u8; 8] = [24, 32, 40, 48, 56, 64, 72, 80];
#[cfg(feature = "secure_product")]
const IDENTITIES: [u32; 8] = [
    0x00101b36, 0x10431af4, 0x10411af4, 0x10431af4, 0x10501af4, 0x10521af4, 0x10521af4, 0x10d38086,
];
#[derive(Clone)]
pub struct Pci {
    functions: [Function; REQUESTERS.len()],
}
impl Pci {
    pub unsafe fn initialize(bank: RamBank) -> Self {
        unsafe {
            assert!(pci_config::quiesce_root_bus());
            let tables = &mut *core::ptr::addr_of_mut!(TABLES);
            tables
                .initialize(tables as *const _ as u64, bank, &REQUESTERS)
                .unwrap();
            assert!(iommu::initialize(tables));
            let mut result = Self {
                functions: [Function::default(); REQUESTERS.len()],
            };
            for (device, expected) in IDENTITIES.into_iter().enumerate() {
                let requester = REQUESTERS[device];
                let identity = pci_config::read(requester, 0);
                // Product headless boots omit the optional workstation devices
                // in slots 7-9 and the SDK e1000e fixture in slot 10. Their
                // protected policy remains empty, while a populated slot must
                // expose exactly the identity authenticated for that requester.
                if cfg!(feature = "secure_product") && device >= 4 && identity == u32::MAX {
                    continue;
                }
                assert_eq!(identity, expected);
                pci_config::write(requester, 4, 0x400);
                let mut index = 0;
                while index < 6 {
                    let register = 0x10 + index as u8 * 4;
                    let flags = pci_config::read(requester, register) as u8 & 15;
                    // Legacy I/O BARs are never exposed to the guest. The
                    // virtual command register forbids I/O decode, while MMIO
                    // BARs continue through the size and protected-window
                    // checks below.
                    if flags & 1 != 0 {
                        index += 1;
                        continue;
                    }
                    let wide = flags & 6 == 4;
                    assert!(!wide || index < 5);
                    pci_config::write(requester, register, u32::MAX);
                    if wide {
                        pci_config::write(requester, register + 4, u32::MAX);
                    }
                    let low = pci_config::read(requester, register) & !15;
                    let high = if wide {
                        pci_config::read(requester, register + 4)
                    } else {
                        u32::MAX
                    };
                    pci_config::write(requester, register, 0);
                    if wide {
                        pci_config::write(requester, register + 4, 0);
                    }
                    if low != 0 {
                        let size = (!((u64::from(high) << 32) | u64::from(low))).wrapping_add(1);
                        result.functions[device].bars[index] = Bar::new(size, flags).unwrap();
                    }
                    index += if wide { 2 } else { 1 };
                }
                if device != 0 {
                    let mut cap = pci_config::read(requester, 0x34) as u8;
                    for _ in 0..48 {
                        if cap == 0 {
                            break;
                        }
                        assert!(cap >= 0x40 && cap & 3 == 0);
                        let header = pci_config::read(requester, cap);
                        if header as u8 == 9 && header >> 24 == 1 {
                            assert!((header >> 16) as u8 >= 16);
                            let bar = pci_config::read(requester, cap + 4) as u8;
                            assert!(
                                bar < 6
                                    && result.functions[device].bars[usize::from(bar)].length != 0
                            );
                            result.functions[device].common_bar = Some(bar);
                            result.functions[device].common_offset =
                                u64::from(pci_config::read(requester, cap + 8));
                        }
                        cap = (header >> 8) as u8;
                    }
                    // Virtio PCI functions must expose the modern common
                    // configuration capability. The polling e1000e acceptance
                    // fixture is deliberately non-virtio and remains confined
                    // by the same BAR and VT-d policy.
                    if expected as u16 == 0x1af4 {
                        assert!(result.functions[device].common_bar.is_some());
                    }
                }
            }
            result.retain_policy();
            crate::log("monitor-runtime: assigned PCI DMA confined; physical interrupts denied\n");
            result
        }
    }
    fn bar_index(function: &Function, index: usize) -> (usize, bool) {
        if index > 0 && function.bars[index - 1].wide() {
            (index - 1, true)
        } else {
            (index, false)
        }
    }
    pub unsafe fn read_config(&self, address: u64, bytes: u8) -> Option<u64> {
        if !(0xe0000000..0xe0100000).contains(&address)
            || !matches!(bytes, 1 | 2 | 4)
            || address & (u64::from(bytes) - 1) != 0
        {
            return None;
        }
        let offset = address - 0xe0000000;
        let requester = (offset >> 12) as u8;
        let register = (offset & 4095) as u16;
        let Some(device) = REQUESTERS.iter().position(|id| *id == requester) else {
            return Some(u64::MAX);
        };
        if register >= 256 {
            return Some(0);
        }
        let function = &self.functions[device];
        let value = if (0x10..0x28).contains(&register) {
            let (index, high) = Self::bar_index(function, (usize::from(register) - 0x10) / 4);
            let bar = function.bars[index];
            if bar.length == 0 { 0 } else { bar.read(high) }
        } else if register & !3 == 4 {
            u32::from(function.command | 0x400)
                | (unsafe { pci_config::read(requester, 4) } & 0xffff0000)
        } else {
            unsafe { pci_config::read(requester, register as u8) }
        };
        Some(u64::from(value >> ((register & 3) * 8)))
    }
    pub unsafe fn write_config(&mut self, address: u64, bytes: u8, value: u64) -> bool {
        if !(0xe0000000..0xe0100000).contains(&address) {
            return false;
        }
        let offset = address - 0xe0000000;
        let requester = (offset >> 12) as u8;
        let register = (offset & 4095) as u16;
        let Some(device) = REQUESTERS.iter().position(|id| *id == requester) else {
            return false;
        };
        if register == 4 && matches!(bytes, 2 | 4) {
            if value & !0x406 != 0 {
                return false;
            }
            if value & 6 != 0
                && self.functions[device]
                    .bars
                    .iter()
                    .any(|bar| bar.length != 0 && bar.base == 0)
            {
                return false;
            }
            self.functions[device].command = value as u16 | 0x400;
            unsafe {
                pci_config::write(requester, 4, value as u32 | 0x400);
            }
            return true;
        }
        if !(0x10..0x28).contains(&register) || register & 3 != 0 || bytes != 4 {
            return false;
        }
        let (index, high) =
            Self::bar_index(&self.functions[device], (usize::from(register) - 0x10) / 4);
        let old = self.functions[device].bars[index];
        if old.length == 0 {
            return value == 0 || value == u32::MAX as u64;
        }
        let Some(bar) = old.changed(high, value as u32) else {
            return false;
        };
        for (other_device, function) in self.functions.iter().enumerate() {
            for (other_index, other) in function.bars.iter().enumerate() {
                if (other_device, other_index) != (device, index) && bar.overlaps(*other) {
                    return false;
                }
            }
        }
        self.functions[device].bars[index] = bar;
        if value as u32 != u32::MAX {
            unsafe {
                pci_config::write(requester, 4, 0x400);
                pci_config::write(requester, 0x10 + index as u8 * 4, bar.base as u32);
                if bar.wide() {
                    pci_config::write(requester, 0x14 + index as u8 * 4, (bar.base >> 32) as u32);
                }
                if self.functions[device]
                    .bars
                    .iter()
                    .all(|bar| bar.length == 0 || bar.base != 0)
                {
                    pci_config::write(
                        requester,
                        4,
                        u32::from(self.functions[device].command | 0x400),
                    );
                }
            }
        }
        true
    }
    fn locate(&self, address: u64, bytes: u8) -> Option<(usize, usize)> {
        self.functions
            .iter()
            .enumerate()
            .find_map(|(device, function)| {
                if function.command & 2 == 0 {
                    return None;
                }
                function
                    .bars
                    .iter()
                    .position(|bar| bar.contains(address, bytes))
                    .map(|bar| (device, bar))
            })
    }
    pub unsafe fn read(&self, address: u64, bytes: u8) -> Option<u64> {
        self.locate(address, bytes)?;
        Some(unsafe {
            match bytes {
                1 => u64::from(core::ptr::read_volatile(address as *const u8)),
                2 => u64::from(core::ptr::read_volatile(address as *const u16)),
                4 => u64::from(core::ptr::read_volatile(address as *const u32)),
                8 => core::ptr::read_volatile(address as *const u64),
                _ => return None,
            }
        })
    }
    pub unsafe fn write(&mut self, address: u64, bytes: u8, value: u64) -> bool {
        let Some((device, bar)) = self.locate(address, bytes) else {
            return false;
        };
        let function = &mut self.functions[device];
        if let Some(common) = function.common_bar {
            let base = function.bars[usize::from(common)].base + function.common_offset;
            if bar == usize::from(common) && address >= base && address < base + 64 {
                if !function.features.write(address - base, bytes, value) {
                    return false;
                }
            } else if !function.features.ready() {
                return false;
            }
        }
        unsafe {
            match bytes {
                1 => core::ptr::write_volatile(address as *mut u8, value as u8),
                2 => core::ptr::write_volatile(address as *mut u16, value as u16),
                4 => core::ptr::write_volatile(address as *mut u32, value as u32),
                8 => core::ptr::write_volatile(address as *mut u64, value),
                _ => return false,
            }
        }
        true
    }
}
