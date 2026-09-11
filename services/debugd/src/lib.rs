#![allow(async_fn_in_trait)]

extern crate alloc;

pub mod processes;
pub mod service;
pub mod transport;

pub use service::{
    AppManager, BufferedAppManager, BufferedTeeManager, BufferedTestAppInstaller,
    BufferedTraceManager, BufferedUpdateManager, BufferedUserManager, LiveTraceManager,
    PlatformUpdateApplier, TeeManager, TraceManager, TracedTraceManager, UnsupportedAppManager,
    UnsupportedTeeManager, UnsupportedTraceManager, UnsupportedUpdateManager,
    UnsupportedUserManager, UserManager, VERSION, handle_frame, handle_frame_with_all_backends,
    handle_frame_with_backends, handle_frame_with_installer, handle_frame_with_trace_backends,
};

pub mod shell_auth;
