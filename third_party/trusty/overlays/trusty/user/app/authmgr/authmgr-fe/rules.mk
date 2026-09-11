# Copyright (C) 2025 The Android Open Source Project
# Copyright 2026 The BexOS Authors
# SPDX-License-Identifier: Apache-2.0

LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_SRCS += $(LOCAL_DIR)/lib.rs
MODULE_CRATE_NAME := authmgr_fe

MODULE_LIBRARY_DEPS += \
	$(call FIND_CRATE,ciborium) \
	$(call FIND_CRATE,coset) \
	$(call FIND_CRATE,log) \
	frameworks/native/libs/binder/trusty/rust \
	frameworks/native/libs/binder/trusty/rust/rpcbinder \
	packages/modules/Virtualization/libs/dice/open_dice \
	trusty/user/base/interface/authmgr/rust \
	trusty/user/base/interface/binder_accessor \
	trusty/user/base/interface/secure_storage/rust \
	trusty/user/base/lib/authgraph-rust/boringssl \
	trusty/user/base/lib/authgraph-rust/core \
	trusty/user/base/lib/authmgr-common-rust \
	trusty/user/base/lib/authmgr-common-util-rust \
	trusty/user/base/lib/hwbcc/rust \
	trusty/user/base/lib/secretkeeper/dice_policy \
	trusty/user/base/lib/secretkeeper/dice-policy-builder \
	trusty/user/base/lib/service_manager/client \
	trusty/user/base/lib/tipc/rust \
	trusty/user/base/lib/trusty-log \
	trusty/user/base/lib/trusty-std \

ifeq (true,$(call TOBOOL,$(AUTHMGRFE_MODE_INSECURE)))
MODULE_RUSTFLAGS += --cfg 'feature="authmgrfe_mode_insecure"'
endif

MODULE_RUST_TESTS := true
MANIFEST := $(LOCAL_DIR)/manifest.json
include make/library.mk
