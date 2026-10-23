#define _GNU_SOURCE
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
extern int f(void);
__asm__(".text\n.p2align 12\n.global f\n.type f,@function\nf:\nmov $1,%eax\nret\n.fill 26,1,0x90\n.size f,.-f\n");
static _Atomic int running=1,errors;
static void *busy(void *unused) {
    (void)unused;
    while(atomic_load(&running)) {
        int v=f();
        if(v!=1 && v!=2)atomic_fetch_add(&errors,1);
        /* Leave a short hazard-free interval without calling the allocator. */
        for(volatile unsigned k=0;k<100;k++){}
    }
    return NULL;
}
void fast_refresh_after_patch(void) {}
int main(int argc,char **argv) {
    if(argc>1 && argv[1][0]=='o'){printf("%d\n",f());return 0;}
    pthread_t threads[4];
    int n=argc>1?atoi(argv[1]):0;
    if(n<0 || n>4)return 2;
    for(int i=0;i<n;i++)if(pthread_create(&threads[i],NULL,busy,NULL))return 3;
    setvbuf(stdout,NULL,_IONBF,0);
    printf("ready %p\n",(void *)f);
    char c;
    while(read(STDIN_FILENO,&c,1)==1) {
        if(c=='q')break;
        if(c=='v')printf("value %d errors %d\n",f(),atomic_load(&errors));
    }
    atomic_store(&running,0);
    for(int i=0;i<n;i++)pthread_join(threads[i],NULL);
    return atomic_load(&errors)?4:0;
}
