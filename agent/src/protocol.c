#include "internal.h"
#include "protocol.h"
#include "park.h"
#include "hazard.h"
#include "apply.h"
#include "unwind.h"
int fullread(int fd,void *p,size_t len) {
    while(len){ssize_t n=read(fd,p,len);if(n<=0)return -1;p=(char*)p+n;len-=n;}return 0;
}
double stamp(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec*1e6+t.tv_nsec/1e3;}

struct rec recs[MAX_RECORDS];
int nrec;
