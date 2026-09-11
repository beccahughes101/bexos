LOCAL_DIR := $(GET_LOCAL_DIR)
MODULE := $(LOCAL_DIR)
MODULE_SRCS += $(LOCAL_DIR)/worker.c $(LOCAL_DIR)/memory.c
MODULE_SRCS += $(LOCAL_DIR)/journal.c $(LOCAL_DIR)/root_worker.c
MODULE_SRCS += trusty/kernel/lib/trusty/tipc_dev_ql.c
MODULE_INCLUDES += trusty/kernel/lib/trusty trusty/kernel/lib/sm/include
MODULE_DEPS += trusty/kernel/lib/trusty trusty/kernel/lib/extmem
MODULE_DEPS += trusty/kernel/lib/ktipc
MODULE_DEPS += trusty/user/base/interface/smc
include make/module.mk
