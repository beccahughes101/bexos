LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MANIFEST := $(LOCAL_DIR)/manifest.json
MODULE_SRCS += $(LOCAL_DIR)/main.rs
MODULE_CRATE_NAME := bexos_authmgr_acceptance
MODULE_LIBRARY_DEPS += \
    trusty/user/base/lib/keymint-rust/wire \
    trusty/user/base/lib/trusty-sys \
    frameworks/native/libs/binder/trusty/rust \
    frameworks/native/libs/binder/trusty/rust/rpcbinder \
    trusty/user/app/sample/rust-hello-world-trusted-hal/aidl \
    trusty/user/base/interface/authmgr/rust \
    trusty/user/base/lib/service_manager/client \
    trusty/user/base/lib/tipc/rust \
    trusty/user/base/lib/trusty-log \
    trusty/user/base/lib/trusty-std \
    $(call FIND_CRATE,log) \

include make/trusted_app.mk
