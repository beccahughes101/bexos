#ifndef BEXOS_ORCHESTRATOR_PROTOCOL_H
#define BEXOS_ORCHESTRATOR_PROTOCOL_H

#include <stdint.h>

#define BEXOS_ORCHESTRATOR_PORT "com.bexos.orchestrator"
#define BEXOS_ORCHESTRATOR_PROTOCOL_VERSION 1u
#define BEXOS_ORCHESTRATOR_MAX_MESSAGE_SIZE 64u

enum bexos_orchestrator_command {
    BEXOS_ORCHESTRATOR_MARK_COMPONENT_FAILED = 0x200u,
    BEXOS_ORCHESTRATOR_GET_ACTIVE_COMPONENT = 0x201u,
    BEXOS_ORCHESTRATOR_GET_KERNEL_SLOT = 0x202u,
    BEXOS_ORCHESTRATOR_GET_TRUSTY_GENERATION = 0x203u,
    BEXOS_ORCHESTRATOR_GET_MIGRATION_ABI = 0x204u,
    BEXOS_ORCHESTRATOR_GET_FIXTURE_MODE = 0x205u,
};

enum bexos_orchestrator_component {
    BEXOS_COMPONENT_KERNEL = 1u,
    BEXOS_COMPONENT_D1_DRIVER = 2u,
    BEXOS_COMPONENT_D2_DRIVER = 3u,
};

enum bexos_orchestrator_slot { BEXOS_SLOT_A = 1u, BEXOS_SLOT_B = 2u };

enum bexos_orchestrator_status {
    BEXOS_ORCHESTRATOR_OK = 0u,
    BEXOS_ORCHESTRATOR_BAD_VERSION = 1u,
    BEXOS_ORCHESTRATOR_BAD_MESSAGE = 2u,
    BEXOS_ORCHESTRATOR_NOT_FOUND = 3u,
    BEXOS_ORCHESTRATOR_UNSUPPORTED = 4u,
};

struct bexos_orchestrator_request {
    uint32_t version;
    uint32_t command;
    uint32_t component;
    uint32_t reserved;
};

struct bexos_orchestrator_response {
    uint32_t version;
    uint32_t status;
    uint32_t active_slot;
    uint32_t reserved;
};

#endif
