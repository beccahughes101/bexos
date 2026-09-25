use crate::ipc::{check, kernel_call};
use kernel_fidl::*;
pub struct Memory;
impl Memory {
    pub fn shared_device(base: u64, size: u64) -> Result<u64, Status> {
        let r: ObjectControlCreateSharedDeviceVmoResponse = kernel_call(
            5,
            "CreateSharedDeviceVmo",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlCreateSharedDeviceVmoRequest {
                base,
                size_bytes: size,
            },
        )?;
        check(r.status)?;
        Ok(r.vmo.raw)
    }
    pub fn shared_device_idle(handle: u64) -> Result<bool, Status> {
        let r: ObjectControlIsSharedDeviceVmoIdleResponse = kernel_call(
            5,
            "IsSharedDeviceVmoIdle",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlIsSharedDeviceVmoIdleRequest {
                vmo: HandleRef { raw: handle },
            },
        )?;
        check(r.status)?;
        Ok(r.idle)
    }
    pub fn channel_identity(handle: u64) -> Result<(u64, u64), Status> {
        let r: ObjectControlGetChannelIdentityResponse = kernel_call(
            5,
            "GetChannelIdentity",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlGetChannelIdentityRequest {
                channel: HandleRef { raw: handle },
            },
        )?;
        check(r.status)?;
        Ok((r.endpoint, r.peer))
    }
    pub fn object_info(handle: u64) -> Result<(ObjectType, u32), Status> {
        let r: ObjectControlGetInfoResponse = kernel_call(
            5,
            "GetInfo",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlGetInfoRequest {
                object: HandleRef { raw: handle },
            },
        )?;
        check(r.status)?;
        Ok((r.kind, r.rights.0))
    }

    pub fn create(size: u64, flags: u32) -> Result<u64, Status> {
        let r: VirtualMemoryCreateVmoResponse = kernel_call(
            2,
            "CreateVmo",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryCreateVmoRequest {
                size_bytes: size,
                flags: VmoFlags(flags),
            },
        )?;
        check(r.status)?;
        Ok(r.vmo.raw)
    }
    /// Creates a copy-on-write VMO snapshot of a page-aligned parent range.
    pub fn clone_vmo(parent: u64, offset: u64, size: u64) -> Result<u64, Status> {
        let r: VirtualMemoryCloneVmoResponse = kernel_call(
            2,
            "CloneVmo",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryCloneVmoRequest {
                parent_vmo: HandleRef { raw: parent },
                offset,
                size_bytes: size,
            },
        )?;
        check(r.status)?;
        Ok(r.cloned_vmo.raw)
    }
    pub fn map(h: u64, size: u64, rights: u32) -> Result<u64, Status> {
        Self::map_at(h, 0, size, 0, rights)
    }
    /// Maps a VMO range at an exact address, or lets the kernel choose when
    /// `target_vaddr` is zero.
    pub fn map_at(
        h: u64,
        vmo_offset: u64,
        size: u64,
        target_vaddr: u64,
        rights: u32,
    ) -> Result<u64, Status> {
        let size = bexos_boot::page_round(size).ok_or(Status::ErrInvalidArgs)?;
        let r: VirtualMemoryMapResponse = kernel_call(
            2,
            "Map",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryMapRequest {
                vmo: HandleRef { raw: h },
                vmo_offset,
                size_bytes: size,
                target_vaddr,
                requested_rights: Rights(rights),
            },
        )?;
        check(r.status)?;
        Ok(r.mapped_vaddr)
    }
    pub fn unmap(va: u64, size: u64) -> Result<(), Status> {
        let size = bexos_boot::page_round(size).ok_or(Status::ErrInvalidArgs)?;
        let r: VirtualMemoryUnmapResponse = kernel_call(
            2,
            "Unmap",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryUnmapRequest {
                vaddr: va,
                size_bytes: size,
            },
        )?;
        check(r.status)
    }
    pub fn create_sub_vmar(
        parent: u64,
        offset: u64,
        size: u64,
        flags: u32,
    ) -> Result<(u64, u64), Status> {
        let r: VirtualMemoryCreateSubVmarResponse = kernel_call(
            2,
            "CreateSubVmar",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryCreateSubVmarRequest {
                parent_vmar: HandleRef { raw: parent },
                offset,
                size_bytes: size,
                flags: VmarFlags(flags),
            },
        )?;
        check(r.status)?;
        Ok((r.sub_vmar.raw, r.base_address))
    }
    pub fn map_vmo(
        vmar: u64,
        vmo: u64,
        vmo_offset: u64,
        vmar_offset: u64,
        size: u64,
        flags: u32,
    ) -> Result<u64, Status> {
        let r: VirtualMemoryMapVmoResponse = kernel_call(
            2,
            "MapVmo",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryMapVmoRequest {
                vmar: HandleRef { raw: vmar },
                vmo: HandleRef { raw: vmo },
                vmo_offset,
                vmar_offset,
                size_bytes: size,
                flags: VmarFlags(flags),
            },
        )?;
        check(r.status)?;
        Ok(r.mapped_vaddr)
    }
    pub fn unmap_vmar(vmar: u64, va: u64, size: u64) -> Result<(), Status> {
        let r: VirtualMemoryUnmapVmarResponse = kernel_call(
            2,
            "UnmapVmar",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryUnmapVmarRequest {
                vmar: HandleRef { raw: vmar },
                vaddr: va,
                size_bytes: size,
            },
        )?;
        check(r.status)
    }
    pub fn destroy_vmar(vmar: u64) -> Result<(), Status> {
        let r: VirtualMemoryDestroyVmarResponse = kernel_call(
            2,
            "DestroyVmar",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryDestroyVmarRequest {
                vmar: HandleRef { raw: vmar },
            },
        )?;
        check(r.status)
    }
    pub fn commit_range(va: u64, size: u64) -> Result<(), Status> {
        let r: VirtualMemoryCommitRangeResponse = kernel_call(
            2,
            "CommitRange",
            VIRTUAL_MEMORY_PUBLIC_METHODS,
            &VirtualMemoryCommitRangeRequest {
                vaddr: va,
                size_bytes: size,
            },
        )?;
        check(r.status)
    }
    pub fn close(h: u64) -> Result<(), Status> {
        let r: ObjectControlCloseResponse = kernel_call(
            5,
            "Close",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlCloseRequest {
                object: HandleRef { raw: h },
            },
        )?;
        check(r.status)
    }
    pub fn duplicate(h: u64, rights: u32) -> Result<u64, Status> {
        let r: ObjectControlDuplicateResponse = kernel_call(
            5,
            "Duplicate",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlDuplicateRequest {
                object: HandleRef { raw: h },
                rights: Rights(rights),
            },
        )?;
        check(r.status)?;
        Ok(r.duplicate.raw)
    }
    pub fn physical(base: u64, size: u64) -> Result<u64, Status> {
        let r: ObjectControlCreatePhysicalVmoResponse = kernel_call(
            5,
            "CreatePhysicalVmo",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlCreatePhysicalVmoRequest {
                base,
                size_bytes: size,
            },
        )?;
        check(r.status)?;
        Ok(r.vmo.raw)
    }
    pub fn pin(h: u64) -> Result<(u64, u64), Status> {
        let r: ObjectControlPinResponse = kernel_call(
            5,
            "Pin",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlPinRequest {
                vmo: HandleRef { raw: h },
            },
        )?;
        check(r.status)?;
        Ok((r.physical_address, r.token))
    }
    pub fn unpin(token: u64) -> Result<(), Status> {
        let r: ObjectControlUnpinResponse = kernel_call(
            5,
            "Unpin",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlUnpinRequest { token },
        )?;
        check(r.status)
    }
    pub fn create_iommu_domain(stream_id: u64, address_width: u8) -> Result<u64, Status> {
        let r: ObjectControlCreateIommuDomainResponse = kernel_call(
            5,
            "CreateIommuDomain",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlCreateIommuDomainRequest {
                stream_id,
                address_width,
            },
        )?;
        check(r.status)?;
        Ok(r.domain.raw)
    }
    pub fn map_dma(
        domain: u64,
        vmo: u64,
        offset: u64,
        length: u64,
        permissions: u32,
    ) -> Result<(u64, u64), Status> {
        let r: ObjectControlMapDmaResponse = kernel_call(
            5,
            "MapDma",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlMapDmaRequest {
                domain: HandleRef { raw: domain },
                vmo: HandleRef { raw: vmo },
                offset,
                length,
                permissions,
            },
        )?;
        check(r.status)?;
        Ok((r.device_address, r.token))
    }
    pub fn unmap_dma(token: u64) -> Result<(), Status> {
        let r: ObjectControlUnmapDmaResponse = kernel_call(
            5,
            "UnmapDma",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlUnmapDmaRequest { token },
        )?;
        check(r.status)
    }
    pub fn stats() -> Result<ObjectControlMemoryStatsResponse, Status> {
        kernel_call(
            5,
            "MemoryStats",
            OBJECT_CONTROL_PUBLIC_METHODS,
            &ObjectControlMemoryStatsRequest {},
        )
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<u64, Status> {
        Self::from_bytes_with_flags(bytes, 0)
    }
    pub fn from_bytes_with_flags(bytes: &[u8], flags: u32) -> Result<u64, Status> {
        let size = bexos_boot::page_round(bytes.len() as u64).ok_or(Status::ErrInvalidArgs)?;
        let h = Self::create(size, flags)?;
        let va = Self::map(h, size, 6)?;
        if let Err(status) = Self::commit_range(va, size) {
            let _ = Self::unmap(va, size);
            let _ = Self::close(h);
            return Err(status);
        }
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), va as *mut u8, bytes.len());
        }
        Self::unmap(va, size)?;
        Ok(h)
    }
}
