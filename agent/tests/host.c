#define _GNU_SOURCE
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <locale.h>
#ifndef F_VALUE
#define F_VALUE 1
#endif
#ifndef RO_VALUE
#define RO_VALUE 17
#endif
#define STR_(x) #x
#define STR(x) STR_(x)
/* Common host symbols must never interpose on agent-private state. */
int mode=99;
extern int cross_f(void);
extern const int cross_ro;
__asm__(".text\n.p2align 12\n.fill 4094,1,0x90\n.global cross_f\n.type cross_f,@function\ncross_f:\nmov $1,%eax\nret\n.size cross_f,.-cross_f\n");
__asm__(".section .rodata\n.p2align 12\n.fill 4094,1,0\n.global cross_ro\ncross_ro:\n.long 17\n.previous\n");
extern int f(void);
extern int g(void);
extern const int ro_word;
static int ro(void) { return *(volatile const int *)&ro_word; }
__asm__(".section .rodata\n.p2align 12\n.global ro_word\nro_word:\n.long " STR(RO_VALUE) "\n.previous\n");
__asm__(".text\n.global g\n.type g,@function\ng:\nmov $1,%eax\nret\n.fill 26,1,0x90\n.size g,.-g\n");
__asm__(".text\n.p2align 12\n.global f\n.type f,@function\nf:\nmov $" STR(F_VALUE) ",%eax\nret\n.fill 26,1,0x90\n.size f,.-f\n");
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
    if(argc>1 && argv[1][0]=='o'){printf("%d %d\n",f(),ro());return 0;}
    pthread_t threads[4];
    int n=argc>1?atoi(argv[1]):0;
    if(n<0 || n>4)return 2;
    for(int i=0;i<n;i++)if(pthread_create(&threads[i],NULL,busy,NULL))return 3;
    setvbuf(stdout,NULL,_IONBF,0);
    printf("ready %p %p %p %p %p\n",(void *)f,(const void *)&ro_word,(void *)g,(void *)cross_f,(const void *)&cross_ro);
    char c;
    while(read(STDIN_FILENO,&c,1)==1) {
        if(c=='q')break;
        if(c=='l'){
            const char *name=getenv("REFRESH_TEST_LOCALE");
            if(!name || !setlocale(LC_NUMERIC,name))puts("locale unavailable");
            else printf("locale %s\n",localeconv()->decimal_point);
        }
        if(c=='n')printf("locale %s\n",localeconv()->decimal_point);
        if(c=='c'){unsetenv("REFRESH_TESTING_FAIL_AT");unsetenv("REFRESH_TESTING_FAIL_PHASE");puts("cleared");}
        if(c=='x')printf("cross %d\n",cross_f());
        if(c=='z')printf("cross_ro %d\n",*(volatile const int *)&cross_ro);
        if(mode!=99)return 5;
        if(c=='r')printf("ro %d\n",ro());
        if(c=='g')printf("g %d\n",g());
        if(c=='v')printf("value %d errors %d\n",f(),atomic_load(&errors));
    }
    atomic_store(&running,0);
    for(int i=0;i<n;i++)pthread_join(threads[i],NULL);
    return atomic_load(&errors)?4:0;
}
