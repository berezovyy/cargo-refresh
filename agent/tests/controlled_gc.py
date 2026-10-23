#!/usr/bin/env python3
"""Optional research-only leaf-GC metadata smoke; never run under ASan root maps."""
import sys
import integration as t

library = sys.argv[1]
h = t.Host(library, env={'E123_SCAN': '1', 'E123_GC_TEST': '1'})
try:
    expected = t.oracle_values(True)[0]
    # Avoid common small integers in heap bytes that conservatively look like
    # pointers near 0x20000000. The scanner deliberately includes unaligned words.
    first, second = 0x61234000, 0x61236000
    reply = h.request(t.v1(*t.unwind_records(h, target=first)))
    assert reply['ok'] and h.value() == expected and reply['unwind_active'] == 1, reply
    reply = h.request(t.v1(*t.patch_records(h, target=second, data=False)))
    assert reply['ok'] and h.value() == expected and reply['reclaimed123'] >= 1, reply
    assert reply['unwind_active'] == 0 and reply['unwind_retained_bytes'] == 0, reply
    reply = h.request(t.v1(*t.patch_records(h, target=first, data=False)))
    assert reply['ok'] and h.value() == expected, reply
    print('test-only controlled leaf GC: old frame/cache removed, freed-page index removed, address reused, stock return 2 PASS')
finally:
    h.close()
