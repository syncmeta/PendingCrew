#!/usr/bin/env python3
"""Exec with inherited Linux no-IP seccomp. Fails closed before target exec.

No namespace/root emulation, host policy modification, network attempt or signal.
All target descendants inherit no_new_privs and this filter. The private Core
still uses its ordinary Unix socket and real production permission boundaries.
"""
import ctypes,errno,os,sys


def main():
    assert sys.platform=='linux' and os.getuid()==os.geteuid()!=0
    assert len(sys.argv)>1 and os.path.isabs(sys.argv[1])
    # libseccomp resolves the actual native syscall/architecture; no guessed IDs.
    lib=ctypes.CDLL('libseccomp.so.2',use_errno=True)
    libc=ctypes.CDLL(None,use_errno=True)
    class Compare(ctypes.Structure):
        _fields_=[('arg',ctypes.c_uint),('op',ctypes.c_int),
                  ('datum_a',ctypes.c_uint64),('datum_b',ctypes.c_uint64)]
    lib.seccomp_init.argtypes=[ctypes.c_uint32];lib.seccomp_init.restype=ctypes.c_void_p
    lib.seccomp_rule_add_array.argtypes=[ctypes.c_void_p,ctypes.c_uint32,ctypes.c_int,ctypes.c_uint,ctypes.POINTER(Compare)]
    lib.seccomp_syscall_resolve_name.argtypes=[ctypes.c_char_p];lib.seccomp_syscall_resolve_name.restype=ctypes.c_int
    lib.seccomp_load.argtypes=[ctypes.c_void_p];lib.seccomp_release.argtypes=[ctypes.c_void_p]
    assert libc.prctl(38,1,0,0,0)==0,'no_new_privs failed'
    ctx=lib.seccomp_init(0x7fff0000) # SCMP_ACT_ALLOW
    assert ctx,'seccomp allocation failed'
    try:
        for name in [b'socket',b'socketpair']:
            syscall=lib.seccomp_syscall_resolve_name(name);assert syscall>=0
            comparison=Compare(0,1,1,0) # SCMP_CMP_NE(1): domain != AF_UNIX(1)
            assert lib.seccomp_rule_add_array(ctx,0x00050000|errno.EPERM,syscall,1,ctypes.byref(comparison))==0
        # io_uring socket operations must not bypass the socket-domain filter.
        for name in [b'io_uring_setup',b'io_uring_enter',b'io_uring_register']:
            syscall=lib.seccomp_syscall_resolve_name(name);assert syscall>=0
            assert lib.seccomp_rule_add_array(ctx,0x00050000|errno.EPERM,syscall,0,None)==0
        # Native x86_64 only is qualified by the caller; no legacy socketcall ABI.
        assert os.uname().machine=='x86_64'
        assert lib.seccomp_load(ctx)==0,'seccomp installation failed'
    finally:lib.seccomp_release(ctx)
    os.execve(sys.argv[1],sys.argv[1:],dict(os.environ))


if __name__=='__main__':main()
