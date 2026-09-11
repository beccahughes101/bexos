use bexos_i2c_spi::{
    BusController, BusEvent, BusKind, ControllerConfig, DeterministicBackend, I2cAddress,
    I2cBundle, I2cOp, Peripheral, PeripheralConfig, Request, SpiBundle, SpiMode, SpiOp, Status,
    SubmitResult, Topology,
    topology::{DeviceProperty, I2cPeripheralConfig, SpiPeripheralConfig},
};
use bexos_migration::codec::{Decoder, Encoder};

fn topology() -> Topology {
    Topology {
        controllers: vec![
            ControllerConfig {
                node_id: 1,
                name: "i2c0".into(),
                bus: BusKind::I2c,
                properties: vec![DeviceProperty {
                    key: "i2c.bus".into(),
                    value: 0,
                }],
                peripherals: vec![
                    Peripheral {
                        node_id: 10,
                        name: "sensor".into(),
                        properties: vec![DeviceProperty {
                            key: "i2c.address".into(),
                            value: 0x48,
                        }],
                        config: PeripheralConfig::I2c(I2cPeripheralConfig {
                            address: I2cAddress::new(0x48, false).unwrap(),
                        }),
                    },
                    Peripheral {
                        node_id: 11,
                        name: "eeprom".into(),
                        properties: vec![DeviceProperty {
                            key: "i2c.address".into(),
                            value: 0x300,
                        }],
                        config: PeripheralConfig::I2c(I2cPeripheralConfig {
                            address: I2cAddress::new(0x300, true).unwrap(),
                        }),
                    },
                ],
            },
            ControllerConfig {
                node_id: 2,
                name: "spi0".into(),
                bus: BusKind::Spi,
                properties: vec![DeviceProperty {
                    key: "spi.bus".into(),
                    value: 0,
                }],
                peripherals: vec![
                    Peripheral {
                        node_id: 20,
                        name: "display".into(),
                        properties: vec![DeviceProperty {
                            key: "spi.chip_select".into(),
                            value: 0,
                        }],
                        config: PeripheralConfig::Spi(SpiPeripheralConfig {
                            chip_select: 0,
                            min_speed_hz: 100_000,
                            max_speed_hz: 24_000_000,
                            mode_mask: SpiMode::Mode0.bit() | SpiMode::Mode2.bit(),
                        }),
                    },
                    Peripheral {
                        node_id: 21,
                        name: "adc".into(),
                        properties: vec![DeviceProperty {
                            key: "spi.chip_select".into(),
                            value: 1,
                        }],
                        config: PeripheralConfig::Spi(SpiPeripheralConfig {
                            chip_select: 1,
                            min_speed_hz: 50_000,
                            max_speed_hz: 12_000_000,
                            mode_mask: SpiMode::Mode1.bit(),
                        }),
                    },
                ],
            },
        ],
    }
}

fn i2c_controller() -> (BusController, DeterministicBackend) {
    let topo = topology();
    let controller = topo.controllers[0].clone();
    (
        BusController::new(controller),
        DeterministicBackend::new(&topo),
    )
}

fn spi_controller() -> (BusController, DeterministicBackend) {
    let topo = topology();
    let controller = topo.controllers[1].clone();
    (
        BusController::new(controller),
        DeterministicBackend::new(&topo),
    )
}

#[test]
fn topology_rejects_duplicate_i2c_addresses_and_bad_spi_limits() {
    let mut topo = topology();
    topo.controllers[0].peripherals[1].config = PeripheralConfig::I2c(I2cPeripheralConfig {
        address: I2cAddress::new(0x48, false).unwrap(),
    });
    assert_eq!(topo.validate(), Err(Status::AlreadyExists));

    let mut topo = topology();
    topo.controllers[1].peripherals[0].config = PeripheralConfig::Spi(SpiPeripheralConfig {
        chip_select: 0,
        min_speed_hz: 10,
        max_speed_hz: 1,
        mode_mask: 1,
    });
    assert_eq!(topo.validate(), Err(Status::InvalidArgs));
}

#[test]
fn i2c_scopes_clients_to_addresses_and_emits_repeated_start_stop() {
    let (mut controller, mut backend) = i2c_controller();
    controller.add_endpoint(100, 10, vec![1, 2, 3]).unwrap();
    let bundle = I2cBundle {
        operations: vec![I2cOp::Write(vec![9, 8]), I2cOp::Read(3)],
    };
    assert_eq!(
        controller.submit(100, 11, Request::I2c(bundle.clone()), 1000, 0),
        SubmitResult::Rejected(Status::AccessDenied)
    );
    assert_eq!(
        controller.submit(100, 10, Request::I2c(bundle), 1000, 0),
        SubmitResult::Queued(1)
    );
    assert!(controller.poll(&mut backend, 0));
    assert!(controller.poll(&mut backend, 0));
    let completion = controller.next_completion().unwrap();
    assert_eq!(completion.outcome.status, Status::Ok);
    assert_eq!(completion.outcome.reads[0].bytes, vec![9, 8, 9]);
    assert_eq!(
        backend.events(),
        &[
            BusEvent::I2cStart(10),
            BusEvent::I2cRepeatedStart(10),
            BusEvent::I2cStop(10)
        ]
    );
}

#[test]
fn spi_validates_chip_select_scope_configuration_and_cs_boundary() {
    let (mut controller, mut backend) = spi_controller();
    controller.add_endpoint(200, 20, vec![1, 2, 3]).unwrap();
    let exchange = SpiBundle {
        operations: vec![SpiOp::FullDuplex(vec![0x55, 0xaa])],
        speed_hz: 1_000_000,
        mode: SpiMode::Mode0,
    };
    assert_eq!(
        controller.submit(200, 21, Request::Spi(exchange.clone()), 1000, 0),
        SubmitResult::Rejected(Status::AccessDenied)
    );
    assert_eq!(
        controller.submit(
            200,
            20,
            Request::Spi(SpiBundle {
                mode: SpiMode::Mode1,
                ..exchange.clone()
            }),
            1000,
            0
        ),
        SubmitResult::Rejected(Status::InvalidArgs)
    );
    assert_eq!(
        controller.submit(200, 20, Request::Spi(exchange), 1000, 0),
        SubmitResult::Queued(1)
    );
    assert!(controller.poll(&mut backend, 0));
    assert!(controller.poll(&mut backend, 0));
    let completion = controller.next_completion().unwrap();
    assert_eq!(completion.outcome.reads[0].bytes, vec![0xaa, 0x55]);
    assert_eq!(
        backend.events(),
        &[BusEvent::SpiCsAssert(20), BusEvent::SpiCsDeassert(20)]
    );
    assert_eq!(backend.spi_config(20), Some((SpiMode::Mode0, 1_000_000)));
    assert_eq!(backend.spi_config(21), None);
}

#[test]
fn fifo_bounds_and_request_deadlines_are_enforced() {
    let (mut controller, mut backend) = i2c_controller();
    controller.add_endpoint(100, 10, vec![1, 2, 3]).unwrap();
    backend.set_delay(1, 20);
    let request = Request::I2c(I2cBundle {
        operations: vec![I2cOp::Read(1)],
    });
    for expected in 1..=64 {
        assert_eq!(
            controller.submit(100, 10, request.clone(), 10, 0),
            SubmitResult::Queued(expected)
        );
    }
    assert_eq!(
        controller.submit(100, 10, request.clone(), 10, 0),
        SubmitResult::Rejected(Status::QueueFull)
    );
    assert!(controller.poll(&mut backend, 0));
    assert!(controller.poll(&mut backend, 11));
    assert_eq!(
        controller.next_completion().unwrap().outcome.status,
        Status::TimedOut
    );
}

#[test]
fn bundle_size_limit_rejects_excess_work() {
    let (mut controller, _) = i2c_controller();
    controller.add_endpoint(100, 10, vec![1, 2, 3]).unwrap();
    assert_eq!(
        controller.submit(
            100,
            10,
            Request::I2c(I2cBundle {
                operations: vec![I2cOp::Write(vec![0; 4097])]
            }),
            1000,
            0
        ),
        SubmitResult::Rejected(Status::TooLarge)
    );
}

#[test]
fn lock_expires_and_peer_disconnect_releases_ownership() {
    let (mut controller, mut backend) = i2c_controller();
    controller.add_endpoint(100, 10, vec![1, 2, 3]).unwrap();
    assert_eq!(
        controller.lock_bus(100, 101, 10, vec![1, 2], 250, 5),
        Ok(105)
    );
    assert_eq!(
        controller.submit(
            100,
            10,
            Request::I2c(I2cBundle {
                operations: vec![I2cOp::Read(1)]
            }),
            10,
            6
        ),
        SubmitResult::Queued(1)
    );
    assert!(!controller.poll(&mut backend, 10));
    assert!(controller.poll(&mut backend, 20));
    assert_eq!(
        controller.next_completion().unwrap().outcome.status,
        Status::TimedOut
    );
    assert_eq!(
        controller.submit(
            101,
            10,
            Request::I2c(I2cBundle {
                operations: vec![I2cOp::Read(1)]
            }),
            1000,
            106
        ),
        SubmitResult::Rejected(Status::LockExpired)
    );
    controller.release_channel(101);
    assert_eq!(
        controller.submit(
            100,
            10,
            Request::I2c(I2cBundle {
                operations: vec![I2cOp::Read(1)]
            }),
            1000,
            106
        ),
        SubmitResult::Queued(2)
    );
    assert!(controller.poll(&mut backend, 106));
}

#[test]
fn injected_failures_are_distinct_and_not_replayed() {
    let (mut controller, mut backend) = i2c_controller();
    controller.add_endpoint(100, 10, vec![1, 2, 3]).unwrap();
    backend.inject_failure(10, Status::Nack, 1);
    let request = Request::I2c(I2cBundle {
        operations: vec![I2cOp::Write(vec![1]), I2cOp::Read(1)],
    });
    assert_eq!(
        controller.submit(100, 10, request.clone(), 1000, 0),
        SubmitResult::Queued(1)
    );
    assert!(controller.poll(&mut backend, 0));
    assert!(controller.poll(&mut backend, 0));
    assert_eq!(
        controller.next_completion().unwrap().outcome.status,
        Status::Nack
    );
    assert_eq!(backend.i2c_operations, 0);
    assert_eq!(
        controller.submit(100, 10, request, 1000, 0),
        SubmitResult::Queued(2)
    );
    assert!(controller.poll(&mut backend, 0));
    assert!(controller.poll(&mut backend, 0));
    assert_eq!(
        controller.next_completion().unwrap().outcome.status,
        Status::Ok
    );
    assert_eq!(backend.i2c_operations, 2);
}

#[test]
fn controller_and_backend_migration_preserve_queue_active_backend_and_lease_deadlines() {
    let (mut controller, mut backend) = i2c_controller();
    controller.add_endpoint(100, 10, vec![1, 2, 3]).unwrap();
    controller
        .lock_bus(100, 101, 10, vec![1, 2], 100, 7)
        .unwrap();
    backend.set_delay(1, 50);
    let request = Request::I2c(I2cBundle {
        operations: vec![I2cOp::Write(vec![4, 5]), I2cOp::Read(2)],
    });
    assert_eq!(
        controller.submit(101, 10, request.clone(), 1000, 8),
        SubmitResult::Queued(1)
    );
    assert!(controller.poll(&mut backend, 8));
    assert_eq!(controller.active.as_ref().unwrap().ready_at_ms, 58);
    assert_eq!(
        controller.submit(101, 10, request, 1000, 9),
        SubmitResult::Queued(2)
    );

    let mut w = Encoder::new();
    bexos_i2c_spi::migration::encode_controller(&mut w, &controller);
    let controller_record = w.finish();
    let mut r = Decoder::new(&controller_record);
    let mut adopted = bexos_i2c_spi::migration::decode_controller(&mut r).unwrap();
    r.finish().unwrap();

    let mut bw = Encoder::new();
    bexos_i2c_spi::migration::encode_backend(&mut bw, &backend);
    let backend_record = bw.finish();
    let mut br = Decoder::new(&backend_record);
    let mut adopted_backend = bexos_i2c_spi::migration::decode_backend(&mut br).unwrap();
    br.finish().unwrap();

    assert_eq!(adopted.clients[1].lock.as_ref().unwrap().expires_at_ms, 107);
    assert_eq!(adopted.queue.len(), 1);
    assert!(!adopted.quiescence_ready(&mut adopted_backend, 40));
    assert!(adopted.quiescence_ready(&mut adopted_backend, 58));
    let completion = adopted.next_completion().unwrap();
    assert_eq!(completion.outcome.status, Status::Ok);
    assert_eq!(adopted_backend.register_bytes(10), Some(&[4, 5][..]));
}

#[test]
fn malformed_migration_records_are_rejected() {
    let mut invalid = vec![0; 16];
    invalid[0] = 9;
    let mut r = Decoder::new(&invalid);
    assert!(bexos_i2c_spi::migration::decode_controller(&mut r).is_err());
}
