// Exercise C++ compilation and C linkage without a host C++ runtime.
namespace {
constexpr unsigned long pointer_bits = sizeof(void*) * 8;
static_assert(pointer_bits == 64, "guest must use a 64-bit ABI");
}
extern "C" unsigned long architecture_cpp_probe() { return pointer_bits; }
