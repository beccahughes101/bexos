//! One bounded device command in flight. Submission and completion never wait;
//! the compatibility request wrapper is used by bootstrap/legacy operations.
use crate::hardware::{Hardware, put32, put64};
use bexos_graphics_runtime::now_us;
use core::sync::atomic::{Ordering, fence};
use kernel_fidl::Status;
use virtio_drivers::transport::Transport;
pub(crate) struct PendingCommand {
    command: u32,
    expected: u32,
    fence: u64,
    index: u16,
    deadline: u64,
    context: u32,
    fenced: bool,
}
impl Hardware {
    pub(crate) fn submit_request(
        &mut self,
        command: u32,
        payload: &[u8],
        expected: u32,
    ) -> Result<u64, Status> {
        self.submit_context_request(command, payload, expected, 0, None)
    }
    pub(crate) fn submit_context_request(
        &mut self,
        command: u32,
        payload: &[u8],
        expected: u32,
        context: u32,
        ring: Option<u8>,
    ) -> Result<u64, Status> {
        if !self.healthy || payload.len() > 4000 {
            return Err(Status::ErrInvalidArgs);
        }
        if self.inflight.is_some() {
            return Err(Status::ErrResourceExhausted);
        }
        self.fence = self.fence.checked_add(1).ok_or(Status::ErrInvalidArgs)?;
        let addr = self.command.snapshot();
        let req = unsafe { self.command.bytes() };
        req.fill(0);
        put32(req, 0, command);
        // UNMAP_BLOB completion already acknowledges removal from the host
        // aperture. It needs no unrelated GL timeline fence. Vulkan submission
        // and display work continue to require their actual completion fences.
        let fenced = command != 0x209;
        put32(
            req,
            4,
            if !fenced {
                0
            } else if ring.is_some() {
                3
            } else {
                1
            },
        );
        put64(req, 8, if fenced { self.fence } else { 0 });
        put32(req, 16, context);
        req[20] = ring.unwrap_or(0);
        req[24..24 + payload.len()].copy_from_slice(payload);
        let q = unsafe { self.queue.bytes() };
        put64(q, 0, addr[0]);
        put32(q, 8, (24 + payload.len()) as u32);
        q[12..14].copy_from_slice(&1u16.to_le_bytes());
        q[14..16].copy_from_slice(&1u16.to_le_bytes());
        put64(q, 16, addr[0] + 4096);
        put32(q, 24, 4096);
        q[28..30].copy_from_slice(&2u16.to_le_bytes());
        unsafe {
            core::ptr::write_volatile(q.as_mut_ptr().add(4096) as *mut u16, 1);
            core::ptr::write_volatile(
                q.as_mut_ptr().add(4100 + (self.index as usize % 2) * 2) as *mut u16,
                0,
            );
        }
        fence(Ordering::SeqCst);
        self.index = self.index.wrapping_add(1);
        self.inflight = Some(PendingCommand {
            command,
            expected,
            fence: self.fence,
            index: self.index,
            deadline: now_us().saturating_add(1_000_000),
            context,
            fenced,
        });
        unsafe {
            core::ptr::write_volatile(q.as_mut_ptr().add(4098) as *mut u16, self.index.to_le());
        }
        fence(Ordering::SeqCst);
        self.transport.notify(0);
        Ok(self.fence)
    }
    pub(crate) fn poll_response(&mut self) -> Result<Option<usize>, Status> {
        let pending = self.inflight.as_ref().ok_or(Status::ErrInvalidArgs)?;
        let q = unsafe { self.queue.bytes() };
        let used =
            u16::from_le(unsafe { core::ptr::read_volatile(q.as_ptr().add(8194) as *const u16) });
        if used == pending.index.wrapping_sub(1) && now_us() < pending.deadline {
            return Ok(None);
        }
        let pending = self.inflight.take().unwrap();
        if used != pending.index {
            self.healthy = false;
            bexos_userspace::log(&format!(
                "virtio-gpu: command failed opcode={:#x} context={} fence={} used={} expected={}\n",
                pending.command, pending.context, pending.fence, used, pending.index
            ));
            return Err(if used == pending.index.wrapping_sub(1) {
                Status::ErrTimedOut
            } else {
                Status::ErrInvalidArgs
            });
        }
        fence(Ordering::SeqCst);
        let slot = 8196 + ((pending.index.wrapping_sub(1) as usize) % 2) * 8;
        let id =
            u32::from_le(unsafe { core::ptr::read_volatile(q.as_ptr().add(slot) as *const u32) });
        let len = u32::from_le(unsafe {
            core::ptr::read_volatile(q.as_ptr().add(slot + 4) as *const u32)
        }) as usize;
        if id != 0 || !(24..=4096).contains(&len) {
            self.healthy = false;
            return Err(Status::ErrInvalidArgs);
        }
        let response = unsafe { self.command.bytes() };
        let mut header = [0; 24];
        for (i, byte) in header.iter_mut().enumerate() {
            *byte = unsafe { core::ptr::read_volatile(response.as_ptr().add(4096 + i)) };
        }
        let response_type = u32::from_le_bytes(header[..4].try_into().unwrap());
        let rejected = (0x1200..=0x1205).contains(&response_type);
        let expected = if rejected {
            response_type
        } else {
            pending.expected
        };
        let completed = if pending.fenced {
            bexos_virtio_gpu_protocol::completion_for_context(
                &header,
                expected,
                pending.fence,
                pending.context,
            )
        } else {
            bexos_virtio_gpu_protocol::control_acknowledgement(&header, expected)
        };
        if completed.is_err() {
            self.healthy = false;
            return Err(Status::ErrInvalidArgs);
        }
        // A valid device rejection consumes the command. It does not corrupt
        // the queue or justify resetting the currently visible scanout.
        if rejected {
            return Err(Status::ErrInvalidArgs);
        }
        Ok(Some(len))
    }
    pub(crate) fn request_response(
        &mut self,
        command: u32,
        payload: &[u8],
        expected: u32,
    ) -> Result<usize, Status> {
        self.submit_request(command, payload, expected)?;
        loop {
            if let Some(len) = self.poll_response()? {
                return Ok(len);
            }
            bexos_userspace::yield_now();
        }
    }
}
