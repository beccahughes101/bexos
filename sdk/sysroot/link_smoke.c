#include <stddef.h>
#include <string.h>

int main(void) {
    const unsigned char source[] = {0x42, 0x45, 0x58, 0x00};
    unsigned char destination[sizeof(source)] = {0};
    memcpy(destination, source, sizeof(source));
    return destination[0] == source[0] ? 0 : 1;
}
