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
/* Explicit terminal ABI 1. The embedding host grants one scoped terminal. */
__attribute__((import_module("pactern.terminal.v1"),import_name("root")))
U32 pt_terminal_root(U32 slot);
__attribute__((import_module("pactern.terminal.v1"),import_name("release")))
void pt_terminal_release(U32 token);
__attribute__((import_module("pactern.terminal.v1"),import_name("argc")))
U32 pt_terminal_argc(U32 token);
__attribute__((import_module("pactern.terminal.v1"),import_name("arg_len")))
U32 pt_terminal_arg_len(U32 token,U32 index);
__attribute__((import_module("pactern.terminal.v1"),import_name("arg_copy")))
void pt_terminal_arg_copy(U32 token,U32 index,U32 ptr,U32 len);
__attribute__((import_module("pactern.terminal.v1"),import_name("read_line")))
int pt_terminal_read_line(U32 token,U32 ptr,U32 capacity);
__attribute__((import_module("pactern.terminal.v1"),import_name("write")))
void pt_terminal_write(U32 token,U32 stream,U32 ptr,U32 len);

static V terminal_args(U32 token) {
    V r[2]={0}; Roots frame={roots,r,2}; roots=&frame;
    U32 count=pt_terminal_argc(token);
    r[0]=allocate(4,count,0);
    if(trap_code)goto done;
    for(U32 i=0;i<count;++i) {
        tick(); if(trap_code)goto done;
        U32 len=pt_terminal_arg_len(token,i);
        r[1]=allocate(3,len,0);
        if(trap_code)goto done;
        pt_terminal_arg_copy(token,i,(U32)(unsigned long)(object(r[1])+1),len);
        items(r[0])[i]=r[1];
    }
done: roots=frame.previous; return r[0];
}

static unsigned char terminal_input[65536];
static V terminal_read_line(U32 token,U32 some,U32 none) {
    V r[2]={0}; Roots frame={roots,r,2}; roots=&frame;
    int len=pt_terminal_read_line(token,(U32)(unsigned long)terminal_input,65536);
    if(len < -1 || len > 65536){fail(6);goto done;}
    if(len == -1) {
        r[1]=allocate(6,0,none);
    } else {
        r[0]=string(terminal_input,(U32)len);
        if(trap_code)goto done;
        r[1]=allocate(6,1,some);
        if(!trap_code)items(r[1])[0]=r[0];
    }
done: roots=frame.previous; return r[1];
}
static V f26();
static V f27();
static V f28(V a29);
static V f30();
static V f31(V a32);
static V f26() {
 if(depth>=max_depth || active_slots>32768-44){fail(4);return unit();} ++depth; active_slots+=44;
 V r[44]={0}; Roots frame={roots,r,44}; roots=&frame;
tick(); if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[43]=string((const unsigned char*)"\110\145\154\154\157\040\120\141\143\164\145\162\156\012\012\040\040\061\040\040\123\150\157\167\040\164\150\145\040\141\156\163\167\145\162\012\040\040\062\040\040\110\145\154\160\012\040\040\060\040\040\105\170\151\164\012",56);
charge(r[43],0);
if(trap_code)goto cleanup;
r[42]=r[43];
goto returned;
goto returned; returned:;
cleanup:;
roots=frame.previous; --depth; active_slots-=44;
return r[42];

}
static V f27() {
 if(depth>=max_depth || active_slots>32768-44){fail(4);return unit();} ++depth; active_slots+=44;
 V r[44]={0}; Roots frame={roots,r,44}; roots=&frame;
tick(); if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[43]=string((const unsigned char*)"\110\145\154\154\157\040\120\141\143\164\145\162\156\012\012\125\163\141\147\145\072\040\160\141\143\164\145\162\156\040\162\165\156\040\055\055\164\145\162\155\151\156\141\154\040\055\055\040\133\103\117\115\115\101\116\104\135\012\012\122\165\156\040\167\151\164\150\157\165\164\040\141\040\143\157\155\155\141\156\144\040\146\157\162\040\164\150\145\040\151\156\164\145\162\141\143\164\151\166\145\040\155\145\156\165\056\012\012\103\157\155\155\141\156\144\163\072\012\040\040\141\156\163\167\145\162\040\040\040\040\040\040\123\150\157\167\040\164\150\145\040\141\156\163\167\145\162\012\040\040\150\145\154\160\040\040\040\040\040\040\040\040\123\150\157\167\040\164\150\151\163\040\150\145\154\160\012\012\117\160\164\151\157\156\163\072\012\040\040\055\150\054\040\055\055\150\145\154\160\040\040\123\150\157\167\040\164\150\151\163\040\150\145\154\160\012",216);
charge(r[43],0);
if(trap_code)goto cleanup;
r[42]=r[43];
goto returned;
goto returned; returned:;
cleanup:;
roots=frame.previous; --depth; active_slots-=44;
return r[42];

}
static V f28(V a29) {
 if(depth>=max_depth || active_slots>32768-186){fail(4);return unit();} ++depth; active_slots+=186;
 V r[186]={0}; Roots frame={roots,r,186}; roots=&frame;
r[29]=a29;
tick(); if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[45]=r[29];
charge(r[45],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[46]=string((const unsigned char*)"\061",1);
charge(r[46],0);
if(trap_code)goto cleanup;
r[44]=binary(5,r[45],r[46]);
charge(r[44],0);
if(trap_code)goto cleanup;
r[43]=r[44];if(!r[44].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[48]=r[29];
charge(r[48],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[49]=string((const unsigned char*)"\141\156\163\167\145\162",6);
charge(r[49],0);
if(trap_code)goto cleanup;
r[47]=binary(5,r[48],r[49]);
charge(r[47],0);
if(trap_code)goto cleanup;
r[43]=r[47];}
charge(r[43],0);
if(trap_code)goto cleanup;
if(r[43].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[50]=scalar(2,1LL);
charge(r[50],0);
if(trap_code)goto cleanup;
r[42]=r[50];
goto returned;
}else{
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[55]=r[29];
charge(r[55],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[56]=string((const unsigned char*)"\062",1);
charge(r[56],0);
if(trap_code)goto cleanup;
r[54]=binary(5,r[55],r[56]);
charge(r[54],0);
if(trap_code)goto cleanup;
r[53]=r[54];if(!r[54].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[58]=r[29];
charge(r[58],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[59]=string((const unsigned char*)"\150\145\154\160",4);
charge(r[59],0);
if(trap_code)goto cleanup;
r[57]=binary(5,r[58],r[59]);
charge(r[57],0);
if(trap_code)goto cleanup;
r[53]=r[57];}
charge(r[53],0);
if(trap_code)goto cleanup;
r[52]=r[53];if(!r[53].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[61]=r[29];
charge(r[61],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[62]=string((const unsigned char*)"\055\055\150\145\154\160",6);
charge(r[62],0);
if(trap_code)goto cleanup;
r[60]=binary(5,r[61],r[62]);
charge(r[60],0);
if(trap_code)goto cleanup;
r[52]=r[60];}
charge(r[52],0);
if(trap_code)goto cleanup;
r[51]=r[52];if(!r[52].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[64]=r[29];
charge(r[64],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[65]=string((const unsigned char*)"\055\150",2);
charge(r[65],0);
if(trap_code)goto cleanup;
r[63]=binary(5,r[64],r[65]);
charge(r[63],0);
if(trap_code)goto cleanup;
r[51]=r[63];}
charge(r[51],0);
if(trap_code)goto cleanup;
if(r[51].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[66]=scalar(2,2LL);
charge(r[66],0);
if(trap_code)goto cleanup;
r[42]=r[66];
goto returned;
}else{
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[71]=r[29];
charge(r[71],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[72]=string((const unsigned char*)"\060",1);
charge(r[72],0);
if(trap_code)goto cleanup;
r[70]=binary(5,r[71],r[72]);
charge(r[70],0);
if(trap_code)goto cleanup;
r[69]=r[70];if(!r[70].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[74]=r[29];
charge(r[74],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[75]=string((const unsigned char*)"\161",1);
charge(r[75],0);
if(trap_code)goto cleanup;
r[73]=binary(5,r[74],r[75]);
charge(r[73],0);
if(trap_code)goto cleanup;
r[69]=r[73];}
charge(r[69],0);
if(trap_code)goto cleanup;
r[68]=r[69];if(!r[69].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[77]=r[29];
charge(r[77],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[78]=string((const unsigned char*)"\161\165\151\164",4);
charge(r[78],0);
if(trap_code)goto cleanup;
r[76]=binary(5,r[77],r[78]);
charge(r[76],0);
if(trap_code)goto cleanup;
r[68]=r[76];}
charge(r[68],0);
if(trap_code)goto cleanup;
r[67]=r[68];if(!r[68].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[80]=r[29];
charge(r[80],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[81]=string((const unsigned char*)"\145\170\151\164",4);
charge(r[81],0);
if(trap_code)goto cleanup;
r[79]=binary(5,r[80],r[81]);
charge(r[79],0);
if(trap_code)goto cleanup;
r[67]=r[79];}
charge(r[67],0);
if(trap_code)goto cleanup;
if(r[67].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[82]=scalar(2,0LL);
charge(r[82],0);
if(trap_code)goto cleanup;
r[42]=r[82];
goto returned;
}else{
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[83]=scalar(2,-1LL);
charge(r[83],0);
if(trap_code)goto cleanup;
r[42]=r[83];
goto returned;
goto returned; returned:;
charge(r[42],0);
if(trap_code)goto cleanup;
r[34]=r[42];
++guard_depth;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[86]=r[34];
charge(r[86],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[87]=scalar(2,-1LL);
charge(r[87],0);
if(trap_code)goto cleanup;
r[85]=binary(10,r[86],r[87]);
charge(r[85],0);
if(trap_code)goto cleanup;
r[84]=r[85];if(r[85].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[89]=r[34];
charge(r[89],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[90]=scalar(2,2LL);
charge(r[90],0);
if(trap_code)goto cleanup;
r[88]=binary(8,r[89],r[90]);
charge(r[88],0);
if(trap_code)goto cleanup;
r[84]=r[88];}
charge(r[84],0);
if(trap_code)goto cleanup;
--guard_depth;
if(!r[84].n){fail(8);goto cleanup;}
++guard_depth;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[95]=r[29];
charge(r[95],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[96]=string((const unsigned char*)"\061",1);
charge(r[96],0);
if(trap_code)goto cleanup;
r[94]=binary(5,r[95],r[96]);
charge(r[94],0);
if(trap_code)goto cleanup;
r[93]=r[94];if(!r[94].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[98]=r[29];
charge(r[98],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[99]=string((const unsigned char*)"\141\156\163\167\145\162",6);
charge(r[99],0);
if(trap_code)goto cleanup;
r[97]=binary(5,r[98],r[99]);
charge(r[97],0);
if(trap_code)goto cleanup;
r[93]=r[97];}
charge(r[93],0);
if(trap_code)goto cleanup;
r[92]=unary(0,r[93]);
charge(r[92],0);
if(trap_code)goto cleanup;
r[91]=r[92];if(!r[92].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[101]=r[34];
charge(r[101],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[102]=scalar(2,1LL);
charge(r[102],0);
if(trap_code)goto cleanup;
r[100]=binary(5,r[101],r[102]);
charge(r[100],0);
if(trap_code)goto cleanup;
r[91]=r[100];}
charge(r[91],0);
if(trap_code)goto cleanup;
--guard_depth;
if(!r[91].n){fail(8);goto cleanup;}
++guard_depth;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[109]=r[29];
charge(r[109],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[110]=string((const unsigned char*)"\062",1);
charge(r[110],0);
if(trap_code)goto cleanup;
r[108]=binary(5,r[109],r[110]);
charge(r[108],0);
if(trap_code)goto cleanup;
r[107]=r[108];if(!r[108].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[112]=r[29];
charge(r[112],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[113]=string((const unsigned char*)"\150\145\154\160",4);
charge(r[113],0);
if(trap_code)goto cleanup;
r[111]=binary(5,r[112],r[113]);
charge(r[111],0);
if(trap_code)goto cleanup;
r[107]=r[111];}
charge(r[107],0);
if(trap_code)goto cleanup;
r[106]=r[107];if(!r[107].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[115]=r[29];
charge(r[115],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[116]=string((const unsigned char*)"\055\055\150\145\154\160",6);
charge(r[116],0);
if(trap_code)goto cleanup;
r[114]=binary(5,r[115],r[116]);
charge(r[114],0);
if(trap_code)goto cleanup;
r[106]=r[114];}
charge(r[106],0);
if(trap_code)goto cleanup;
r[105]=r[106];if(!r[106].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[118]=r[29];
charge(r[118],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[119]=string((const unsigned char*)"\055\150",2);
charge(r[119],0);
if(trap_code)goto cleanup;
r[117]=binary(5,r[118],r[119]);
charge(r[117],0);
if(trap_code)goto cleanup;
r[105]=r[117];}
charge(r[105],0);
if(trap_code)goto cleanup;
r[104]=unary(0,r[105]);
charge(r[104],0);
if(trap_code)goto cleanup;
r[103]=r[104];if(!r[104].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[121]=r[34];
charge(r[121],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[122]=scalar(2,2LL);
charge(r[122],0);
if(trap_code)goto cleanup;
r[120]=binary(5,r[121],r[122]);
charge(r[120],0);
if(trap_code)goto cleanup;
r[103]=r[120];}
charge(r[103],0);
if(trap_code)goto cleanup;
--guard_depth;
if(!r[103].n){fail(8);goto cleanup;}
++guard_depth;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[129]=r[29];
charge(r[129],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[130]=string((const unsigned char*)"\060",1);
charge(r[130],0);
if(trap_code)goto cleanup;
r[128]=binary(5,r[129],r[130]);
charge(r[128],0);
if(trap_code)goto cleanup;
r[127]=r[128];if(!r[128].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[132]=r[29];
charge(r[132],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[133]=string((const unsigned char*)"\161",1);
charge(r[133],0);
if(trap_code)goto cleanup;
r[131]=binary(5,r[132],r[133]);
charge(r[131],0);
if(trap_code)goto cleanup;
r[127]=r[131];}
charge(r[127],0);
if(trap_code)goto cleanup;
r[126]=r[127];if(!r[127].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[135]=r[29];
charge(r[135],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[136]=string((const unsigned char*)"\161\165\151\164",4);
charge(r[136],0);
if(trap_code)goto cleanup;
r[134]=binary(5,r[135],r[136]);
charge(r[134],0);
if(trap_code)goto cleanup;
r[126]=r[134];}
charge(r[126],0);
if(trap_code)goto cleanup;
r[125]=r[126];if(!r[126].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[138]=r[29];
charge(r[138],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[139]=string((const unsigned char*)"\145\170\151\164",4);
charge(r[139],0);
if(trap_code)goto cleanup;
r[137]=binary(5,r[138],r[139]);
charge(r[137],0);
if(trap_code)goto cleanup;
r[125]=r[137];}
charge(r[125],0);
if(trap_code)goto cleanup;
r[124]=unary(0,r[125]);
charge(r[124],0);
if(trap_code)goto cleanup;
r[123]=r[124];if(!r[124].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[141]=r[34];
charge(r[141],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[142]=scalar(2,0LL);
charge(r[142],0);
if(trap_code)goto cleanup;
r[140]=binary(5,r[141],r[142]);
charge(r[140],0);
if(trap_code)goto cleanup;
r[123]=r[140];}
charge(r[123],0);
if(trap_code)goto cleanup;
--guard_depth;
if(!r[123].n){fail(8);goto cleanup;}
++guard_depth;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[154]=r[29];
charge(r[154],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[155]=string((const unsigned char*)"\061",1);
charge(r[155],0);
if(trap_code)goto cleanup;
r[153]=binary(5,r[154],r[155]);
charge(r[153],0);
if(trap_code)goto cleanup;
r[152]=r[153];if(!r[153].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[157]=r[29];
charge(r[157],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[158]=string((const unsigned char*)"\141\156\163\167\145\162",6);
charge(r[158],0);
if(trap_code)goto cleanup;
r[156]=binary(5,r[157],r[158]);
charge(r[156],0);
if(trap_code)goto cleanup;
r[152]=r[156];}
charge(r[152],0);
if(trap_code)goto cleanup;
r[151]=r[152];if(!r[152].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[160]=r[29];
charge(r[160],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[161]=string((const unsigned char*)"\062",1);
charge(r[161],0);
if(trap_code)goto cleanup;
r[159]=binary(5,r[160],r[161]);
charge(r[159],0);
if(trap_code)goto cleanup;
r[151]=r[159];}
charge(r[151],0);
if(trap_code)goto cleanup;
r[150]=r[151];if(!r[151].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[163]=r[29];
charge(r[163],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[164]=string((const unsigned char*)"\150\145\154\160",4);
charge(r[164],0);
if(trap_code)goto cleanup;
r[162]=binary(5,r[163],r[164]);
charge(r[162],0);
if(trap_code)goto cleanup;
r[150]=r[162];}
charge(r[150],0);
if(trap_code)goto cleanup;
r[149]=r[150];if(!r[150].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[166]=r[29];
charge(r[166],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[167]=string((const unsigned char*)"\055\055\150\145\154\160",6);
charge(r[167],0);
if(trap_code)goto cleanup;
r[165]=binary(5,r[166],r[167]);
charge(r[165],0);
if(trap_code)goto cleanup;
r[149]=r[165];}
charge(r[149],0);
if(trap_code)goto cleanup;
r[148]=r[149];if(!r[149].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[169]=r[29];
charge(r[169],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[170]=string((const unsigned char*)"\055\150",2);
charge(r[170],0);
if(trap_code)goto cleanup;
r[168]=binary(5,r[169],r[170]);
charge(r[168],0);
if(trap_code)goto cleanup;
r[148]=r[168];}
charge(r[148],0);
if(trap_code)goto cleanup;
r[147]=r[148];if(!r[148].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[172]=r[29];
charge(r[172],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[173]=string((const unsigned char*)"\060",1);
charge(r[173],0);
if(trap_code)goto cleanup;
r[171]=binary(5,r[172],r[173]);
charge(r[171],0);
if(trap_code)goto cleanup;
r[147]=r[171];}
charge(r[147],0);
if(trap_code)goto cleanup;
r[146]=r[147];if(!r[147].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[175]=r[29];
charge(r[175],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[176]=string((const unsigned char*)"\161",1);
charge(r[176],0);
if(trap_code)goto cleanup;
r[174]=binary(5,r[175],r[176]);
charge(r[174],0);
if(trap_code)goto cleanup;
r[146]=r[174];}
charge(r[146],0);
if(trap_code)goto cleanup;
r[145]=r[146];if(!r[146].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[178]=r[29];
charge(r[178],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[179]=string((const unsigned char*)"\161\165\151\164",4);
charge(r[179],0);
if(trap_code)goto cleanup;
r[177]=binary(5,r[178],r[179]);
charge(r[177],0);
if(trap_code)goto cleanup;
r[145]=r[177];}
charge(r[145],0);
if(trap_code)goto cleanup;
r[144]=r[145];if(!r[145].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[181]=r[29];
charge(r[181],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[182]=string((const unsigned char*)"\145\170\151\164",4);
charge(r[182],0);
if(trap_code)goto cleanup;
r[180]=binary(5,r[181],r[182]);
charge(r[180],0);
if(trap_code)goto cleanup;
r[144]=r[180];}
charge(r[144],0);
if(trap_code)goto cleanup;
r[143]=r[144];if(!r[144].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[184]=r[34];
charge(r[184],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[185]=scalar(2,-1LL);
charge(r[185],0);
if(trap_code)goto cleanup;
r[183]=binary(5,r[184],r[185]);
charge(r[183],0);
if(trap_code)goto cleanup;
r[143]=r[183];}
charge(r[143],0);
if(trap_code)goto cleanup;
--guard_depth;
if(!r[143].n){fail(8);goto cleanup;}
cleanup:;
roots=frame.previous; --depth; active_slots-=186;
return r[42];

}
static V f30() {
 if(depth>=max_depth || active_slots>32768-47){fail(4);return unit();} ++depth; active_slots+=47;
 V r[47]={0}; Roots frame={roots,r,47}; roots=&frame;
tick(); if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[43]=string((const unsigned char*)"\124\150\145\040\141\156\163\167\145\162\040\151\163\040\064\062\056\012",18);
charge(r[43],0);
if(trap_code)goto cleanup;
r[42]=r[43];
goto returned;
goto returned; returned:;
charge(r[42],0);
if(trap_code)goto cleanup;
r[33]=r[42];
++guard_depth;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[45]=r[33];
charge(r[45],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[46]=string((const unsigned char*)"\124\150\145\040\141\156\163\167\145\162\040\151\163\040\064\062\056\012",18);
charge(r[46],0);
if(trap_code)goto cleanup;
r[44]=binary(5,r[45],r[46]);
charge(r[44],0);
if(trap_code)goto cleanup;
--guard_depth;
if(!r[44].n){fail(8);goto cleanup;}
cleanup:;
roots=frame.previous; --depth; active_slots-=47;
return r[42];

}
static V f31(V a32) {
 if(depth>=max_depth || active_slots>32768-123){fail(4);return unit();} ++depth; active_slots+=123;
 V r[123]={0}; Roots frame={roots,r,123}; roots=&frame;
r[32]=a32;
tick(); if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[44]=r[32];
charge(r[44],0);
if(trap_code)goto cleanup;
r[43]=scalar(2,pt_terminal_argc((U32)r[44].n));
charge(r[43],0);
if(trap_code)goto cleanup;
r[36]=r[43];
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[46]=r[32];
charge(r[46],0);
if(trap_code)goto cleanup;
r[45]=terminal_args((U32)r[46].n);
charge(r[45],0);
if(trap_code)goto cleanup;
r[37]=r[45];
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[48]=r[36];
charge(r[48],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[49]=scalar(2,1LL);
charge(r[49],0);
if(trap_code)goto cleanup;
r[47]=binary(5,r[48],r[49]);
charge(r[47],0);
if(trap_code)goto cleanup;
if(r[47].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[52]=r[37];
charge(r[52],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[53]=scalar(2,0LL);
charge(r[53],0);
if(trap_code)goto cleanup;
r[51]=index_value(r[52],r[53]);
charge(r[51],0);
if(trap_code)goto cleanup;
r[50]=f28(r[51]);
charge(r[50],0);
if(trap_code)goto cleanup;
r[38]=r[50];
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[55]=r[38];
charge(r[55],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[56]=scalar(2,1LL);
charge(r[56],0);
if(trap_code)goto cleanup;
r[54]=binary(5,r[55],r[56]);
charge(r[54],0);
if(trap_code)goto cleanup;
if(r[54].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[58]=r[32];
charge(r[58],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[59]=f30();
charge(r[59],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[58].n,0,(U32)(unsigned long)(object(r[59])+1),object(r[59])->len);r[57]=unit();
charge(r[57],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[60]=scalar(2,0LL);
charge(r[60],0);
if(trap_code)goto cleanup;
r[42]=r[60];
goto returned;
}else{
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[62]=r[38];
charge(r[62],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[63]=scalar(2,2LL);
charge(r[63],0);
if(trap_code)goto cleanup;
r[61]=binary(5,r[62],r[63]);
charge(r[61],0);
if(trap_code)goto cleanup;
if(r[61].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[65]=r[32];
charge(r[65],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[66]=f27();
charge(r[66],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[65].n,0,(U32)(unsigned long)(object(r[66])+1),object(r[66])->len);r[64]=unit();
charge(r[64],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[67]=scalar(2,0LL);
charge(r[67],0);
if(trap_code)goto cleanup;
r[42]=r[67];
goto returned;
}else{
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[69]=r[38];
charge(r[69],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[70]=scalar(2,0LL);
charge(r[70],0);
if(trap_code)goto cleanup;
r[68]=binary(5,r[69],r[70]);
charge(r[68],0);
if(trap_code)goto cleanup;
if(r[68].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[71]=scalar(2,0LL);
charge(r[71],0);
if(trap_code)goto cleanup;
r[42]=r[71];
goto returned;
}else{
}
}else{
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[73]=r[36];
charge(r[73],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[74]=scalar(2,0LL);
charge(r[74],0);
if(trap_code)goto cleanup;
r[72]=binary(9,r[73],r[74]);
charge(r[72],0);
if(trap_code)goto cleanup;
if(r[72].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[76]=r[32];
charge(r[76],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[77]=string((const unsigned char*)"\125\156\153\156\157\167\156\040\143\157\155\155\141\156\144\040\157\162\040\145\170\164\162\141\040\141\162\147\165\155\145\156\164\163\056\040\125\163\145\040\055\055\150\145\154\160\040\146\157\162\040\165\163\141\147\145\056\012",58);
charge(r[77],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[76].n,1,(U32)(unsigned long)(object(r[77])+1),object(r[77])->len);r[75]=unit();
charge(r[75],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[78]=scalar(2,2LL);
charge(r[78],0);
if(trap_code)goto cleanup;
r[42]=r[78];
goto returned;
}else{
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[80]=r[32];
charge(r[80],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[81]=f26();
charge(r[81],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[80].n,0,(U32)(unsigned long)(object(r[81])+1),object(r[81])->len);r[79]=unit();
charge(r[79],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
while(1){
tick();
if(trap_code)goto cleanup;
r[82]=scalar(1,1);
charge(r[82],0);
if(trap_code)goto cleanup;
if(!r[82].n)break;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[84]=r[32];
charge(r[84],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[85]=string((const unsigned char*)"\012\103\150\157\157\163\145\040\141\156\040\157\160\164\151\157\156\072\040",19);
charge(r[85],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[84].n,0,(U32)(unsigned long)(object(r[85])+1),object(r[85])->len);r[83]=unit();
charge(r[83],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[87]=r[32];
charge(r[87],0);
if(trap_code)goto cleanup;
r[86]=terminal_read_line((U32)r[87].n,7,8);
charge(r[86],0);
if(trap_code)goto cleanup;
r[39]=r[86];
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[89]=r[39];
charge(r[89],0);
if(trap_code)goto cleanup;
if(((r[89]).tag==6)&&(object(r[89])->aux==7)&&(object(r[89])->len==1)&&(1)){
charge(items(r[89])[0],0);
if(trap_code)goto cleanup;
r[40]=items(r[89])[0];
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[91]=r[40];
charge(r[91],0);
if(trap_code)goto cleanup;
r[90]=f28(r[91]);
charge(r[90],0);
if(trap_code)goto cleanup;
r[88]=r[90];goto match_0;}
if(((r[89]).tag==6)&&(object(r[89])->aux==8)&&(object(r[89])->len==0)){
tick();
if(trap_code)goto cleanup;
r[92]=scalar(2,0LL);
charge(r[92],0);
if(trap_code)goto cleanup;
r[88]=r[92];goto match_0;}
fail(6);goto cleanup;match_0:;
charge(r[88],0);
if(trap_code)goto cleanup;
r[41]=r[88];
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[94]=r[41];
charge(r[94],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[95]=scalar(2,0LL);
charge(r[95],0);
if(trap_code)goto cleanup;
r[93]=binary(5,r[94],r[95]);
charge(r[93],0);
if(trap_code)goto cleanup;
if(r[93].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[97]=r[32];
charge(r[97],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[98]=string((const unsigned char*)"\107\157\157\144\142\171\145\041\012",9);
charge(r[98],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[97].n,0,(U32)(unsigned long)(object(r[98])+1),object(r[98])->len);r[96]=unit();
charge(r[96],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[99]=scalar(2,0LL);
charge(r[99],0);
if(trap_code)goto cleanup;
r[42]=r[99];
goto returned;
}else{
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[101]=r[41];
charge(r[101],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[102]=scalar(2,1LL);
charge(r[102],0);
if(trap_code)goto cleanup;
r[100]=binary(5,r[101],r[102]);
charge(r[100],0);
if(trap_code)goto cleanup;
if(r[100].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[104]=r[32];
charge(r[104],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[105]=f30();
charge(r[105],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[104].n,0,(U32)(unsigned long)(object(r[105])+1),object(r[105])->len);r[103]=unit();
charge(r[103],0);
if(trap_code)goto cleanup;
}else{
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[107]=r[41];
charge(r[107],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[108]=scalar(2,2LL);
charge(r[108],0);
if(trap_code)goto cleanup;
r[106]=binary(5,r[107],r[108]);
charge(r[106],0);
if(trap_code)goto cleanup;
if(r[106].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[110]=r[32];
charge(r[110],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[111]=f27();
charge(r[111],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[110].n,0,(U32)(unsigned long)(object(r[111])+1),object(r[111])->len);r[109]=unit();
charge(r[109],0);
if(trap_code)goto cleanup;
}else{
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[113]=r[32];
charge(r[113],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[114]=string((const unsigned char*)"\103\150\157\157\163\145\040\061\054\040\062\054\040\157\162\040\060\056\012",19);
charge(r[114],0);
if(trap_code)goto cleanup;
pt_terminal_write((U32)r[113].n,1,(U32)(unsigned long)(object(r[114])+1),object(r[114])->len);r[112]=unit();
charge(r[112],0);
if(trap_code)goto cleanup;
}
}
}
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[115]=scalar(2,0LL);
charge(r[115],0);
if(trap_code)goto cleanup;
r[42]=r[115];
goto returned;
goto returned; returned:;
charge(r[42],0);
if(trap_code)goto cleanup;
r[35]=r[42];
++guard_depth;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[118]=r[35];
charge(r[118],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[119]=scalar(2,0LL);
charge(r[119],0);
if(trap_code)goto cleanup;
r[117]=binary(5,r[118],r[119]);
charge(r[117],0);
if(trap_code)goto cleanup;
r[116]=r[117];if(!r[117].n){
tick();
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[121]=r[35];
charge(r[121],0);
if(trap_code)goto cleanup;
tick();
if(trap_code)goto cleanup;
r[122]=scalar(2,2LL);
charge(r[122],0);
if(trap_code)goto cleanup;
r[120]=binary(5,r[121],r[122]);
charge(r[120],0);
if(trap_code)goto cleanup;
r[116]=r[120];}
charge(r[116],0);
if(trap_code)goto cleanup;
--guard_depth;
if(!r[116].n){fail(8);goto cleanup;}
cleanup:;
if(r[32].tag==7){pt_terminal_release((U32)r[32].n);r[32]=unit();}
roots=frame.previous; --depth; active_slots-=123;
return r[42];

}
unsigned int pt_run(void) { reset(100000ULL,64); V r[44]={0};Roots frame={0,r,44};roots=&frame; active_slots=44; r[43]=scalar(7,pt_terminal_root(0));
charge(r[43],0);
if(trap_code)goto cleanup;
r[42]=f31(r[43]);
if(!trap_code)encode(r[42],0);
cleanup: roots=0; active_slots=0; collect(); return trap_code;
 }
