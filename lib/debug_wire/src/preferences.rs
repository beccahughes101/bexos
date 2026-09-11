use super::*;
pub const METHOD_PREFERENCES: u32 = 43;
/// operation: 0 get, 1 set, 2 reset, 3 lock, 4 unlock, 5 operator info.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PreferencesRequest {
    pub package_id: String,
    pub uid: u64,
    pub operation: u32,
    pub expected_generation: u64,
    pub config: Vec<u8>,
    pub names: Vec<String>,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PreferencesResponse {
    pub status: i32,
    pub generation: u64,
    pub config: Vec<u8>,
    pub schema: Vec<u8>,
    pub locks: Vec<String>,
    pub message: String,
}
pub fn encode_preferences_request(v: &PreferencesRequest, out: &mut Vec<u8>) {
    out.clear();
    put_string(out, 1, &v.package_id);
    put_varint_field(out, 2, v.uid);
    put_varint_field(out, 3, u64::from(v.operation));
    put_varint_field(out, 4, v.expected_generation);
    put_len_field(out, 5, &v.config);
    for n in &v.names {
        put_string(out, 6, n);
    }
}
pub fn decode_preferences_request(b: &[u8]) -> Result<PreferencesRequest, WireError> {
    let mut v = PreferencesRequest::default();
    read_fields(b, |n, k, b| {
        match (n, k) {
            (1, 2) => v.package_id = read_string(b)?,
            (2, 0) => v.uid = read_varint_from(b)?,
            (3, 0) => {
                v.operation =
                    u32::try_from(read_varint_from(b)?).map_err(|_| WireError::InvalidProto)?
            }
            (4, 0) => v.expected_generation = read_varint_from(b)?,
            (5, 2) => v.config = b.to_vec(),
            (6, 2) => v.names.push(read_string(b)?),
            _ => {}
        }
        Ok(())
    })?;
    if v.package_id.len() > 128 || v.names.len() > 256 || v.names.iter().any(|n| n.len() > 64) {
        return Err(WireError::InvalidProto);
    }
    Ok(v)
}
pub fn encode_preferences_response(v: &PreferencesResponse, out: &mut Vec<u8>) {
    out.clear();
    put_varint_field(out, 1, v.status as u64);
    put_varint_field(out, 2, v.generation);
    put_len_field(out, 3, &v.config);
    put_len_field(out, 4, &v.schema);
    for n in &v.locks {
        put_string(out, 5, n);
    }
    put_string(out, 6, &v.message);
}
pub fn decode_preferences_response(b: &[u8]) -> Result<PreferencesResponse, WireError> {
    let mut v = PreferencesResponse::default();
    read_fields(b, |n, k, b| {
        match (n, k) {
            (1, 0) => v.status = read_varint_from(b)? as i32,
            (2, 0) => v.generation = read_varint_from(b)?,
            (3, 2) => v.config = b.to_vec(),
            (4, 2) => v.schema = b.to_vec(),
            (5, 2) => v.locks.push(read_string(b)?),
            (6, 2) => v.message = read_string(b)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(v)
}
