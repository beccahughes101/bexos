use bexos_i2c_spi::{BusKind, Request, SpiBundle, SpiMode, SpiOp, SubmitResult, topology_proto};
use bexos_spid::Runtime;
use bexos_userspace::{Channel, live_migration::State};

#[test]
fn packaged_topology_builds_spi_runtime_and_validates_configuration() {
    let topology = topology_proto::decode(include_bytes!(env!("I2C_SPI_TOPOLOGY"))).unwrap();
    let mut runtime = Runtime::new(Channel(1), Some(Channel(2)), topology).unwrap();
    assert_eq!(runtime.controllers.len(), 1);
    assert_eq!(runtime.controllers[0].config.bus, BusKind::Spi);
    runtime.controllers[0]
        .add_endpoint(20, 8193, vec![1, 2, 3])
        .unwrap();
    assert_eq!(
        runtime.controllers[0].submit(
            20,
            8193,
            Request::Spi(SpiBundle {
                operations: vec![SpiOp::FullDuplex(vec![1, 2])],
                speed_hz: 99_999,
                mode: SpiMode::Mode0,
            }),
            1000,
            0
        ),
        SubmitResult::Rejected(bexos_i2c_spi::Status::InvalidArgs)
    );
}

#[test]
fn migration_preserves_spi_queue_and_configuration_state() {
    let topology = topology_proto::decode(include_bytes!(env!("I2C_SPI_TOPOLOGY"))).unwrap();
    let mut runtime = Runtime::new(Channel(1), Some(Channel(2)), topology).unwrap();
    runtime.registered = true;
    runtime.controllers[0]
        .add_endpoint(20, 8193, vec![1, 2, 3])
        .unwrap();
    assert_eq!(
        runtime.controllers[0].submit(
            20,
            8193,
            Request::Spi(SpiBundle {
                operations: vec![SpiOp::FullDuplex(vec![1, 2])],
                speed_hz: 1_000_000,
                mode: SpiMode::Mode0,
            }),
            1000,
            0
        ),
        SubmitResult::Queued(1)
    );

    let mut adopted = Runtime::empty();
    for key in runtime.keys() {
        let record = runtime.encode_record(key).unwrap();
        adopted.adopt_record(key, record.as_deref()).unwrap();
    }
    adopted.validate().unwrap();
    assert!(adopted.registered);
    assert_eq!(adopted.controllers[0].queue.len(), 1);
}
