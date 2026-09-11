//! Validate ownership of an owned graphics response before exposing its handles.
use graphics_fidl::{FidlDecode, FidlEncode, HandleRef, Status};

/// Every transferred handle must occur exactly once, in wire order, in the
/// decoded response. The caller retains ownership and must close all handles
/// on error. Scratch buffers are reusable and bound the accepted response size.
pub fn decode_owned<R: for<'a> FidlDecode<'a> + FidlEncode>(
    bytes: &[u8],
    handles: &[HandleRef],
    scratch: &mut [u8],
    encoded_handles: &mut [HandleRef],
) -> Result<R, Status> {
    let response = R::decode(bytes, handles).map_err(|_| Status::ErrInvalidArgs)?;
    let n = response
        .encode(scratch, encoded_handles)
        .map_err(|_| Status::ErrInvalidArgs)?;
    if n.handles != handles.len()
        || !encoded_handles[..n.handles]
            .iter()
            .zip(handles)
            .all(|(a, b)| a.raw == b.raw)
    {
        return Err(Status::ErrInvalidArgs);
    }
    Ok(response)
}
