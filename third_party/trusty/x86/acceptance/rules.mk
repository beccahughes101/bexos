LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_CRATE_NAME := bexos_security_acceptance
MODULE_SRCS += $(LOCAL_DIR)/main.rs
MANIFEST := $(LOCAL_DIR)/manifest.json
MODULE_LIBRARY_DEPS += \
    trusty/user/base/lib/keymint-rust/wire \
    trusty/user/base/lib/tipc/rust \
    trusty/user/base/lib/storage/rust \
    trusty/user/base/lib/trusty-log \
    trusty/user/base/lib/trusty-sys \
    trusty/user/base/lib/trusty-std \
    $(call FIND_CRATE,log)
include make/trusted_app.mk
