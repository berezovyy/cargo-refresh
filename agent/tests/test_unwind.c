#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <stdio.h>
#include <assert.h>
#include "unwind-copy.h"
static void put32(unsigned char *b,size_t p,uint32_t v){memcpy(b+p,&v,4);}
static uint32_t get32(unsigned char *b,size_t p){uint32_t v;memcpy(&v,b+p,4);return v;}
int main(void){
 unsigned char b[128]={0},original[128];size_t p=8;
 b[p++]=1;memcpy(b+p,"zPLR",5);p+=5;
 b[p++]=1;b[p++]=0x78;b[p++]=16;b[p++]=7;
 b[p++]=0x9b;size_t personality=p;put32(b,p,0x2000);p+=4;
 b[p++]=0x1b;size_t lsda_encoding=p-1;b[p++]=0x1b;
 while(p%4)b[p++]=0;
 put32(b,0,p-4);size_t fde=p;p+=8;put32(b,fde+4,fde+4);
 size_t pc=p;put32(b,p,0x1000);p+=4;size_t range=p;put32(b,p,0x100);p+=4;
 b[p++]=4;size_t lsda=p;put32(b,p,0x3000);p+=4;
 put32(b,fde,p-fde-4);memcpy(original,b,p);
 assert(rebase121(b,p,0x100)==0);
 assert(get32(b,personality)==0x1f00 && get32(b,pc)==0xf00 && get32(b,lsda)==0x2f00);
 assert(get32(b,range)==0x100 && get32(b,fde+4)==fde+4);
 memcpy(b,original,p);put32(b,lsda,0);assert(rebase121(b,p,0x100)==0);assert(get32(b,lsda)==0);
 memcpy(b,original,p);b[lsda_encoding]=0x3b;assert(rebase121(b,p,0x100)==-1);
 memcpy(b,original,p);assert(rebase121(b,p,INT64_C(0x100000000))==-1);
 memcpy(b,original,p);assert(rebase121(b,p-1,0x100)==-1);
 puts("unwind-copy: pcrel personality/PC/LSDA, null LSDA, invariant CIE/range, unsupported encoding, overflow, truncation PASS");
 return 0;
}
