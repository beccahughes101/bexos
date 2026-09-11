fn append(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
}
fn request(command: u32) -> Vec<u8> {
    [command, 0, 901]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect()
}
fn verify(blob: &[u8], password: &[u8]) -> Result<Vec<u8>, String> {
    let mut bytes = request(2);
    bytes.extend_from_slice(&0u64.to_le_bytes());
    append(&mut bytes, blob);
    append(&mut bytes, password);
    super::call(c"com.android.trusty.gatekeeper", &bytes)
}
pub fn run() -> Result<(), String> {
    let password = b"standalone-test-password";
    let mut bytes = request(0);
    append(&mut bytes, password);
    append(&mut bytes, &[]);
    append(&mut bytes, &[]);
    let response = super::call(c"com.android.trusty.gatekeeper", &bytes)?;
    if super::word(&response, 0)? != 1 || super::word(&response, 4)? != 0 {
        return Err("Gatekeeper enroll failed".into());
    }
    let size = super::word(&response, 12)? as usize;
    let blob = response
        .get(16..16 + size)
        .filter(|v| !v.is_empty())
        .ok_or("invalid password handle")?;
    let response = verify(blob, password)?;
    if super::word(&response, 0)? != 3
        || super::word(&response, 4)? != 0
        || super::word(&response, 12)? != 69
    {
        return Err("Gatekeeper verification failed".into());
    }
    let response = verify(blob, b"wrong password")?;
    if super::word(&response, 0)? != 3 || super::word(&response, 4)? != 1 {
        return Err("Gatekeeper did not reject wrong password".into());
    }
    log::info!("bexos-security-acceptance: Gatekeeper verification and rejection verified");
    Ok(())
}
