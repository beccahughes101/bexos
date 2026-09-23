#include "include/bexos_package_state.h"
#include <assert.h>
#include <string.h>
int main(void) {
    uint8_t request[128] = {0}, old[128] = {0}, next[128] = {0};
    memcpy(request, "PKGSEC01", 8); bexos_package_put32(request, 8, 2); bexos_package_put32(request, 88, 32);
    bexos_package_put64(request, 56, 5); bexos_package_put64(request, 80, 100);
    assert(!bexos_package_next(old, 0, request, sizeof(request), next));
    assert(bexos_package_u64(next, 48) == 1);
    memcpy(old, next, sizeof(old));
    assert(bexos_package_next(old, sizeof(old), request, sizeof(request), next) == 2);
    bexos_package_put64(request, 48, 1); bexos_package_put64(request, 56, 4);
    assert(bexos_package_next(old, sizeof(old), request, sizeof(request), next) == 2);
    bexos_package_put64(request, 56, 5); bexos_package_put64(request, 80, 99);
    assert(bexos_package_next(old, sizeof(old), request, sizeof(request), next) == 2);
    bexos_package_put64(request, 80, 100);
    assert(!bexos_package_next(old, sizeof(old), request, sizeof(request), next));
    bexos_package_put64(old, 64, 1000);
    assert(bexos_package_next(old, sizeof(old), request, sizeof(request), next) == 2);
    bexos_package_put64(request, 56, 6);
    assert(!bexos_package_next(old, sizeof(old), request, sizeof(request), next));
    bexos_package_put64(request, 80, 99);
    assert(bexos_package_next(old, sizeof(old), request, sizeof(request), next) == 2);
    bexos_package_put64(request, 80, 100);
    assert(!bexos_package_valid(request, 95));
    request[16] = 1;
    assert(bexos_package_next(old, sizeof(old), request, sizeof(request), next) == 4);
    return 0;
}
