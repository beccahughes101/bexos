//! Bounded transfer/scanout/flush pipeline. DMA remains owned until completion;
//! no command wait or scheduler yield occurs in submission/completion handling.
use crate::hardware::{Hardware, put32, put64};
use bexos_graphics::{Damage, Surface};
use kernel_fidl::Status;
pub(crate) fn poll(display: &mut crate::state::Display) {
    let Some((client, generation)) = display.pending_reply else {
        return;
    };
    let result = display
        .hardware
        .as_mut()
        .ok_or(Status::ErrInvalidArgs)
        .and_then(|h| h.poll_present());
    crate::scanout::progress(display, result.is_err());
    let (status, fence) = match result {
        Ok(None) => return,
        Ok(Some(fence)) => {
            if display.ownership.pending.is_some() {
                let _ = display.ownership.complete(client, generation);
                bexos_userspace::log("virtio-gpu: first compositor frame equals splash\n");
            }
            if display.resumed {
                bexos_userspace::log(&format!(
                    "virtio-gpu: post-transplant presentation fence={fence}\n"
                ));
                display.resumed = false;
            }
            (graphics_fidl::Status::Ok, fence)
        }
        Err(_) => (graphics_fidl::Status::ErrIo, 0),
    };
    display.pending_reply = None;
    bexos_graphics_runtime::reply(
        bexos_userspace::Channel(client),
        &graphics_fidl::DisplayCoordinatorPresentResponse { status, fence },
    );
}
#[derive(Clone, Copy)]
enum Phase {
    Transfer,
    Scanout,
    Flush,
}
pub(crate) struct PresentFlight {
    phase: Phase,
    resource: u32,
    staging: Option<u32>,
    damage: Damage,
}
impl Hardware {
    pub fn begin_present(
        &mut self,
        bytes: &[u8],
        surface: Surface,
        damage: Damage,
    ) -> Result<(), Status> {
        if self.presenting.is_some() || self.inflight.is_some() {
            return Err(Status::ErrResourceExhausted);
        }
        if !self.healthy {
            return Err(Status::ErrInvalidArgs);
        }
        surface
            .validate(bytes.len() as u64)
            .map_err(|_| Status::ErrInvalidArgs)?;
        damage
            .validate(surface)
            .map_err(|_| Status::ErrInvalidArgs)?;
        if surface.width != self.surface.width || surface.height != self.surface.height {
            return Err(Status::ErrInvalidArgs);
        }
        // The staging cache predates an imported scanout. Rebuild from the
        // caller's complete canvas when transitioning back to composition.
        let damage = if self.scanout_resource > 2 {
            self.surface.full()
        } else {
            damage
        };
        let next = 1 - self.front;
        let (a, b) = self.frames.split_at_mut(1);
        let (front, back) = if self.front == 0 {
            (&mut a[0], &mut b[0])
        } else {
            (&mut b[0], &mut a[0])
        };
        let dirty = bexos_graphics::buffer::update_tracked_back_buffer(
            self.surface,
            unsafe { front.bytes() },
            unsafe { back.bytes() },
            surface,
            bytes,
            self.stale[next as usize],
            damage,
        )
        .map_err(|_| Status::ErrInvalidArgs)?;
        let mut rect = [0; 32];
        put32(&mut rect, 0, dirty.x);
        put32(&mut rect, 4, dirty.y);
        put32(&mut rect, 8, dirty.width);
        put32(&mut rect, 12, dirty.height);
        put64(
            &mut rect,
            16,
            (dirty.y * self.surface.stride + dirty.x * 4) as u64,
        );
        put32(&mut rect, 24, next + 1);
        self.submit_request(0x105, &rect, 0x1100)?;
        self.presenting = Some(PresentFlight {
            phase: Phase::Transfer,
            resource: next + 1,
            staging: Some(next),
            damage,
        });
        Ok(())
    }
    pub fn begin_imported(
        &mut self,
        resource: u32,
        surface: Surface,
        damage: Damage,
    ) -> Result<(), Status> {
        if self.presenting.is_some() || self.inflight.is_some() {
            return Err(Status::ErrResourceExhausted);
        }
        if !self.healthy
            || resource < 3
            || crate::scanout_state::import_size(surface, self.surface).is_none()
        {
            return Err(Status::ErrInvalidArgs);
        }
        damage
            .validate(surface)
            .map_err(|_| Status::ErrInvalidArgs)?;
        let mut rect = [0; 32];
        put32(&mut rect, 0, damage.x);
        put32(&mut rect, 4, damage.y);
        put32(&mut rect, 8, damage.width);
        put32(&mut rect, 12, damage.height);
        put64(
            &mut rect,
            16,
            (damage.y * surface.stride + damage.x * 4) as u64,
        );
        put32(&mut rect, 24, resource);
        self.submit_request(0x105, &rect, 0x1100)?;
        self.presenting = Some(PresentFlight {
            phase: Phase::Transfer,
            resource,
            staging: None,
            damage,
        });
        Ok(())
    }
    pub fn poll_present(&mut self) -> Result<Option<u64>, Status> {
        if self.presenting.is_none() {
            return Err(Status::ErrInvalidArgs);
        }
        match self.poll_response() {
            Ok(None) => return Ok(None),
            Err(error) => {
                self.presenting = None;
                return Err(error);
            }
            Ok(Some(_)) => {}
        }
        let mut frame = self.presenting.take().unwrap();
        let mut payload = [0; 24];
        put32(&mut payload, 8, self.surface.width);
        put32(&mut payload, 12, self.surface.height);
        match frame.phase {
            Phase::Transfer => {
                put32(&mut payload, 20, frame.resource);
                self.submit_request(0x103, &payload, 0x1100)?;
                frame.phase = Phase::Scanout;
            }
            Phase::Scanout => {
                self.scanout_resource = frame.resource;
                if let Some(next) = frame.staging {
                    self.stale[next as usize] = None;
                    self.stale[self.front as usize] = Some(
                        self.stale[self.front as usize]
                            .map_or(frame.damage, |d| d.union(frame.damage)),
                    );
                    self.front = next;
                } else {
                    self.stale = [Some(self.surface.full()); 2];
                }

                put32(&mut payload, 16, frame.resource);
                self.submit_request(0x104, &payload, 0x1100)?;
                frame.phase = Phase::Flush;
            }
            Phase::Flush => {
                return Ok(Some(self.fence));
            }
        }
        self.presenting = Some(frame);
        Ok(None)
    }
}
