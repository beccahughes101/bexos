//! Polling NVMe backend. Queue and payload memory is genuinely pinned and
//! shared with the emulated controller; completions are never synthesized.
mod migration;
use crate::namespace::{NamespaceInfo, parse_identify_namespace};
use bexos_d1_linux_shim::bexos::MappedMmio;
use bexos_d1_linux_shim::mmio::Mmio;
use bexos_userspace::Memory;
use core::time::Duration;
use kernel_fidl::Status;
const DEPTH: u16 = 16;
// QEMU's NVMe controller advertises MDTS=7 with 4 KiB pages, allowing
// transfers up to 512 KiB. Using the advertised limit keeps large namespace
// reads from paying one IPC and queue-completion round trip per 64 KiB.
const MAX_TRANSFER_BYTES: usize = 512 * 1024;
const PAGE_BYTES: usize = 4096;
pub struct Dma {
    handle: u64,
    va: u64,
    pa: u64,
    token: u64,
    size: u64,
    owned: bool,
}
impl Dma {
    fn new(domain: u64, size: usize) -> Result<Self, Status> {
        if size == 0 || !size.is_multiple_of(PAGE_BYTES) {
            return Err(Status::ErrInvalidArgs);
        }
        let size = size as u64;
        let handle = Memory::create(size, 2)?;
        let va = match Memory::map(handle, size, 6) {
            Ok(va) => va,
            Err(error) => {
                let _ = Memory::close(handle);
                return Err(error);
            }
        };
        let (pa, token) = match Memory::map_dma(domain, handle, 0, size, 2 | 4) {
            Ok(pin) => pin,
            Err(error) => {
                let _ = Memory::unmap(va, size);
                let _ = Memory::close(handle);
                return Err(error);
            }
        };
        unsafe {
            core::ptr::write_bytes(va as *mut u8, 0, size as usize);
        }
        Ok(Self {
            handle,
            va,
            pa,
            token,
            size,
            owned: true,
        })
    }

    fn adopt(handle: u64, va: u64, pa: u64, token: u64, size: u64) -> Result<Self, Status> {
        if handle == 0
            || token == 0
            || size == 0
            || !size.is_multiple_of(PAGE_BYTES as u64)
            || !va.is_multiple_of(PAGE_BYTES as u64)
            || !pa.is_multiple_of(PAGE_BYTES as u64)
        {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(Self {
            handle,
            va,
            pa,
            token,
            size,
            owned: false,
        })
    }

    fn handle(&self) -> u64 {
        self.handle
    }

    fn va(&self) -> u64 {
        self.va
    }

    fn pa(&self) -> u64 {
        self.pa
    }

    fn token(&self) -> u64 {
        self.token
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn activate(&mut self) {
        self.owned = true;
    }
}
impl Drop for Dma {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let _ = Memory::unmap_dma(self.token);
        let _ = Memory::unmap(self.va, self.size);
        let _ = Memory::close(self.handle);
    }
}
pub struct Queue {
    sq: Dma,
    cq: Dma,
    tail: u16,
    head: u16,
    phase: u16,
    cid: u16,
    id: u16,
}
impl Hardware {
    pub fn queue_addresses(&self) -> (u64, u64, u64) {
        (self.io.sq.pa(), self.io.cq.pa(), self.payload.pa())
    }
}
impl Queue {
    fn new(domain: u64, id: u16) -> Result<Self, Status> {
        Ok(Self {
            sq: Dma::new(domain, PAGE_BYTES)?,
            cq: Dma::new(domain, PAGE_BYTES)?,
            tail: 0,
            head: 0,
            phase: 1,
            cid: 0,
            id,
        })
    }
    async fn command(
        &mut self,
        mmio: &mut MappedMmio,
        stride: u64,
        mut words: [u32; 16],
    ) -> Result<(), Status> {
        self.cid = self.cid.wrapping_add(1);
        words[0] |= (self.cid as u32) << 16;
        unsafe {
            for (i, w) in words.iter().enumerate() {
                core::ptr::write_volatile(
                    (self.sq.va() + self.tail as u64 * 64 + i as u64 * 4) as *mut u32,
                    *w,
                );
            }
        }
        barrier();
        self.tail = (self.tail + 1) % DEPTH;
        mmio.write32(
            0x1000 + (2 * self.id) as usize * stride as usize,
            self.tail as u32,
        )
        .map_err(linux_error_status)?;
        bexos_d1_linux_shim::task::wait_until(Duration::from_secs(5), || {
            let pos = self.cq.va() + self.head as u64 * 16;
            let last = read32(pos + 12);
            if (last >> 16) & 1 == self.phase as u32 {
                barrier();
                if last as u16 != self.cid {
                    return Err(bexos_d1_linux_shim::LinuxError::Invalid);
                }
                let status = (last >> 17) & 0x7ff;
                self.head = (self.head + 1) % DEPTH;
                if self.head == 0 {
                    self.phase ^= 1;
                }
                mmio.write32(
                    0x1000 + (2 * self.id + 1) as usize * stride as usize,
                    self.head as u32,
                )?;
                if status != 0 {
                    return Err(bexos_d1_linux_shim::LinuxError::Invalid);
                }
                return Ok(true);
            }
            Ok(false)
        })
        .await
        .map_err(linux_error_status)
    }
}
pub struct Hardware {
    mmio: MappedMmio,
    stride: u64,
    admin: Queue,
    io: Queue,
    payload: Dma,
    prp_list: Dma,
    iommu_domain: u64,
    pub info: NamespaceInfo,
    healthy: bool,
}
impl Hardware {
    pub async fn connect(mmio: u64, iommu_domain: u64) -> Result<Self, Status> {
        let mut mmio = MappedMmio::new(mmio);
        let cap = mmio.read64(0).map_err(linux_error_status)?;
        if cap & 0xffff < (DEPTH - 1) as u64 || (cap >> 48) & 15 != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        let stride = 4u64 << ((cap >> 32) & 15);
        mmio.write32(0x14, 0).map_err(linux_error_status)?;
        wait_ready(&mmio, false).await?;
        let admin = Queue::new(iommu_domain, 0)?;
        let io = Queue::new(iommu_domain, 1)?;
        let payload = Dma::new(iommu_domain, MAX_TRANSFER_BYTES)?;
        let prp_list = Dma::new(iommu_domain, PAGE_BYTES)?;
        mmio.write32(0x24, (DEPTH as u32 - 1) | ((DEPTH as u32 - 1) << 16))
            .map_err(linux_error_status)?;
        mmio.write64(0x28, admin.sq.pa())
            .map_err(linux_error_status)?;
        mmio.write64(0x30, admin.cq.pa())
            .map_err(linux_error_status)?;
        barrier();
        mmio.write32(0x14, 1 | (6 << 16) | (4 << 20))
            .map_err(linux_error_status)?;
        wait_ready(&mmio, true).await?;
        let mut hw = Self {
            mmio,
            stride,
            admin,
            io,
            payload,
            prp_list,
            iommu_domain,
            healthy: true,
            info: NamespaceInfo {
                namespace_id: 1,
                block_size: 512,
                block_count: 0,
                max_transfer_blocks: 8,
            },
        };
        let mut identify = [0; 16];
        identify[0] = 6;
        identify[6] = hw.payload.pa() as u32;
        identify[7] = (hw.payload.pa() >> 32) as u32;
        identify[10] = 1;
        hw.admin.command(&mut hw.mmio, stride, identify).await?;
        let controller =
            unsafe { core::slice::from_raw_parts(hw.payload.va() as *const u8, PAGE_BYTES) };
        let max_transfer_blocks = controller_max_transfer_blocks(controller[77]);
        identify[1] = 1;
        identify[10] = 0;
        hw.admin.command(&mut hw.mmio, stride, identify).await?;
        let data = unsafe { core::slice::from_raw_parts(hw.payload.va() as *const u8, 4096) };
        hw.info = parse_identify_namespace(1, data, max_transfer_blocks)
            .map_err(|_| Status::ErrInvalidArgs)?;
        if hw.info.block_size != 512 || hw.info.block_count == 0 {
            return Err(Status::ErrInvalidArgs);
        }
        let mut cq = [0; 16];
        cq[0] = 5;
        cq[6] = hw.io.cq.pa() as u32;
        cq[7] = (hw.io.cq.pa() >> 32) as u32;
        cq[10] = 1 | ((DEPTH as u32 - 1) << 16);
        cq[11] = 1;
        hw.admin.command(&mut hw.mmio, stride, cq).await?;
        let mut sq = cq;
        sq[0] = 1;
        sq[6] = hw.io.sq.pa() as u32;
        sq[7] = (hw.io.sq.pa() >> 32) as u32;
        sq[11] = 1 | (1 << 16);
        hw.admin.command(&mut hw.mmio, stride, sq).await?;
        Ok(hw)
    }
    pub async fn transfer(&mut self, opcode: u8, lba: u64, bytes: &mut [u8]) -> Result<(), Status> {
        if opcode != 1 && opcode != 2 {
            return Err(Status::ErrInvalidArgs);
        }
        if bytes.is_empty() || bytes.len() > MAX_TRANSFER_BYTES || bytes.len() % 512 != 0 {
            return Err(Status::ErrInvalidArgs);
        }
        if lba
            .checked_add(bytes.len() as u64 / 512)
            .is_none_or(|n| n > self.info.block_count)
        {
            return Err(Status::ErrInvalidArgs);
        }

        self.transfer_payload(opcode, lba, bytes).await
    }

    async fn transfer_payload(
        &mut self,
        opcode: u8,
        lba: u64,
        bytes: &mut [u8],
    ) -> Result<(), Status> {
        if opcode == 1 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    self.payload.va() as *mut u8,
                    bytes.len(),
                );
            }
        }
        let mut cmd = [0; 16];
        cmd[0] = opcode as u32;
        cmd[1] = 1;
        if opcode != 0 {
            cmd[6] = self.payload.pa() as u32;
            cmd[7] = (self.payload.pa() >> 32) as u32;
            let prp2 = if bytes.len() <= PAGE_BYTES {
                0
            } else if bytes.len() <= PAGE_BYTES * 2 {
                self.payload.pa() + PAGE_BYTES as u64
            } else {
                let pages = bytes.len().div_ceil(PAGE_BYTES);
                for index in 1..pages {
                    unsafe {
                        ((self.prp_list.va() as *mut u64).add(index - 1))
                            .write_volatile(self.payload.pa() + (index * PAGE_BYTES) as u64);
                    }
                }
                barrier();
                self.prp_list.pa()
            };
            cmd[8] = prp2 as u32;
            cmd[9] = (prp2 >> 32) as u32;
            cmd[10] = lba as u32;
            cmd[11] = (lba >> 32) as u32;
            cmd[12] = bytes.len() as u32 / 512 - 1;
        }
        if let Err(e) = self.io.command(&mut self.mmio, self.stride, cmd).await {
            self.healthy = false;
            return Err(e);
        }
        if opcode == 2 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.payload.va() as *const u8,
                    bytes.as_mut_ptr(),
                    bytes.len(),
                );
            }
        }
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<(), Status> {
        Ok(())
    }

    pub async fn power_down(&mut self) -> Result<(), Status> {
        let _ = self.flush().await;
        self.mmio.write32(0x14, 0).map_err(linux_error_status)?;
        wait_ready(&self.mmio, false).await?;
        self.healthy = false;
        Ok(())
    }

    pub async fn power_up(&mut self) -> Result<(), Status> {
        if self.healthy {
            return Ok(());
        }
        let replacement = Self::connect(self.mmio.base(), self.iommu_domain).await?;
        *self = replacement;
        Ok(())
    }
}

fn controller_max_transfer_blocks(mdts: u8) -> u32 {
    let bytes = if mdts == 0 {
        MAX_TRANSFER_BYTES
    } else {
        PAGE_BYTES
            .checked_shl(u32::from(mdts))
            .unwrap_or(MAX_TRANSFER_BYTES)
            .min(MAX_TRANSFER_BYTES)
    };
    (bytes / 512) as u32
}

fn read32(p: u64) -> u32 {
    unsafe { core::ptr::read_volatile(p as *const u32) }
}
fn barrier() {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("dsb sy", options(nostack));
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}
async fn wait_ready(mmio: &MappedMmio, ready: bool) -> Result<(), Status> {
    bexos_d1_linux_shim::task::wait_until(Duration::from_secs(5), || {
        let status = mmio.read32(0x1c)?;
        if status & 2 != 0 {
            return Err(bexos_d1_linux_shim::LinuxError::Invalid);
        }
        if (status & 1 != 0) == ready {
            return Ok(true);
        }
        Ok(false)
    })
    .await
    .map_err(linux_error_status)
}

fn linux_error_status(error: bexos_d1_linux_shim::LinuxError) -> Status {
    match error {
        bexos_d1_linux_shim::LinuxError::Invalid => Status::ErrInvalidArgs,
        bexos_d1_linux_shim::LinuxError::NoMemory => Status::ErrNoMemory,
        bexos_d1_linux_shim::LinuxError::Timeout => Status::ErrTimedOut,
        bexos_d1_linux_shim::LinuxError::Io => Status::ErrInvalidHandle,
        bexos_d1_linux_shim::LinuxError::Unsupported => Status::ErrInvalidArgs,
    }
}
