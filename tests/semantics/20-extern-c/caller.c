#include <stdint.h>
uint64_t c_call(uint64_t (*f)(void)) { return f(); }
