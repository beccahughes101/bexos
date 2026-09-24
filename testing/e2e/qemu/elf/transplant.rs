use bexos_debug_client::DebugTransport;
use bexos_e2e::DebugSession;
use std::time::{Duration, Instant};

pub fn replace_network<T: DebugTransport>(session: &mut DebugSession<T>) -> Result<(), String> {
    for (package, generation) in [
        ("bexos.driver.network.virtio_net", 71),
        ("bexos.service.vswitchd", 72),
        ("bexos.service.netstackd", 73),
        ("bexos.service.networkd", 74),
        ("bexos.service.keychaind", 75),
    ] {
        let before = session
            .client
            .list_processes()
            .map_err(|e| format!("process list: {e:?}"))?;
        let old = before
            .iter()
            .find(|p| p.package_id == package && p.state == "Running")
            .ok_or_else(|| format!("missing {package}: {before:?}"))?
            .pid;
        let response = session
            .client
            .exec_command(
                "update.apply_stored_service",
                &[
                    package.into(),
                    generation.to_string(),
                    format!("network-{generation}"),
                ],
            )
            .map_err(|e| format!("apply {package}: {e:?}"))?;
        if response.exit_code != 0 {
            return Err(format!("apply {package}: {response:?}"));
        }
        let prefix = format!("service-transplant: committed generation={generation} cutover_ms=");
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let trace = String::from_utf8_lossy(session.client.received_trace());
            if trace.contains("appd: migration task failed:") || trace.contains("guest-fault") {
                return Err(format!("network replacement failed: {trace}"));
            }
            if let Some(ms) = trace.lines().find_map(|line| {
                line.split_once(&prefix)
                    .and_then(|(_, value)| value.trim().parse::<u64>().ok())
            }) {
                if ms > 150 {
                    return Err(format!("{package} cutover exceeded 150 ms: {ms}"));
                }
                eprintln!("e2e: {package} retained TCP cutover {ms} ms");
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!("{package} cutover timeout: {trace}"));
            }
            session
                .client
                .drain_for(Duration::from_millis(25))
                .map_err(|e| format!("trace: {e:?}"))?;
        }
        let after = session
            .client
            .list_processes()
            .map_err(|e| format!("process list: {e:?}"))?;
        if !after
            .iter()
            .any(|p| p.package_id == package && p.state == "Running" && p.pid != old)
        {
            return Err(format!("replacement identity missing: {after:?}"));
        }
    }
    Ok(())
}
