//! Real secure-service acceptance, included only in acceptance firmware.
mod boot_ready;
mod gatekeeper;
mod keymint;
use std::ffi::CStr;
use tipc::Handle;

fn receive(handle: &Handle) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; 8192];
    let mut handles = std::array::from_fn::<_, 8, _>(|_| None);
    let (size, count) = handle
        .recv_vectored(&mut [&mut bytes], &mut handles)
        .map_err(|e| format!("TIPC receive: {e:?}"))?;
    if count != 0 {
        return Err("unexpected transferred handles".into());
    }
    bytes.truncate(size);
    Ok(bytes)
}
fn call(port: &CStr, request: &[u8]) -> Result<Vec<u8>, String> {
    let handle = Handle::connect(port).map_err(|e| format!("connect {port:?}: {e:?}"))?;
    handle
        .send(&request)
        .map_err(|e| format!("TIPC send: {e:?}"))?;
    receive(&handle)
}
fn word(data: &[u8], offset: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        data.get(offset..offset + 4)
            .ok_or("short reply")?
            .try_into()
            .unwrap(),
    ))
}
fn storage() -> Result<(), String> {
    let mut session = storage::Session::new(storage::Port::TamperProof, true)
        .map_err(|e| format!("storage connect: {e:?}"))?;
    let mut bytes = [0; 8];
    let old = match session.read("acceptance-counter", &mut bytes) {
        Ok(value) if value.len() == 8 => u64::from_le_bytes(bytes),
        Err(storage::Error::Code(storage::ErrorCode::NotFound)) => 0,
        other => return Err(format!("storage counter read: {other:?}")),
    };
    let generation = old.checked_add(1).ok_or("counter overflow")?;
    session
        .write("acceptance-counter", &generation.to_le_bytes())
        .map_err(|e| format!("storage commit: {e:?}"))?;
    drop(session);
    let mut session = storage::Session::new(storage::Port::TamperProof, true)
        .map_err(|e| format!("storage reconnect: {e:?}"))?;
    if session
        .read("acceptance-counter", &mut bytes)
        .map_err(|e| format!("storage readback: {e:?}"))?
        != generation.to_le_bytes()
    {
        return Err("storage committed value mismatch".into());
    }
    log::info!("bexos-security-acceptance: storage generation={generation}");
    Ok(())
}
fn run() -> Result<(), String> {
    let integrated = boot_ready::integrated();
    boot_ready::wait()?;
    storage()?;
    keymint::run(integrated)?;
    gatekeeper::run()?;
    let request: Vec<u8> = [1u32, 0x202, 1, 0]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let response = call(c"com.bexos.orchestrator", &request)?;
    if response.len() != 16
        || word(&response, 0)? != 1
        || word(&response, 4)? != 0
        || ![1, 2].contains(&word(&response, 8)?)
    {
        return Err("orchestrator response invalid".into());
    }
    log::info!("bexos-security-acceptance: orchestrator reachable");
    let response = call(c"com.android.trusty.avb", &[4, 0, 0, 0, 0, 0, 0, 0])?;
    if response.len() < 12 || word(&response, 0)? != 5 || word(&response, 4)? != 0 {
        return Err("AVB version query failed".into());
    }
    log::info!("bexos-security-acceptance: AVB reachable");
    Ok(())
}
fn main() {
    trusty_log::init();
    match run() {
        Ok(()) => log::info!("bexos-security-acceptance: complete"),
        Err(error) => panic!("bexos-security-acceptance: {error}"),
    }
}
