//! Four-CPU BexOS domain. Secure products permit entry only after real Trusty
//! rollback approval; explicit development fixtures use development evidence.
use crate::{memory::DomainMemory, platform::Platform};
use bexos_secure_monitor::{
    acpi,
    fabric::CpuState,
    image::Image,
    npt_tables::{PageSize, PageTables, Permissions, RamBank},
    svm::Vmcb,
    vcpu::{Registers, bexos_svm_enter},
};
// Contiguous RAM immediately above Trusty's bank. Keep the bank below the
// firmware's loader allocations near the top of Q35's low-memory aperture.
const BASE: u64 = 0x30000000;
const LENGTH: usize = 0x30000000;
#[path = "normal_state.rs"]
mod state;
pub use state::{Prepared, STATE_BYTES};
#[unsafe(link_section = ".resident.normal_npt")]
static mut TABLES: PageTables<16> = PageTables::empty();
#[unsafe(link_section = ".resident.normal_cpus")]
static mut CPUS: [Vmcb; 4] = [const { Vmcb::new() }; 4];
#[cfg(not(feature = "secure_product"))]
const KERNEL: &[u8] = include_bytes!(env!("NORMAL_KERNEL"));
#[cfg(not(feature = "secure_product"))]
const BOOTFS: &[u8] = include_bytes!(env!("NORMAL_BOOTFS"));
#[cfg(feature = "secure_product")]
use crate::approval::{bootfs, kernel};
#[cfg(not(feature = "secure_product"))]
fn kernel() -> &'static [u8] {
    KERNEL
}
#[cfg(not(feature = "secure_product"))]
fn bootfs() -> &'static [u8] {
    BOOTFS
}
#[cfg(not(feature = "secure_product"))]
const EVIDENCE: &[u8] = include_bytes!(env!("NORMAL_EVIDENCE"));
pub struct Normal {
    platform: Platform<4>,
    registers: [Registers; 4],
    root: u64,
    next: usize,
    entry_approved: bool,
    #[cfg(feature = "secure_product")]
    evidence_address: usize,
}
fn reject_layout() -> ! {
    crate::log("monitor-runtime: kernel load layout rejected; refusing execution\n");
    crate::halt()
}
impl Normal {
    pub unsafe fn initialize() -> Self {
        let ram = unsafe { core::slice::from_raw_parts_mut(BASE as *mut u8, LENGTH) };
        ram.fill(0);
        let image = Image::parse(kernel(), LENGTH).unwrap_or_else(|_| reject_layout());
        let handoff = crate::handoff::normal(bootfs().len());
        let word = |index: usize| {
            u64::from_le_bytes(handoff[index * 8..index * 8 + 8].try_into().unwrap())
        };
        assert_eq!(word(3), LENGTH as u64);
        assert_eq!(word(5), bootfs().len() as u64);
        assert_eq!(word(8), 4);
        let bootfs_address = usize::try_from(word(4)).unwrap();
        for (address, length) in [
            (0x01000000, 4096),
            (word(4), word(5)),
            (word(6), word(7)),
            (word(14), word(15)),
        ] {
            if image.reserve(address as usize, length as usize).is_err() {
                reject_layout();
            }
        }
        image.load(ram).unwrap_or_else(|_| reject_layout());
        ram[0x01000000..0x01000000 + handoff.len()].copy_from_slice(&handoff);
        if !unsafe {
            bexos_secure_monitor::fw_cfg::read(
                b"opt/bexos/normal-entropy",
                &mut ram[0x01000050..0x01000070],
            )
        } {
            crate::log("monitor-runtime: normal-world entropy unavailable; refusing execution\n");
            crate::halt();
        }
        ram[0x01000048..0x01000050].copy_from_slice(&1u64.to_le_bytes());
        ram[bootfs_address..bootfs_address + bootfs().len()].copy_from_slice(bootfs());
        #[cfg(not(feature = "secure_product"))]
        {
            let evidence = usize::try_from(word(14)).unwrap();
            ram[evidence..evidence + EVIDENCE.len()].copy_from_slice(EVIDENCE);
        }
        assert!(acpi::install(ram, 4));
        // Valid empty Multiboot2 information; the fixed BexOS descriptor owns
        // the usable RAM inventory and excludes all protected host memory.
        ram[0x9000..0x9010].copy_from_slice(&[16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0, 0]);
        let tables = unsafe { &mut *core::ptr::addr_of_mut!(TABLES) };
        tables
            .initialize(
                core::ptr::addr_of!(TABLES) as u64,
                RamBank {
                    start: BASE,
                    length: LENGTH as u64,
                },
            )
            .unwrap();
        for page in 0..LENGTH as u64 / 0x200000 {
            tables
                .map(
                    page * 0x200000,
                    BASE + page * 0x200000,
                    PageSize::Large,
                    Permissions::RAM,
                )
                .unwrap();
        }
        let root = tables.root().unwrap();
        let cpu = unsafe { &mut (*core::ptr::addr_of_mut!(CPUS))[0] };
        cpu.initialize_protected(2, root, u32::try_from(image.entry()).unwrap())
            .unwrap();
        Self::configure(cpu);
        cpu.set_rax(0x36d76289);
        let mut registers: [Registers; 4] = core::array::from_fn(|_| Registers::default());
        registers[0].rbx = 0x9000;
        crate::log("monitor-runtime: assigned four BexOS vCPUs in private NPT bank\n");
        let mut platform = Platform::new(
            DomainMemory {
                base: BASE,
                length: LENGTH,
            },
            false,
        );
        platform.devices.pci = Some(unsafe {
            crate::pci::Pci::initialize(RamBank {
                start: BASE,
                length: LENGTH as u64,
            })
        });
        Self {
            platform,
            registers,
            root,
            next: 0,
            entry_approved: !cfg!(feature = "secure_product"),
            #[cfg(feature = "secure_product")]
            evidence_address: word(14) as usize,
        }
    }
    #[cfg(feature = "secure_product")]
    pub unsafe fn approve_entry(
        &mut self,
        verified: bexos_secure_monitor::boot_verify::Verified,
        approval: bexos_trusty_boot::approval::Approval,
    ) {
        use sha2::{Digest, Sha256};
        assert!(!self.entry_approved);
        assert_eq!(verified.generation, approval.generation());
        assert_eq!(verified.rollback_location, approval.location());
        assert_eq!(
            <[u8; 32]>::from(Sha256::digest(kernel())),
            verified.kernel_sha256
        );
        assert_eq!(
            <[u8; 32]>::from(Sha256::digest(bootfs())),
            verified.bootfs_sha256
        );
        let mut evidence = bexos_boot::BootEvidenceV1::verified_monitor(
            verified.generation,
            verified.kernel_sha256,
            verified.bootfs_sha256,
            verified.policy_sha256,
            crate::firmware_generations::trusty_digest(),
            verified.root_sha256,
        );
        #[cfg(feature = "resident_nucleus")]
        {
            // product_recovery::select completed and discarded its restricted
            // Trusty instance before boot approval reaches this entry point.
            evidence.flags |= bexos_boot::BOOT_EVIDENCE_FLAG_FIRMWARE_SELECTION;
        }
        let mut bytes = [0; bexos_boot::BootEvidenceV1::BYTES];
        evidence.encode(&mut bytes);
        let address = self.evidence_address;
        assert!(
            address
                .checked_add(bytes.len())
                .is_some_and(|end| end <= LENGTH)
        );
        unsafe {
            let ram = core::slice::from_raw_parts_mut(BASE as *mut u8, LENGTH);
            ram[address..address + bytes.len()].copy_from_slice(&bytes);
            crate::transport::seal_boot_evidence(Sha256::digest(bytes).into());
        }
        self.entry_approved = true;
    }
    fn configure(cpu: &mut Vmcb) {
        cpu.set_permission_maps(
            core::ptr::addr_of!(crate::IOPM) as u64,
            core::ptr::addr_of!(crate::MSRPM) as u64,
        )
        .unwrap();
        cpu.intercept_cpuid();
        cpu.intercept_nmi();
    }
    pub unsafe fn step(&mut self) {
        assert!(
            self.entry_approved,
            "normal entry before Trusty boot approval"
        );
        for _ in 0..4 {
            let id = self.next;
            self.next = (self.next + 1) % 4;
            let cpu = unsafe { &mut (*core::ptr::addr_of_mut!(CPUS))[id] };
            match self.platform.devices.fabric.state(id).unwrap() {
                CpuState::WaitingForStartup => continue,
                CpuState::Start(vector) => {
                    cpu.initialize_startup(id as u32 + 2, self.root, vector)
                        .unwrap();
                    Self::configure(cpu);
                    self.platform.reset(id);
                    self.registers[id] = Registers::default();
                    assert!(self.platform.devices.fabric.started(id, vector));
                }
                CpuState::Running => {}
            }
            if !self
                .platform
                .before_entry(id, cpu, unsafe { bexos_secure_monitor::clock::now_ns() })
            {
                continue;
            }
            unsafe {
                #[cfg(feature = "monitor_transfer")]
                bexos_secure_monitor::watchdog::disarm_recovery();
                bexos_svm_enter(
                    cpu as *mut _ as u64,
                    &mut self.registers[id],
                    core::ptr::addr_of!(crate::HOST) as u64,
                );
            }
            self.platform.devices.after_exit(id, cpu);
            unsafe {
                bexos_secure_monitor::watchdog::service_pending();
            }
            if !unsafe { self.platform.exit(id, cpu, &mut self.registers[id]) } {
                crate::log("monitor-runtime: rejected BexOS cpu=");
                crate::hex(id as u64);
                crate::hex(cpu.exit_code());
                crate::hex(cpu.exit_info().0);
                crate::hex(cpu.exit_info().1);
                crate::hex(cpu.rip());
                crate::halt();
            }
            #[cfg(feature = "resident_nucleus")]
            crate::nucleus::completed_boundary(0);
            break;
        }
    }
}
