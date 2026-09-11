#![no_main]
bexos_libc::entry!(main);
fn main(raw: u64) -> ! {
    let (service_name, marker) = if cfg!(file_client) {
        ("bexos.test.wasm.FileCounter", "wasm-file-client")
    } else {
        ("bexos.test.wasm.Counter", "wasm-client")
    };
    let control = bexos_userspace::Channel(raw);
    let result = (|| -> Result<(), String> {
        let startup =
            bexos_userspace::Startup::receive(control).map_err(|e| format!("startup {e:?}"))?;
        let endpoint = startup
            .service_grants
            .iter()
            .find(|g| g.service == service_name)
            .ok_or("counter grant missing")?
            .endpoint;
        bexos_userspace::Startup::ready(control).map_err(|e| format!("ready {e:?}"))?;
        let channel = bexos_userspace::Channel(endpoint);
        // The QEMU harness owns this process. Keep its channel alive across all
        // replacements, including staging a full trusted runtime image.
        for expected in 1u32..=u32::MAX {
            channel
                .send(&1u64.to_le_bytes(), &[])
                .map_err(|e| format!("send {e:?}"))?;
            let reply = channel
                .recv_with_timeout(10)
                .map_err(|e| format!("receive {e:?}"))?;
            if !reply.handles.is_empty() || reply.bytes != expected.to_le_bytes() {
                return Err(format!("counter reset or invalid response at {expected}"));
            }
            if expected == 2 || expected % 20 == 0 {
                bexos_userspace::log(&format!("{marker}: counter={expected}\n"));
            }
            let until = bexos_userspace::syscall::ticks()
                .saturating_add(bexos_userspace::syscall::frequency());
            while bexos_userspace::syscall::ticks() < until {
                bexos_userspace::yield_now();
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        bexos_userspace::log(&format!("{marker}: failed: {error}\n"));
    }
    bexos_userspace::exit()
}
