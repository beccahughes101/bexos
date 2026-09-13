LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MANIFEST := $(LOCAL_DIR)/manifest.json
MODULE_INCLUDES += $(LOCAL_DIR)/include
MODULE_SRCS += $(LOCAL_DIR)/main.c $(LOCAL_DIR)/state.c
MODULE_SRCS += $(LOCAL_DIR)/journal_state.c $(LOCAL_DIR)/journal_storage.c
MODULE_SRCS += $(LOCAL_DIR)/boot_selection.c $(LOCAL_DIR)/boot_selection_storage.c
MODULE_SRCS += $(LOCAL_DIR)/package_state.c $(LOCAL_DIR)/package_storage.c
MODULE_LIBRARY_DEPS += trusty/user/base/lib/libc-trusty trusty/user/base/lib/tipc
MODULE_LIBRARY_DEPS += trusty/user/base/lib/storage
include make/trusted_app.mk
