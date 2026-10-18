/* Pactern pure guest ABI v1. Freestanding C11; no libc or ambient imports.
 * Immutable values reference a nonmoving heap. Every live compiler slot is an
 * explicit root; mark/sweep never scans machine words conservatively. */
typedef unsigned int U32;
typedef unsigned long long U64;
typedef long long I64;
typedef struct { I64 n; U32 tag, ref; } V;
typedef struct { U32 size, used, mark, kind, len, aux; } Obj;
typedef struct { U32 id, pad; V value; } Field;
typedef struct Roots { struct Roots *previous; V *values; U32 count; } Roots;
#ifndef PT_HEAP_SIZE
#define PT_HEAP_SIZE (4 * 1024 * 1024)
#endif
static union { U64 alignment; unsigned char data[PT_HEAP_SIZE]; } heap;
static Roots *roots;
static U64 fuel;
static U32 depth, max_depth, trap_code, trap_cause, guard_depth, collections, active_slots;
static unsigned char output[1024 * 1024];
static U32 output_len;
static V unit(void) { return (V){0,0,0}; }
static V scalar(U32 tag, I64 n) { return (V){n,tag,0}; }
static void fail(U32 code) {
    if (!trap_code) {
        trap_code = guard_depth ? 9 : code;
        trap_cause = guard_depth ? code : 0;
    }
}
static void tick(void) { if (!fuel) fail(4); else --fuel; }
static Obj *object(V v) { return (Obj *)(heap.data + v.ref); }
static V *items(V v) { return (V *)(object(v) + 1); }
static Field *fields(V v) { return (Field *)(object(v) + 1); }
static void charge(V v,U32 level) {
    if(trap_code)return;
    if(level>64){fail(4);return;}
    tick();if(trap_code)return;
    if(v.tag==3){U32 len=object(v)->len;if(fuel<len){fail(4);return;}fuel-=len;}
    if(v.tag==4||v.tag==6)for(U32 i=0;i<object(v)->len;++i)charge(items(v)[i],level+1);
    if(v.tag==5)for(U32 i=0;i<object(v)->len;++i)charge(fields(v)[i].value,level+1);
}
static void mark(V v, U32 level) {
    if (v.tag < 3 || v.tag == 7 || !v.ref || trap_code) return;
    if (level > 64) { fail(4); return; }
    Obj *o = object(v);
    if (o->mark) return;
    o->mark = 1;
    if (v.tag == 4 || v.tag == 6)
        for (U32 i=0;i<o->len;++i) mark(items(v)[i],level+1);
    if (v.tag == 5)
        for (U32 i=0;i<o->len;++i) mark(fields(v)[i].value,level+1);
}
static void collect(void) {
    ++collections;
    for (Roots *r=roots;r;r=r->previous)
        for (U32 i=0;i<r->count;++i) mark(r->values[i],0);
    for (U32 p=8;p<PT_HEAP_SIZE;) {
        Obj *o=(Obj *)(heap.data+p);
        if (!o->mark) o->used=0;
        o->mark=0;
        p+=o->size;
    }
    for (U32 p=8;p<PT_HEAP_SIZE;) {
        Obj *o=(Obj *)(heap.data+p);
        while (!o->used && p+o->size<PT_HEAP_SIZE) {
            Obj *next=(Obj *)(heap.data+p+o->size);
            if (next->used) break;
            o->size+=next->size;
        }
        p+=o->size;
    }
}
static V allocate(U32 tag, U32 len, U32 aux) {
    if (trap_code) return unit();
    U64 bytes=(U64)len*(tag==3?1:tag==5?sizeof(Field):sizeof(V));
    if (bytes>PT_HEAP_SIZE-sizeof(Obj)-16) { fail(4); return unit(); }
    U32 need=(U32)(sizeof(Obj)+bytes+7)&~7u;
    for (U32 attempt=0;attempt<2;++attempt) {
        for (U32 p=8;p<PT_HEAP_SIZE;) {
            Obj *o=(Obj *)(heap.data+p);
            if (!o->used && o->size>=need) {
                if (o->size>=need+sizeof(Obj)+8) {
                    Obj *next=(Obj *)(heap.data+p+need);
                    *next=(Obj){o->size-need,0,0,0,0,0};
                    o->size=need;
                }
                o->used=1;o->mark=0;o->kind=tag;o->len=len;o->aux=aux;
                for (U32 i=sizeof(Obj);i<o->size;++i) heap.data[p+i]=0;
                return (V){0,tag,p};
            }
            p+=o->size;
        }
        collect();
    }
    fail(4); return unit();
}
static V string(const unsigned char *s,U32 len) {
    V v=allocate(3,len,0);if(trap_code)return unit();
    unsigned char *p=(unsigned char *)(object(v)+1);
    for(U32 i=0;i<len;++i)p[i]=s[i];return v;
}
static int equal(V a,V b,U32 level) {
    if(level>64){fail(4);return 0;} if(a.tag!=b.tag)return 0;
    if(a.tag<3)return a.n==b.n;
    Obj *x=object(a),*y=object(b);
    if(x->len!=y->len||x->aux!=y->aux)return 0;
    for(U32 i=0;i<x->len;++i) {
        if(a.tag==3) {if(((unsigned char *)(x+1))[i]!=((unsigned char *)(y+1))[i])return 0;}
        else if(a.tag==5) {
            Field f=fields(a)[i];int found=0;
            for(U32 j=0;j<y->len;++j)if(fields(b)[j].id==f.id){found=equal(f.value,fields(b)[j].value,level+1);break;}
            if(!found)return 0;
        } else if(!equal(items(a)[i],items(b)[i],level+1))return 0;
    }
    return 1;
}
static V field(V a,U32 id) {
    for(U32 i=0;i<object(a)->len;++i)if(fields(a)[i].id==id)return fields(a)[i].value;
    fail(6);return unit();
}
static V index_value(V a,V i) {
    if(i.n<0||(U64)i.n>=object(a)->len){fail(3);return unit();}
    return items(a)[i.n];
}
static V unary(U32 op,V a) {
    if(op==0)return scalar(1,!a.n);
    I64 v;if(__builtin_sub_overflow((I64)0,a.n,&v)){fail(1);return unit();}return scalar(2,v);
}
static V binary(U32 op,V a,V b) {
    I64 v=0;int overflow=0;
    switch(op){
    case 0:overflow=__builtin_add_overflow(a.n,b.n,&v);break;
    case 1:overflow=__builtin_sub_overflow(a.n,b.n,&v);break;
    case 2:{
        U64 aa=a.n<0?0-(U64)a.n:(U64)a.n;
        U64 bb=b.n<0?0-(U64)b.n:(U64)b.n;
        int negative=(a.n<0)!=(b.n<0);
        U64 maximum=negative?(1ULL<<63):((1ULL<<63)-1);
        overflow=bb&&aa>maximum/bb;
        if(!overflow){U64 product=aa*bb;v=(I64)(negative?0-product:product);}
        break;
    }
    case 3:case 4:
        if(!b.n){fail(2);return unit();}
        if(a.n==(-9223372036854775807LL-1)&&b.n==-1){fail(1);return unit();}
        v=op==3?a.n/b.n:a.n%b.n;break;
    case 5:return scalar(1,equal(a,b,0));
    case 6:return scalar(1,!equal(a,b,0));
    case 7:return scalar(1,a.n<b.n);
    case 8:return scalar(1,a.n<=b.n);
    case 9:return scalar(1,a.n>b.n);
    case 10:return scalar(1,a.n>=b.n);
    case 11:return scalar(1,a.n&&b.n);
    case 12:return scalar(1,a.n||b.n);
    default:fail(6);return unit();}
    if(overflow){fail(1);return unit();}return scalar(2,v);
}
/* Compact bounded wire representation: u8 tag, i64 little endian for scalars;
 * objects have u32 length, variants an additional u32 symbol, records u32 keys. */
static void byte(U32 b){if(output_len>=sizeof(output)){fail(4);return;}output[output_len++]=(unsigned char)b;}
static void word(U64 n,U32 count){for(U32 i=0;i<count;++i){byte((U32)n);n>>=8;}}
static void encode(V v,U32 level){
    if(level>64){fail(4);return;}if(v.tag>6){fail(6);return;}byte(v.tag);
    if(v.tag==1||v.tag==2){word((U64)v.n,8);return;}
    if(v.tag==0)return;
    Obj *o=object(v);word(o->len,4);if(v.tag==6)word(o->aux,4);
    for(U32 i=0;i<o->len;++i){
        if(v.tag==3)byte(((unsigned char *)(o+1))[i]);
        else if(v.tag==5){word(fields(v)[i].id,4);encode(fields(v)[i].value,level+1);}
        else encode(items(v)[i],level+1);
    }
}
static void reset(U64 budget,U32 calls){
    roots=0;fuel=budget;depth=0;max_depth=calls;trap_code=0;trap_cause=0;guard_depth=0;collections=0;output_len=0;active_slots=0;
    *(Obj *)(heap.data+8)=(Obj){PT_HEAP_SIZE-8,0,0,0,0,0};
}
unsigned int pt_abi_version(void){return 1;}
unsigned int pt_output_ptr(void){return (U32)(unsigned long)output;}
unsigned int pt_output_len(void){return output_len;}
unsigned int pt_trap_cause(void){return trap_cause;}
unsigned int pt_collections(void){return collections;}
