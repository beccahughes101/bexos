//! Keep display failures distinct from malformed compositor requests. Until
//! transfer succeeds the acknowledgement endpoint remains owned by the caller.
use crate::state::Splash;
use bexos_graphics_runtime as rt;
use bexos_userspace::{Channel, Memory};
use graphics_fidl::*;

pub(crate) fn begin(
    s: &mut Splash,
    bytes: &[u8],
    handles: &[HandleRef],
) -> Result<DisplayFrame, Status> {
    if handles.len() != 1 {
        return Err(Status::ErrInvalidArgs);
    }
    let request = ProgressTrackerHandoverToCompositorRequest::decode(bytes, handles)
        .map_err(|_| Status::ErrInvalidArgs)?;
    // Progress and readiness arrive on separate channels, without ordering.
    if s.frozen || s.progress.stage < 4 {
        return Err(Status::ErrBusy);
    }
    let canvas = s.canvas.as_mut().ok_or(Status::ErrIo)?;
    let snapshot: DisplayCoordinatorSnapshotResponse = rt::call(
        &mut canvas.display,
        5,
        &DisplayCoordinatorSnapshotRequest {},
    )
    .inspect_err(|error| {
        bexos_userspace::log(&format!("splashd: snapshot RPC failed {error:?}\n"))
    })?;
    if snapshot.status != Status::Ok {
        if let Some(frame) = snapshot.frame {
            let _ = Memory::close(frame.buffer.raw);
        }
        return Err(snapshot.status);
    }
    let frame = snapshot.frame.ok_or(Status::ErrIo)?;
    let transfer = (|| {
        let reply: DisplayCoordinatorTransferResponse = rt::call(
            &mut canvas.display,
            4,
            &DisplayCoordinatorTransferRequest {
                generation: canvas.generation,
                next_client: request.next_client,
            },
        )?;
        if reply.status != Status::Ok {
            return Err(reply.status);
        }
        Ok(reply.generation)
    })();
    let generation = match transfer {
        Ok(generation) => generation,
        Err(error) => {
            let _ = Memory::close(frame.buffer.raw);
            bexos_userspace::log(&format!("splashd: display transfer failed {error:?}\n"));
            return Err(error);
        }
    };
    s.frozen = true;
    s.ack = Some(Channel(request.ack_channel.raw));
    s.deadline = rt::now_us().saturating_add(1_500_000);
    s.handoff_generation = generation;
    Ok(DisplayFrame {
        generation,
        ..frame
    })
}
