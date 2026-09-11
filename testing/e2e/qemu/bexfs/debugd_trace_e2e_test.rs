use bexos_e2e::E2eDevice;
use bexos_qemu_test::{QemuArtifacts, QemuDevice};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let artifacts = QemuArtifacts::from_args(&args)?;
    let markers = [
        b"debugd: QEMU socket transport ready".as_slice(),
        b"traced: ready".as_slice(),
    ];
    let mut device = QemuDevice::new(artifacts)?;
    let mut session = device.boot_with_debugd(&markers)?;
    session.assert_debugd_ready()?;

    let mut traced = session.traced_test("debugd_trace", bexos_trace::CATEGORY_DEBUG_SERVICE)?;
    traced
        .session()
        .client
        .health_check()
        .map_err(|e| format!("{e:?}"))?;
    let analysis = traced.finish()?;
    analysis.assert_event_present("debugd:trace_start")?;
    analysis.assert_event_present("debugd:health_check")?;
    Ok(())
}
