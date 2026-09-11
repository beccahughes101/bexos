use bexos_userspace::{Channel, fs};
use time_fidl::{ClockSource, Status, SyncState, TimeQuality};

const STATE_FILE: &str = "time.state";
const MAGIC: &[u8; 8] = b"BEXTIME1";
const STATE_LEN: usize = 48;

pub fn default_quality() -> TimeQuality {
    TimeQuality {
        source: ClockSource::SntpNetwork,
        state: SyncState::Unsynced,
        stratum: 0,
        root_dispersion_ns: 0,
        last_synced_timestamp_ns: 0,
        utc_offset_ns: 0,
        last_error: Status::ErrNotFound,
    }
}

pub fn load(data: Channel) -> TimeQuality {
    let Ok(file) = fs::open(data, STATE_FILE, 1) else {
        return default_quality();
    };
    let result = fs::read(file, STATE_LEN as u64)
        .ok()
        .and_then(|bytes| decode(&bytes));
    let _ = fs::close(file);
    result.unwrap_or_else(default_quality)
}

pub fn store(data: Channel, quality: &TimeQuality) -> Result<(), Status> {
    let file = fs::open(data, STATE_FILE, 1 | 2 | 8 | 16).map_err(|_| Status::ErrIo)?;
    let bytes = encode(quality);
    let result = fs::write(file, &bytes).map_err(|_| Status::ErrIo);
    let _ = fs::close(file);
    result
}

pub fn encode(quality: &TimeQuality) -> [u8; STATE_LEN] {
    let mut out = [0; STATE_LEN];
    out[..8].copy_from_slice(MAGIC);
    out[8] = quality.source as u8;
    out[9] = quality.state as u8;
    out[10] = quality.stratum;
    out[12..20].copy_from_slice(&quality.root_dispersion_ns.to_le_bytes());
    out[20..28].copy_from_slice(&quality.last_synced_timestamp_ns.to_le_bytes());
    out[28..36].copy_from_slice(&quality.utc_offset_ns.to_le_bytes());
    out[36..40].copy_from_slice(&(quality.last_error as i32).to_le_bytes());
    out
}

pub fn decode(bytes: &[u8]) -> Option<TimeQuality> {
    if bytes.len() < STATE_LEN || &bytes[..8] != MAGIC {
        return None;
    }
    Some(TimeQuality {
        source: decode_source(bytes[8])?,
        state: decode_state(bytes[9])?,
        stratum: bytes[10],
        root_dispersion_ns: u64::from_le_bytes(bytes[12..20].try_into().ok()?),
        last_synced_timestamp_ns: u64::from_le_bytes(bytes[20..28].try_into().ok()?),
        utc_offset_ns: i64::from_le_bytes(bytes[28..36].try_into().ok()?),
        last_error: decode_status(i32::from_le_bytes(bytes[36..40].try_into().ok()?))?,
    })
}

fn decode_source(value: u8) -> Option<ClockSource> {
    match value {
        1 => Some(ClockSource::RtcHardware),
        2 => Some(ClockSource::SntpNetwork),
        3 => Some(ClockSource::NtsSecure),
        4 => Some(ClockSource::CellularNitz),
        5 => Some(ClockSource::ManualUser),
        _ => None,
    }
}

fn decode_state(value: u8) -> Option<SyncState> {
    match value {
        1 => Some(SyncState::Unsynced),
        2 => Some(SyncState::Synced),
        3 => Some(SyncState::Failed),
        4 => Some(SyncState::Manual),
        _ => None,
    }
}

fn decode_status(value: i32) -> Option<Status> {
    match value {
        0 => Some(Status::Ok),
        -6 => Some(Status::ErrTimedOut),
        -8 => Some(Status::ErrInvalidArgs),
        -10 => Some(Status::ErrNotFound),
        -12 => Some(Status::ErrNetworkUnreachable),
        -13 => Some(Status::ErrUnsupported),
        -14 => Some(Status::ErrIo),
        _ => None,
    }
}
