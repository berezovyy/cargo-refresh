#ifndef REFRESH_UNWIND_H
#define REFRESH_UNWIND_H
#include "protocol.h"
#define REFRESH_NEAR_SLOTS 512
size_t unwind_bytes122(void);
void unwind_init(void);
void discard_staged(void);
enum refresh_code prepare_frames(char error[160]);
void register_staged(void);
void retain_staged(void);
#ifdef REFRESH_TESTING
void forget_active_frame123(int index);
#endif
#endif
