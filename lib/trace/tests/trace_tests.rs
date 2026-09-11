use bexos_trace::{
    BufferMode, CATEGORY_ALL, CATEGORY_DEBUG_SERVICE, CATEGORY_IPC_MESSAGES, HEADER_SIZE,
    MAX_PRODUCER_BUFFER_SIZE, SLOT_SIZE, SharedTraceReader, SharedTraceWriter, TraceConfig,
    TraceEvent, TraceEventKind, TraceOutputFormat, TraceProducer, TraceSession, category_list,
    export_legacy_bexos_fxt, export_perfetto_trace, looks_like_legacy_bexos_fxt,
    looks_like_perfetto_trace, parse_category_list,
};

#[test]
fn category_lists_round_trip_common_names() {
    let categories = parse_category_list("kernel,ipc,vfs,network,ui,app,debug").unwrap();
    assert_eq!(categories, CATEGORY_ALL);
    assert_eq!(parse_category_list("all").unwrap(), CATEGORY_ALL);
    assert!(category_list(categories).contains("kernel_sched"));
    assert_eq!(parse_category_list("wat"), None);
}

#[test]
fn trace_session_records_and_exports() {
    let mut session = TraceSession::new(
        TraceConfig {
            categories: CATEGORY_DEBUG_SERVICE | CATEGORY_IPC_MESSAGES,
            buffer_mode: BufferMode::CircularRing,
            buffer_size_kb: 8,
            output_format: TraceOutputFormat::Perfetto,
        },
        10,
    );
    session.register_producer(
        TraceProducer {
            id: 42,
            pid: 42,
            main_tid: 1,
            process_name: "debugd".into(),
            categories: CATEGORY_DEBUG_SERVICE,
        },
        11,
    );
    assert!(session.record(TraceEvent::new(
        12,
        42,
        1,
        CATEGORY_IPC_MESSAGES,
        TraceEventKind::Instant,
        "debugd:health_check"
    )));
    let bytes = session.stop(13);
    assert!(looks_like_perfetto_trace(&bytes));
    assert!(
        bytes
            .windows(b"debugd:health_check".len())
            .any(|w| w == b"debugd:health_check")
    );
}

#[test]
fn oneshot_buffer_stops_after_capacity() {
    let mut session = TraceSession::new(
        TraceConfig {
            categories: CATEGORY_DEBUG_SERVICE,
            buffer_mode: BufferMode::OneshotStopOnFull,
            buffer_size_kb: 1,
            output_format: TraceOutputFormat::LegacyBexosFxt,
        },
        1,
    );
    for n in 0..200 {
        session.record(TraceEvent::new(
            n,
            1,
            1,
            CATEGORY_DEBUG_SERVICE,
            TraceEventKind::Instant,
            "event",
        ));
    }
    assert!(session.dropped_count() > 0);
}

#[test]
fn exporter_has_stable_prefix() {
    let legacy = export_legacy_bexos_fxt(&[TraceEvent::metadata(1, "hello")]);
    assert!(looks_like_legacy_bexos_fxt(&legacy));
    let perfetto = export_perfetto_trace(&[TraceEvent::metadata(1, "hello")]);
    assert!(looks_like_perfetto_trace(&perfetto));
    assert!(!looks_like_legacy_bexos_fxt(&perfetto));
}

#[test]
fn shared_ring_records_filters_wraps_and_reports_drops() {
    let mut bytes = vec![0u8; HEADER_SIZE + SLOT_SIZE * 2];
    let writer = unsafe {
        SharedTraceWriter::from_raw_parts(bytes.as_mut_ptr(), bytes.len(), 7, 42, 99).unwrap()
    };
    writer.configure(
        &TraceConfig {
            categories: CATEGORY_DEBUG_SERVICE,
            buffer_mode: BufferMode::CircularRing,
            buffer_size_kb: 8,
            output_format: TraceOutputFormat::Perfetto,
        },
        1,
    );
    writer.record(
        TraceEventKind::Instant,
        CATEGORY_IPC_MESSAGES,
        "filtered",
        0,
        0,
    );
    writer.record(
        TraceEventKind::Instant,
        CATEGORY_DEBUG_SERVICE,
        "first",
        0,
        0,
    );
    writer.record(
        TraceEventKind::Counter,
        CATEGORY_DEBUG_SERVICE,
        "second",
        0,
        5,
    );
    writer.record(
        TraceEventKind::FlowBegin,
        CATEGORY_DEBUG_SERVICE,
        "third",
        12,
        0,
    );
    let reader = unsafe { SharedTraceReader::from_raw_parts(bytes.as_ptr(), bytes.len()).unwrap() };
    let events = reader.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name, "second");
    assert_eq!(events[0].value, 5);
    assert_eq!(events[1].name, "third");
    assert_eq!(events[1].flow_id, 12);
}

#[test]
fn shared_ring_oneshot_stops_on_full() {
    let mut bytes = vec![0u8; HEADER_SIZE + SLOT_SIZE];
    let writer = unsafe {
        SharedTraceWriter::from_raw_parts(bytes.as_mut_ptr(), bytes.len(), 1, 1, 1).unwrap()
    };
    writer.configure(
        &TraceConfig {
            categories: CATEGORY_DEBUG_SERVICE,
            buffer_mode: BufferMode::OneshotStopOnFull,
            buffer_size_kb: 4,
            output_format: TraceOutputFormat::Perfetto,
        },
        1,
    );
    writer.record(TraceEventKind::Instant, CATEGORY_DEBUG_SERVICE, "one", 0, 0);
    writer.record(TraceEventKind::Instant, CATEGORY_DEBUG_SERVICE, "two", 0, 0);
    assert_eq!(writer.dropped_count(), 1);
    let events =
        unsafe { SharedTraceReader::from_raw_parts(bytes.as_ptr(), bytes.len()).unwrap() }.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].name, "one");
}

#[test]
fn shared_ring_rejects_unconfigured_or_malformed_slots() {
    let bytes = vec![0u8; HEADER_SIZE + SLOT_SIZE];
    let reader = unsafe { SharedTraceReader::from_raw_parts(bytes.as_ptr(), bytes.len()).unwrap() };
    assert!(reader.events().is_empty());
    assert_eq!(MAX_PRODUCER_BUFFER_SIZE, 2 * 1024 * 1024);
}

#[test]
fn instrumentation_macros_compile_without_runtime_registration() {
    bexos_trace::trace_scope!("test:scope", "value" => 7u64);
    bexos_trace::trace_instant!("test:instant");
    bexos_trace::trace_counter!("test:counter", 1i64);
    bexos_trace::trace_flow_begin!("test:flow", 42u64);
    bexos_trace::trace_flow_step!("test:flow", 42u64);
    bexos_trace::trace_flow_end!("test:flow", 42u64);
}
