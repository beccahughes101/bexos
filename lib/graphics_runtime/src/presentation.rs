//! Nonblocking legacy display submission. A caller must keep the canvas output
//! unchanged and issue no other display RPC until this submission completes.
use crate::{canvas::Canvas, now_us, stream::read_no_handles, wire};
use bexos_userspace::Memory;
use graphics_fidl::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Submission {
    pub deadline_us: u64,
}
impl Submission {
    pub fn begin(canvas: &Canvas, damage: bexos_graphics::Damage) -> Result<Self, Status> {
        damage
            .validate(canvas.surface)
            .map_err(|_| Status::ErrInvalidArgs)?;
        if canvas.display.0 == 0 {
            return Err(Status::ErrIo);
        }
        let handle =
            Memory::duplicate(canvas.output.handle, 1 | 2 | 16 | 32).map_err(|_| Status::ErrIo)?;
        let mut bytes = [0; 128];
        bytes[..8].copy_from_slice(&3u64.to_le_bytes());
        let request = DisplayCoordinatorPresentRequest {
            generation: canvas.generation,
            buffer: HandleRef { raw: handle },
            surface: wire(canvas.surface),
            damage: Damage {
                x: damage.x,
                y: damage.y,
                width: damage.width,
                height: damage.height,
            },
        };
        let result = request
            .encode(&mut bytes[8..], &mut [HandleRef { raw: 0 }])
            .map_err(|_| Status::ErrInvalidArgs)
            .and_then(|n| {
                canvas
                    .display
                    .send(&bytes[..8 + n.bytes], &[handle])
                    .map_err(|_| Status::ErrIo)
            });
        if let Err(error) = result {
            let _ = Memory::close(handle);
            return Err(error);
        }
        Ok(Self {
            deadline_us: now_us().saturating_add(60_000_000),
        })
    }
    pub fn poll(self, canvas: &mut Canvas, now: u64) -> Result<Option<u64>, Status> {
        let result = match read_no_handles(canvas.display, &mut [0; 1024]) {
            Ok(bytes) if bytes.len() == 16 => {
                let response = DisplayCoordinatorPresentResponse::decode(bytes, &[])
                    .map_err(|_| Status::ErrInvalidArgs)?;
                if response.status != Status::Ok {
                    return Err(response.status);
                }
                decode_completion(bytes).map(Some)
            }
            Ok(_) => Err(Status::ErrInvalidArgs),
            Err(kernel_fidl::Status::ErrTimedOut) if now < self.deadline_us => return Ok(None),
            Err(kernel_fidl::Status::ErrTimedOut) => Err(Status::ErrTimedOut),
            Err(_) => Err(Status::ErrIo),
        };
        // A missing or malformed response cannot safely be matched to a future
        // request on this untagged channel. Retire it before allowing a retry.
        if result.is_err() {
            let _ = Memory::close(canvas.display.0);
            canvas.display.0 = 0;
        }
        result
    }
}
pub fn decode_completion(bytes: &[u8]) -> Result<u64, Status> {
    if bytes.len() != 16 {
        return Err(Status::ErrInvalidArgs);
    }
    let response = DisplayCoordinatorPresentResponse::decode(bytes, &[])
        .map_err(|_| Status::ErrInvalidArgs)?;
    if response.status != Status::Ok {
        return Err(response.status);
    }
    if response.fence == 0 {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(response.fence)
}
