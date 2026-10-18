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
