use bexos_i2c_spi::{BusKind, I2cBundle, I2cOp, Request, SubmitResult, topology_proto};
use bexos_i2cd::Runtime;
use bexos_userspace::{Channel, live_migration::State};

#[test]
fn packaged_topology_builds_i2c_runtime_with_scoped_channels() {
    let topology = topology_proto::decode(include_bytes!(env!("I2C_SPI_TOPOLOGY"))).unwrap();
    let mut runtime = Runtime::new(Channel(1), Some(Channel(2)), topology).unwrap();
    assert_eq!(runtime.controllers.len(), 1);
    assert_eq!(runtime.controllers[0].config.bus, BusKind::I2c);
    assert_eq!(runtime.controllers[0].config.peripherals.len(), 2);
    runtime.controllers[0]
        .add_endpoint(10, 4097, vec![1, 2, 3])
        .unwrap();
    assert_eq!(
        runtime.controllers[0].submit(
            10,
            4098,
            Request::I2c(I2cBundle {
                operations: vec![I2cOp::Read(1)]
            }),
            1000,
            0
        ),
        SubmitResult::Rejected(bexos_i2c_spi::Status::AccessDenied)
    );
}

#[test]
fn migration_preserves_i2c_backend_endpoints_and_registered_state() {
    let topology = topology_proto::decode(include_bytes!(env!("I2C_SPI_TOPOLOGY"))).unwrap();
    let mut runtime = Runtime::new(Channel(1), Some(Channel(2)), topology).unwrap();
    runtime.registered = true;
    runtime.controllers[0]
        .add_endpoint(10, 4097, vec![1, 2, 3])
        .unwrap();
    runtime
        .backend
        .inject_failure(4097, bexos_i2c_spi::Status::ArbitrationLost, 1);

    let mut adopted = Runtime::empty();
    for key in runtime.keys() {
        let record = runtime.encode_record(key).unwrap();
        adopted.adopt_record(key, record.as_deref()).unwrap();
    }
    adopted.validate().unwrap();
    assert!(adopted.registered);
    assert_eq!(adopted.controllers[0].clients[0].peripheral_node_id, 4097);
    assert_eq!(adopted.backend.i2c_operations, 0);
}
