use core::time::Duration;

use bexos_d1_linux_shim::bexos::MappedMmio;
use bexos_d1_linux_shim::mmio::Mmio;

#[test]
fn mapped_mmio_tracks_raw_memory_registers() {
    let mut registers = [0u32; 4];
    let mut mmio = MappedMmio::new(registers.as_mut_ptr() as u64);

    mmio.write32(4, 0x1122_3344).expect("write32");
    assert_eq!(registers[1], 0x1122_3344);
    assert_eq!(mmio.read32(4).expect("read32"), 0x1122_3344);
}

#[test]
fn wait_until_completes_after_poll_turns_true() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        let mut polls = 0;
        bexos_d1_linux_shim::task::wait_until(Duration::from_millis(50), || {
            polls += 1;
            Ok(polls == 2)
        })
        .await
        .expect("wait_until");
        assert_eq!(polls, 2);
    });
}
