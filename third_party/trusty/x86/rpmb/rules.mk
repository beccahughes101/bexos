LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_SRCS += $(LOCAL_DIR)/proxy.c
MODULE_DEPS += trusty/kernel/lib/ktipc
include make/module.mk
