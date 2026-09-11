static _Thread_local unsigned long long value = 17;
static _Thread_local unsigned long long zero;
static unsigned int constructors;
__attribute__((constructor)) static void initialize(void) { constructors++; }
unsigned long long fixture_read(void) { return value + zero; }
void fixture_write(unsigned long long next) { value = next; zero = 3; }
unsigned int fixture_constructors(void) { return constructors; }
