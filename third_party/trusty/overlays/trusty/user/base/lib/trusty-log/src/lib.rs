/*
 * Copyright (C) 2021 The Android Open Source Project
 * Copyright 2026 The BexOS Authors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *      http://www.apache.org/licenses/LICENSE-2.0
 */

//! Trusty simple logger backend.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};
use log::{Level, Log, Metadata, Record};
use std::io::{stderr, Write};

type FormatFn = Box<dyn Fn(&log::Record) -> String + Sync + Send>;

pub struct TrustyLoggerConfig {
    log_level: log::Level,
    custom_format: Option<FormatFn>,
}

impl TrustyLoggerConfig {
    pub const fn new() -> Self {
        Self { log_level: Level::Info, custom_format: None }
    }

    pub fn with_min_level(mut self, level: log::Level) -> Self {
        self.log_level = level;
        self
    }

    pub fn format<F>(mut self, format: F) -> Self
    where
        F: Fn(&log::Record) -> String + Sync + Send + 'static,
    {
        self.custom_format = Some(Box::new(format));
        self
    }
}

impl Default for TrustyLoggerConfig {
    fn default() -> Self {
        Self::new()
    }
}

struct StaticLogger {
    // 0 = uninitialized, 1 = initializing, 2 = registered, 3 = registration failed.
    state: AtomicU8,
    config: UnsafeCell<MaybeUninit<TrustyLoggerConfig>>,
}

// Configuration is published exactly once before state becomes 2 and is then
// immutable for the process lifetime.
unsafe impl Sync for StaticLogger {}

impl StaticLogger {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            config: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    fn config(&self) -> Option<&TrustyLoggerConfig> {
        if self.state.load(Ordering::Acquire) != 2 {
            return None;
        }
        // SAFETY: state 2 is stored with Release only after the configuration
        // has been initialized, and the configuration is never mutated again.
        Some(unsafe { (&*self.config.get()).assume_init_ref() })
    }
}

impl Log for StaticLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.config().is_some_and(|config| metadata.level() <= config.log_level)
    }

    fn log(&self, record: &Record) {
        let Some(config) = self.config() else {
            return;
        };
        if record.metadata().level() > config.log_level {
            return;
        }
        let message = match &config.custom_format {
            Some(format) => format(record),
            None => default_log_function(record),
        };
        let _ = stderr().write(message.as_bytes());
    }

    fn flush(&self) {}
}

fn default_log_function(record: &Record) -> String {
    format!("{} - {}\n", record.level(), record.args())
}

static LOGGER: StaticLogger = StaticLogger::new();

pub fn init() {
    if !try_init_with_config(TrustyLoggerConfig::default()) {
        let _ = stderr().write(b"trusty-log: logger registration failed\n");
    }
}

pub fn init_with_config(config: TrustyLoggerConfig) {
    if !try_init_with_config(config) {
        let _ = stderr().write(b"trusty-log: logger registration failed\n");
    }
}

/// Registers the process logger without allocation leaks or panic-on-failure.
/// Repeated calls after successful registration are harmless.
pub fn try_init_with_config(config: TrustyLoggerConfig) -> bool {
    match LOGGER.state.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(2) => return true,
        Err(_) => return false,
    }

    let level = config.log_level.to_level_filter();
    // SAFETY: this thread owns the 0 -> 1 transition and no reader accesses
    // the value until state is published as 2.
    unsafe { (*LOGGER.config.get()).write(config) };
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
        LOGGER.state.store(2, Ordering::Release);
        true
    } else {
        // SAFETY: registration failed, so the log facade cannot hold LOGGER,
        // and this thread still exclusively owns the initialized value.
        unsafe { (*LOGGER.config.get()).assume_init_drop() };
        LOGGER.state.store(3, Ordering::Release);
        false
    }
}
