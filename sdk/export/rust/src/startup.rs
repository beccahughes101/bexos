use crate::arch;

const CHANNEL_CONTROL_PROTOCOL: u64 = 1;
const WRITE_MESSAGE_ORDINAL: u64 = 3_778_539_391_082_572_459;
const READY_REQUEST: [u8; 44] = [
    0, 0, 0, 0, // startup channel handle table index
    40, 0, 0, 0, 0, 0, 0, 0, // byte-vector offset
    4, 0, 0, 0, 0, 0, 0, 0, // byte-vector length
    1, 0, 0, 0, 0, 0, 0, 0, // handle-vector offset
    0, 0, 0, 0, 0, 0, 0, 0, // handle-vector length
    0, 0, 0, 0, // inline padding
    0, 0, 0, 0, // ready status payload
];

/// Announces that a service process has accepted its startup channel.
pub fn service_ready(startup_channel: u64) -> Result<(), i32> {
    let mut response = core::mem::MaybeUninit::<[u8; 8]>::uninit();
    let response = unsafe { &mut *response.as_mut_ptr() };
    let response_len = arch::fidl(
        CHANNEL_CONTROL_PROTOCOL,
        WRITE_MESSAGE_ORDINAL,
        &READY_REQUEST,
        &[startup_channel],
        response,
    )?;
    if response_len < 4 {
        return Err(-8);
    }
    let status = i32::from_le_bytes([response[0], response[1], response[2], response[3]]);
    if status == 0 { Ok(()) } else { Err(status) }
}
