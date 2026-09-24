//! Retain the device registry's capabilities while granting driver resources.
use crate::{HardwareAccessTier, HardwareResourceKind, HardwareResourceLease};
use alloc::vec::Vec;
use bexos_kernel_core::kernel_services::{
    RIGHT_DUPLICATE, RIGHT_MAP, RIGHT_READ, RIGHT_TRANSFER, RIGHT_WRITE,
};
use bexos_userspace::{HardwareResourceKind as StartupKind, StartupHardwareResource};
use kernel_fidl::Status;

pub fn duplicate_startup_resources(
    resources: &[HardwareResourceLease],
    access: HardwareAccessTier,
    mut duplicate: impl FnMut(u64, u32) -> Result<u64, Status>,
    mut close: impl FnMut(u64),
) -> Result<Vec<StartupHardwareResource>, Status> {
    let mut out: Vec<StartupHardwareResource> = Vec::new();
    for resource in resources {
        let allowed = match access {
            HardwareAccessTier::Direct => matches!(
                resource.kind,
                HardwareResourceKind::Mmio
                    | HardwareResourceKind::Interrupt
                    | HardwareResourceKind::DmaPool
                    | HardwareResourceKind::IommuDomain
                    | HardwareResourceKind::BusControl
            ),
            HardwareAccessTier::Isolated => matches!(
                resource.kind,
                HardwareResourceKind::DmaPool
                    | HardwareResourceKind::IommuDomain
                    | HardwareResourceKind::RegisterProxy
                    | HardwareResourceKind::BusControl
            ),
            HardwareAccessTier::None => false,
        };
        if !allowed {
            continue;
        }
        let (kind, extra_rights) = match resource.kind {
            HardwareResourceKind::Mmio => (StartupKind::Mmio, RIGHT_MAP),
            HardwareResourceKind::DmaPool => (StartupKind::DmaPool, RIGHT_MAP),
            HardwareResourceKind::IommuDomain => (StartupKind::IommuDomain, 0),
            HardwareResourceKind::Interrupt => (StartupKind::Interrupt, RIGHT_DUPLICATE),
            HardwareResourceKind::RegisterProxy => (StartupKind::RegisterProxy, RIGHT_DUPLICATE),
            HardwareResourceKind::BusControl => (StartupKind::BusControl, RIGHT_DUPLICATE),
        };
        let handle = match duplicate(
            resource.capability.object_id,
            RIGHT_TRANSFER | RIGHT_READ | RIGHT_WRITE | extra_rights,
        ) {
            Ok(handle) => handle,
            Err(error) => {
                for granted in out {
                    close(granted.handle);
                }
                return Err(error);
            }
        };
        out.push(StartupHardwareResource {
            kind,
            resource_id: resource.resource_id,
            base: resource.base,
            length: resource.length,
            flags: resource.flags,
            handle,
        });
    }
    Ok(out)
}
