#!/usr/bin/env python3
import sys
from pathlib import Path
from integration import Host, record, v1, unwind_records

def run(library):
    # Reproduce the observed failure class with a real C host's existing heap.
    # Its kernel-selected address is independent of compiler generation placement.
    h = Host(library, host_name='host-pie')
    try:
        base = h.request(v1(flags=1))['load_base']
        heap = next((line for line in Path(f'/proc/{h.p.pid}/maps').read_text().splitlines()
                     if '[heap]' in line and line.split()[1]=='rw-p'), None)
        # A reservation can contain the kernel's randomized start_brk, making
        # glibc fall back entirely to mmap. Only writable VMAs are real heap.
        address = int(heap.split('-')[0], 16) if heap else h.entry

        reply = h.request(v1(record(address-base, b'x', 1, 0)))
        assert reply['code'] == 'MMAP_FAILED' and 'conflicts with' in reply['message'], reply
        if heap: assert '[heap]' in reply['message'], reply
        assert reply['attempts'] == 0 and reply['pages'] == 0 and h.value() == 1, reply
        assert reply['conflict_address'] == address and reply['conflict_start'] <= address < reply['conflict_end'], reply
        assert h.request(v1(flags=1))['conflict_address'] == 0
        print(f"mapping: PIE application mapping outside reservation refused without writes: {reply['message']} PASS")
    finally:
        h.close()
    h = Host(library)
    try:
        for i in range(512):
            # Real registered descriptors, without publishing reachable entries.
            target = 0x20000000+i*8192
            reply = h.request(v1(*unwind_records(h, target)[:3]))
            assert reply['ok'] and reply['unwind_copies'] == i+1, reply
            assert reply['near_slots_used'] == i+1 and reply['retained_bytes'] == (i+1)*12288, reply
            assert reply['pressure'] == (i+1 > 384), reply
        reply = h.request(v1(*unwind_records(h, 0x21000000)[:3]))
        assert reply['code'] == 'UNWIND_LIMIT' and reply['generation'] == 512, reply
        assert reply['pages'] == 1024 and reply['unwind_copies'] == 512 and reply['near_slots_used'] == 512, reply
        assert reply['retained_bytes'] == 512*12288 and reply['pressure'], reply
        ping = h.request(v1(flags=1))
        assert ping['pressure'] and ping['near_slots_used'] == 512 and h.value() == 1, ping
        print('pressure: 512 real copies, >75% threshold, hard-cap refusal cleans staged pages and preserves counters/output PASS')
    finally:
        h.close()

if __name__ == '__main__': run(sys.argv[1])
