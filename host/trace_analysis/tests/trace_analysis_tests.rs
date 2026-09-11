use std::path::PathBuf;

use bexos_trace::{
    CATEGORY_APP_CUSTOM, TraceAnnotation, TraceEvent, TraceEventKind, TraceOutputFormat,
};
use bexos_trace_analysis::TraceAnalysis;

#[test]
fn analyzes_perfetto_exported_event_bytes() {
    let mut event = TraceEvent::new(
        10,
        42,
        7,
        CATEGORY_APP_CUSTOM,
        TraceEventKind::Instant,
        "appd:startup_wave",
    );
    event.annotations[0] = Some(TraceAnnotation {
        key: "wave".into(),
        value: 2,
    });
    let bytes = TraceOutputFormat::Perfetto.export(&[event]);
    let analysis = TraceAnalysis::from_bytes(PathBuf::from("startup.pftrace"), bytes)
        .expect("perfetto bytes should load");

    analysis
        .assert_event_present("appd:startup_wave")
        .expect("event should be present");
    analysis
        .assert_category_present("app_custom")
        .expect("category should be present");
}

#[test]
fn analyzes_legacy_exported_event_bytes() {
    let event = TraceEvent::metadata(10, "debugd:request");
    let bytes = TraceOutputFormat::LegacyBexosFxt.export(&[event]);
    let analysis = TraceAnalysis::from_bytes(PathBuf::from("debugd.fxt"), bytes)
        .expect("legacy bytes should load");

    analysis
        .assert_event_present("debugd:request")
        .expect("event should be present");
}

#[test]
fn rejects_unknown_trace_bytes() {
    let error = TraceAnalysis::from_bytes(PathBuf::from("bad.trace"), b"not a trace".to_vec())
        .expect_err("invalid trace should fail");
    assert!(error.contains("neither Perfetto"));
}
