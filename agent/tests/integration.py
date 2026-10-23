#!/usr/bin/env python3
"""Local protocol regression driver; deliberately does not start rustd."""
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import time

MAGIC = 0x76313232
BASE = Path(__file__).resolve().parents[1]

def record(addr, data, prot, kind, fixups=()):
    return struct.pack('<QQIII', addr, len(data), prot, kind, len(fixups)) + data + b''.join(struct.pack('<I', x) for x in fixups)

def v0(*records):
    return struct.pack('<II', MAGIC, len(records)) + b''.join(records)

class Host:
    def __init__(self, library, threads=0, env=None):
        self.tmp = tempfile.TemporaryDirectory(prefix='refresh-test-')
        self.path = self.tmp.name + '/agent.sock'
        host = str(BASE/'build/host')
        environ = dict(os.environ, LD_PRELOAD=str(Path(library).resolve()), E077_AGENT=self.path, E106_HOST_EXE=host)
        environ.update(env or {})
        self.p = subprocess.Popen([host, str(threads)], env=environ, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        line = self.p.stdout.readline()
        assert line.startswith('ready '), (line, self.p.stderr.read())
        self.entry = int(line.split()[1], 16)
        for _ in range(500):
            if Path(self.path).exists(): break
            assert self.p.poll() is None, self.p.stderr.read()
            time.sleep(.01)
        else: raise AssertionError('agent did not start')

    def request(self, message):
        with socket.socket(socket.AF_UNIX) as c:
            c.settimeout(10)
            c.connect(self.path)
            c.sendall(message)
            c.shutdown(socket.SHUT_WR)
            data = b''
            while b'\n' not in data:
                part = c.recv(4096)
                if not part: break
                data += part
        return json.loads(data)

    def value(self):
        self.p.stdin.write('v'); self.p.stdin.flush()
        line = self.p.stdout.readline().split()
        assert line[0] == 'value' and line[2:] == ['errors', '0'], line
        return int(line[1])

    def close(self):
        try:
            if self.p.poll() is None:
                self.p.stdin.write('q'); self.p.stdin.flush()
                self.p.wait(timeout=10)
            stderr = self.p.stderr.read()
            assert self.p.returncode == 0, (self.p.returncode, stderr)
            assert 'ERROR: AddressSanitizer' not in stderr and 'runtime error:' not in stderr, stderr
        finally:
            if self.p.poll() is None: self.p.kill(); self.p.wait()
            self.tmp.cleanup()

def protocol_suite(library):
    h = Host(library)
    outcomes = []
    try:
        assert h.value() == 1
        bad = [b'badmagic', b'\x32', struct.pack('<II', MAGIC, 8193),
               struct.pack('<II', MAGIC, 1),
               v0(record(h.entry, b'abcde', 5, 8)),
               v0(record(h.entry, b'abcde', 5, 1, [0])),
               v0(record(0x20000000, b'12345678', 1, 0, [1])),
               v0(record(0x20000000, b'1234567890123456', 1, 0, [8, 0])),
               v0(record(0x20000000, b'1234567890123456', 1, 0, [0, 4])),
               v0(record(0x20000000, b'12345678', 5, 3)),
               v0(record(h.entry, b'x', 1, 2))]
        for msg in bad:
            reply = h.request(msg)
            assert not reply['ok'], reply
            outcomes.append(reply['ok'])
            assert h.value() == 1
        reply = h.request(v0())
        assert reply['ok'], reply
        outcomes.append(reply['ok'])
        target = 0x20000000
        jump = b'\xe9' + struct.pack('<i', target-h.entry-5)
        msg = v0(record(target, b'\xb8\x02\0\0\0\xc3', 5, 0), record(h.entry, jump, 5, 1))
        reply = h.request(msg)
        assert reply['ok'], reply
        assert h.value() == 2
        outcomes.append(reply['ok'])
        reply = h.request(msg)
        assert not reply['ok'] and h.value() == 2, reply
        outcomes.append(reply['ok'])
        assert h.request(struct.pack('<II', 0xE099CA11, 0))['ok']
        print(f'{Path(library).name}: protocol cases={len(outcomes)} + HOOK, entry jump 1->2 PASS')
        return outcomes
    finally:
        h.close()

if __name__ == '__main__':
    outcomes = [protocol_suite(lib) for lib in sys.argv[1:]]
    assert all(x == outcomes[0] for x in outcomes), outcomes
    if len(outcomes)>1: print('old/new protocol outcomes identical PASS')
