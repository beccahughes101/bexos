/*
 * Copyright (C) 2025 The Android Open Source Project
 * Copyright 2026 The BexOS Authors
 * SPDX-License-Identifier: Apache-2.0
 */

use std::io::{stderr, Write};

fn main() {
    if !trusty_log::try_init_with_config(trusty_log::TrustyLoggerConfig::default()) {
        let _ = stderr().write(b"AuthMgr-FE: logger initialization failed\n");
        return;
    }
    log::info!("Hello from AuthMgr-FE!");
    if let Err(error) = authmgr_fe::init_and_start_loop() {
        log::error!("AuthMgr-FE stopped during initialization: {:?}", error);
    }
}
