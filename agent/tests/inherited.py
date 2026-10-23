#!/usr/bin/env python3
"""Make a splice-like PT_LOAD in a private C host; exercise real kernel inheritance."""
import os
from pathlib import Path
import struct
import subprocess
import sys
from integration import Host, BUILD, record, v1

def inherited_host(name, target=0x30000000):
    src = BUILD/name
    dst = BUILD/('inherited-'+name)
    blob = bytearray(src.read_bytes())
    phoff = struct.unpack_from('<Q', blob, 32)[0]
    phsize, count = struct.unpack_from('<HH', blob, 54)
    # PT_NOTE is not needed for this host. Replace one rather than growing PHDRs.
    at = next(phoff+i*phsize for i in range(count)
              if struct.unpack_from('<I', blob, phoff+i*phsize)[0] == 4)
    offset = (len(blob)+4095)&~4095
    blob.extend(bytes(offset+4096-len(blob)))
    blob[offset:offset+8] = b'inherit!'
    struct.pack_into('<IIQQQQQQ', blob, at, 1, 4, offset, target, target, 4096, 4096, 4096)
    dst.write_bytes(blob); dst.chmod(src.stat().st_mode)
    return dst, f'{target:x}:1000:1'

def run(library):
    for name in ('host', 'host-pie'):
        dst, spec = inherited_host(name)
        for env in ({'REFRESH_INHERITED': spec}, {'E106_INHERITED_SEGMENTS': spec},
                    {'REFRESH_INHERITED': spec+';'+spec}):
            h = Host(library, host_name=dst.name, env=env)
            try:
                ping = h.request(v1(flags=1))
                assert ping['pages'] == 1 and ping['retained_bytes'] == 4096, ping
                reply = h.request(v1(record(0x30000000, b'x', 1, 0)))
                assert reply['code'] == 'OVERLAP' and reply['attempts'] == 0, reply
                assert reply['conflict_address'] == ping['load_base']+0x30000000, reply
                assert h.value() == 1
                for i in range(20):
                    reply = h.request(v1(record(0x31000000+i*4096, b'x', 1, 0)))
                    assert reply['ok'], reply
                assert h.request(v1(flags=1))['pages'] == 21
            finally:
                h.close()
        # New empty value wins over legacy; occupied kernel mappings stay protected.
        h = Host(library, host_name=dst.name, env={'REFRESH_INHERITED': '', 'E106_INHERITED_SEGMENTS': spec})
        try:
            reply = h.request(v1(record(0x30000000, b'x', 1, 0)))
            assert reply['code'] == 'MMAP_FAILED' and 'conflicts with' in reply['message'], reply
            assert reply['pages'] == 0 and h.value() == 1
        finally:
            h.close()
        prefix = os.environ.get('REFRESH_TEST_ASAN_LIBRARY', '')
        for bad in (spec+';', ';'+spec, spec+';;'+spec, '30000000:1001:1', '30000000:1000:5',
                    '30000000:1000:1junk', 'fffffffffffff000:1000:1'):
            env = dict(os.environ, LD_PRELOAD=(prefix+':' if prefix else '')+str(Path(library).resolve()),
                       REFRESH_AGENT_SOCKET='/tmp/refresh-must-not-bind', REFRESH_HOST_EXE=str(dst), REFRESH_INHERITED=bad)
            result = subprocess.run([str(dst), 'o'], env=env, capture_output=True, text=True, timeout=10)
            assert result.returncode == 0 and result.stdout.strip() == '1 17', result
            assert 'invalid REFRESH_INHERITED' in result.stderr, result
    print('inherited: synthetic splice PT_LOAD, PIE/non-PIE, aliases/precedence, exact validation, OVERLAP and 20 later live patches PASS')

if __name__ == '__main__':
    run(sys.argv[1])
