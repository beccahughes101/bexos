use time_fidl::{ClockSource, Status, SyncState, TimeQuality};

use sntpc as _;

const NTP_UNIX_EPOCH_DELTA: u64 = 2_208_988_800;
const NTP_PACKET_LEN: usize = 48;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SntpSample {
    pub unix_time_ns: u64,
    pub stratum: u8,
    pub root_dispersion_ns: u64,
}

pub fn encode_request(out: &mut [u8]) -> Result<usize, Status> {
    if out.len() < NTP_PACKET_LEN {
        return Err(Status::ErrInvalidArgs);
    }
    out[..NTP_PACKET_LEN].fill(0);
    out[0] = 0x23;
    Ok(NTP_PACKET_LEN)
}

pub fn decode_response(packet: &[u8]) -> Result<SntpSample, Status> {
    if packet.len() < NTP_PACKET_LEN {
        return Err(Status::ErrInvalidArgs);
    }
    let mode = packet[0] & 0x7;
    if mode != 4 && mode != 5 {
        return Err(Status::ErrInvalidArgs);
    }
    let stratum = packet[1];
    if stratum == 0 {
        return Err(Status::ErrInvalidArgs);
    }
    let seconds = read_u32(packet, 40)? as u64;
    if seconds < NTP_UNIX_EPOCH_DELTA {
        return Err(Status::ErrInvalidArgs);
    }
    let fraction = read_u32(packet, 44)? as u64;
    let unix_seconds = seconds - NTP_UNIX_EPOCH_DELTA;
    let unix_time_ns = unix_seconds
        .checked_mul(1_000_000_000)
        .and_then(|base| base.checked_add((fraction * 1_000_000_000) >> 32))
        .ok_or(Status::ErrInvalidArgs)?;
    let root_dispersion = read_u32(packet, 8)? as u64;
    let root_dispersion_ns = ((root_dispersion >> 16) * 1_000_000_000)
        .saturating_add(((root_dispersion & 0xffff) * 1_000_000_000) >> 16);
    Ok(SntpSample {
        unix_time_ns,
        stratum,
        root_dispersion_ns,
    })
}

pub fn quality_from_sample(sample: SntpSample, monotonic_ns: u64) -> TimeQuality {
    TimeQuality {
        source: ClockSource::SntpNetwork,
        state: SyncState::Synced,
        stratum: sample.stratum,
        root_dispersion_ns: sample.root_dispersion_ns,
        last_synced_timestamp_ns: sample.unix_time_ns,
        utc_offset_ns: sample.unix_time_ns as i64 - monotonic_ns as i64,
        last_error: Status::Ok,
    }
}

fn read_u32(packet: &[u8], offset: usize) -> Result<u32, Status> {
    let bytes = packet
        .get(offset..offset + 4)
        .ok_or(Status::ErrInvalidArgs)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}
