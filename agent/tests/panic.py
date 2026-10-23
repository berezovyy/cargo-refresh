#!/usr/bin/env python3
import struct
import sys
import time
from integration import Host, record, v1

def unwind_throw(target, helper):
    # sub rsp,8; movabs helper,rax; call rax; ud2. CFA changes at offset four.
    code = b'\x48\x83\xec\x08\x48\xb8'+struct.pack('<Q', helper)+b'\xff\xd0\x0f\x0b'
    cie = struct.pack('<I', 0)+b'\x01zR\0\x01\x78\x10\x01\x1b\x0c\x07\x08\x90\x01'
    cie += bytes(-len(cie)%4)
    cie = struct.pack('<I', len(cie))+cie
    eh = target+4096
    fde = struct.pack('<IiI', len(cie)+4, target-(eh+len(cie)+8), len(code))+b'\0\x44\x0e\x10'
    fde += bytes(-len(fde)%4)
    frames = cie+struct.pack('<I', len(fde))+fde
    return [record(target, code, 5, 0), record(eh, frames, 1, 0),
            record(eh, struct.pack('<Q', len(frames)), 1, 3)]

def run(library):
    h = Host(library, host_name='host-panic')
    refused = 0
    try:
        initial = int(h.command('s').split()[1])
        for i in range(200):
            target = 0x20000000+i*8192
            records = unwind_throw(target, h.ro)
            records += [record(h.entry, b'\xe9'+struct.pack('<i', target-h.entry-5), 5, 1)]
            for _ in range(10):
                reply = h.request(v1(*records))
                if reply['ok']: break
                assert reply['code'] in ('STACK_COLLISION', 'PARK_TIMEOUT'), reply
                refused += 1; time.sleep(.01)
            else: raise AssertionError(('persistent panic refusal', i, reply))
            assert reply['unwind_copies'] == i+1 and reply['unwind_registered_total'] == i+1, reply
            assert reply['near_slots_used'] == i+1 and not reply['pressure'], reply
            assert reply['retained_bytes'] == (i+1)*3*4096, reply
            # Force progress through every freshly published FDE, not just old code.
            before, errors = map(int, h.command('s').split()[1:]); assert errors == 0
            for _ in range(100):
                after, errors = map(int, h.command('s').split()[1:]); assert errors == 0
                if after > before: break
                time.sleep(.001)
            else: raise AssertionError(('panic worker stalled', i))
        assert after > initial
        print(f'panic: two C++ throw/catch loops, 200/200 patches through new FDEs, caught={after}, safe retries={refused}, zero errors/deadlocks PASS')
    finally:
        h.close()

if __name__ == '__main__': run(sys.argv[1])
