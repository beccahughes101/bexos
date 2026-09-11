#![no_std]

//! Shared components for the integrated SVM monitor and its hardware probes.
//! Permanent execution ownership supports monitor policy replacement and
//! authenticated reboot recovery. Live Trusty replacement is excluded.

pub mod acpi;
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub mod clock;
pub mod cpu_policy;
pub mod fabric;
pub mod firmware_disk;
pub mod guest_memory;
pub mod hpet;
pub mod image;
pub mod ioapic;
pub mod lapic;
pub mod legacy_irq;
pub mod mmio;
pub mod monitor_image;
pub mod npt;
pub mod npt_tables;
pub mod policy_image;
pub mod replacement;
pub mod shared;
pub mod svm;
pub mod transport_lanes;
pub mod trial_rpmb;
pub mod vcpu;
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub mod watchdog;

pub mod run_state;

pub mod cmos;

pub mod dma_tables;
#[cfg(all(target_arch = "x86_64", target_os = "none"))]
pub mod iommu;
#[cfg(target_arch = "x86_64")]
pub mod pci_config;

pub mod pci_policy;

#[cfg(target_arch = "x86_64")]
pub mod fw_cfg;

pub mod boot_verify;
pub mod mailbox;
pub mod pci_state;
pub mod state_wire;
pub mod uart;
pub mod virtio_policy;
