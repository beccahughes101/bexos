use bexos_trace::{BufferMode, CATEGORY_DEBUG_SERVICE, CATEGORY_IPC_MESSAGES, TraceState};
use bexos_traced::{TraceError, TraceManager};
use bexos_userspace::Channel;
use bexos_userspace::live_migration::State;
use bexos_userspace::service_binding::BoundServiceEndpoint;

#[test]
fn manager_starts_registers_and_stops_trace() {
    let mut manager = TraceManager::new();
    manager
        .start(
            CATEGORY_DEBUG_SERVICE | CATEGORY_IPC_MESSAGES,
            BufferMode::CircularRing,
            64,
            100,
        )
        .unwrap();
    manager
        .register_producer(10, "debugd", CATEGORY_DEBUG_SERVICE, 101)
        .unwrap();
    manager.record_debug_event("debugd:health_check", 102);
    let status = manager.status();
    assert_eq!(status.state, TraceState::Recording);
    assert_eq!(status.producer_count, 2);

    let bytes = manager.stop(103).unwrap();
    assert!(bexos_trace::looks_like_perfetto_trace(&bytes));
    assert!(
        bytes
            .windows(b"debugd:health_check".len())
            .any(|w| w == b"debugd:health_check")
    );
    assert_eq!(manager.status().state, TraceState::Stopped);
    assert_eq!(manager.last_trace(), bytes.as_slice());
}

#[test]
fn manager_rejects_invalid_and_duplicate_sessions() {
    let mut manager = TraceManager::new();
    assert_eq!(
        manager
            .start(0, BufferMode::CircularRing, 64, 1)
            .unwrap_err(),
        TraceError::InvalidArgs
    );
    manager
        .start(CATEGORY_DEBUG_SERVICE, BufferMode::CircularRing, 64, 2)
        .unwrap();
    assert_eq!(
        manager
            .start(CATEGORY_DEBUG_SERVICE, BufferMode::CircularRing, 64, 3)
            .unwrap_err(),
        TraceError::AlreadyRecording
    );
}

#[test]
fn migration_preserves_runtime_trace_state() {
    let mut manager = TraceManager::new();
    manager
        .start(CATEGORY_DEBUG_SERVICE, BufferMode::CircularRing, 64, 10)
        .unwrap();
    manager
        .register_producer(99, "producer", CATEGORY_DEBUG_SERVICE, 11)
        .unwrap();
    manager.record_debug_event("producer:event", 12);

    let mut runtime = bexos_traced::migration::Runtime::new(Channel(1), Some(Channel(2)), manager);
    runtime
        .clients
        .push(BoundServiceEndpoint::new(Channel(3), vec![1, 2, 3, 4]));

    let mut adopted = bexos_traced::migration::Runtime::empty();
    for key in runtime.keys() {
        let record = runtime.encode_record(key).unwrap();
        adopted.adopt_record(key, record.as_deref()).unwrap();
    }

    adopted.validate().unwrap();
    assert_eq!(adopted.clients.len(), 1);
    assert_eq!(adopted.manager.status().state, TraceState::Recording);
    assert_eq!(adopted.manager.status().producer_count, 2);

    let bytes = adopted.manager.stop(13).unwrap();
    assert!(bexos_trace::looks_like_perfetto_trace(&bytes));
    assert!(
        bytes
            .windows(b"producer:event".len())
            .any(|window| window == b"producer:event")
    );
}
