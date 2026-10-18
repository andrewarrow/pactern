/* Native sanitizer harness for the exact freestanding guest runtime. */
#include <assert.h>
#define PT_HEAP_SIZE 2048
#include "pactern.c"
int main(void) {
    reset(100000,64);
    V slots[3]={0}; Roots frame={0,slots,3};roots=&frame;
    slots[0]=string((const unsigned char *)"rooted",6);
    slots[1]=allocate(4,1,0);items(slots[1])[0]=slots[0];
    slots[0]=unit();
    for(U32 i=0;i<200;++i){slots[2]=allocate(4,8,0);assert(!trap_code);}
    assert(collections>0);
    assert(object(items(slots[1])[0])->len==6);
    assert(((unsigned char *)(object(items(slots[1])[0])+1))[0]=='r');
    /* A scalar with the numeric bits of a heap address is not a root. */
    slots[0]=scalar(2,slots[1].ref);slots[1]=unit();slots[2]=unit();collect();
    assert(!((Obj *)(heap.data+8))->used);
    assert(((Obj *)(heap.data+8))->size==PT_HEAP_SIZE-8);
    binary(0,scalar(2,9223372036854775807LL),scalar(2,1));assert(trap_code==1);
    reset(10,64);binary(4,scalar(2,(-9223372036854775807LL-1)),scalar(2,-1));assert(trap_code==1);
    reset(10,64);binary(3,scalar(2,1),scalar(2,0));assert(trap_code==2);
    reset(10,64);slots[0]=allocate(4,0,0);index_value(slots[0],scalar(2,0));assert(trap_code==3);
    reset(0,64);tick();assert(trap_code==4);
    roots=0;collect();assert(!((Obj *)(heap.data+8))->used);
    return 0;
}
