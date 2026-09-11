use bexos_lazy_service::{IdleDecision, LazyServiceController, StopDecision};

#[test]
fn final_connection_starts_full_grace_period() {
    let controller = LazyServiceController::new(2);
    let guard = controller.track_connection();
    assert_eq!(controller.idle_decision(10), IdleDecision::Busy);
    drop(guard);
    assert_eq!(
        controller.idle_decision(10),
        IdleDecision::Deadline {
            generation: 0,
            deadline_ns: 2_000_010
        }
    );
    assert_eq!(
        controller.idle_decision(2_000_010),
        IdleDecision::Ready { generation: 0 }
    );
}

#[test]
fn stale_stop_generation_is_rejected() {
    let controller = LazyServiceController::new(0);
    assert_eq!(
        controller.idle_decision(5),
        IdleDecision::Ready { generation: 0 }
    );
    assert_eq!(
        controller.acknowledge_stop(0),
        StopDecision::Accepted { generation: 1 }
    );
    assert_eq!(controller.acknowledge_stop(0), StopDecision::Stale);
}

#[test]
fn keepalive_blocks_idle_until_released() {
    let controller = LazyServiceController::new(0);
    let keepalive = controller.keep_alive();
    assert_eq!(controller.idle_decision(1), IdleDecision::Busy);
    drop(keepalive);
    assert_eq!(
        controller.idle_decision(1),
        IdleDecision::Ready { generation: 0 }
    );
}
