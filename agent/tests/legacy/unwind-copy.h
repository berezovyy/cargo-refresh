// EXP-121: Copy contiguous CIE/FDE records to an agent-owned near mapping and
// append a terminator. Rebase every supported PC-relative field; absolute
// pointers already received the normal PIE fixups. CIE backward offsets and
// intra-table references remain unchanged. Unsupported encodings fail preflight.
struct cie121 {size_t at;unsigned fde,lsda;int z;};
static int leb121(const unsigned char *b,size_t end,size_t *p,uint64_t *v){
    unsigned shift=0;*v=0;
    while(*p<end && shift<64){unsigned c=b[(*p)++];*v|=(uint64_t)(c&127)<<shift;if(!(c&128))return 0;shift+=7;}return -1;
}
static int pointer121(unsigned char *b,size_t end,size_t *p,unsigned enc,int64_t delta,int relocate){
    if(enc==255)return 0;
    unsigned fmt=enc&15,app=enc&0x70;size_t n=fmt==0?8:(fmt==3||fmt==11)?4:(fmt==4||fmt==12)?8:0;
    if(!n || *p+n>end || (app && app!=0x10) || (app==0x10 && fmt!=11 && fmt!=12))return -1;
    if(relocate && app==0x10){
        if(n==4){int32_t old;memcpy(&old,b+*p,4);if(!old){*p+=n;return 0;}int64_t next=(int64_t)old-delta;
            if(next<INT32_MIN || next>INT32_MAX)return -1;
            int32_t val=next;memcpy(b+*p,&val,4);
        }else{int64_t old;memcpy(&old,b+*p,8);if(!old){*p+=n;return 0;}old-=delta;memcpy(b+*p,&old,8);}
    }
    *p+=n;return 0;
}
static int rebase121(unsigned char *b,size_t size,int64_t delta){
    struct cie121 cie[1024];int nc=0;size_t at=0;
    while(at<size){
        uint32_t len,back;if(at+8>size)return -1;memcpy(&len,b+at,4);memcpy(&back,b+at+4,4);
        if(len<4 || len==UINT32_MAX || len>size-at-4)return -1;
        size_t end=at+4+len,p=at+8;uint64_t v;
        if(!back){
            if(nc==1024 || p>=end)return -1;
            unsigned version=b[p++];if(version!=1 && version!=3)return -1;
            size_t aug=p;while(p<end && b[p])p++;if(p==end)return -1;p++;
            if(leb121(b,end,&p,&v)||leb121(b,end,&p,&v))return -1;
            if(version==1){if(p==end)return -1;p++;}else if(leb121(b,end,&p,&v))return -1;
            struct cie121 c={at,0,255,b[aug]=='z'};
            if(!c.z && b[aug])return -1;
            if(c.z){
                if(leb121(b,end,&p,&v)||v>end-p)return -1;
                size_t ae=p+v;
                for(size_t q=aug+1;b[q];q++){
                    if(b[q]=='L'){if(p==ae)return -1;c.lsda=b[p++];}
                    else if(b[q]=='R'){if(p==ae)return -1;c.fde=b[p++];}
                    else if(b[q]=='P'){if(p==ae)return -1;unsigned enc=b[p++];if(pointer121(b,ae,&p,enc,delta,1))return -1;}
                    else if(b[q]!='S')return -1;
                }
                if(p!=ae)return -1;
            }
            cie[nc++]=c;
        }else{
            if(back>at+4)return -1;
            size_t ca=at+4-back;int j;for(j=0;j<nc;j++)if(cie[j].at==ca)break;if(j==nc)return -1;
            struct cie121 *c=&cie[j];
            if(pointer121(b,end,&p,c->fde,delta,1)||pointer121(b,end,&p,c->fde&15,delta,0))return -1;
            if(c->z){if(leb121(b,end,&p,&v)||v>end-p)return -1;
                size_t ae=p+v;
                if(v && c->lsda!=255 && pointer121(b,ae,&p,c->lsda,delta,1))return -1;
                if(p!=ae)return -1;
            }
        }
        at=end;
    }
    return at==size?0:-1;
}
