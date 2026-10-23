#!/usr/bin/env python3
"""Local protocol regression driver; deliberately does not start rustd."""
import argparse
import json
import select
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
BUILD = Path(os.environ.get('REFRESH_TEST_BUILD', BASE/'build'))

def record(addr, data, prot, kind, fixups=()):
    return struct.pack('<QQIII', addr, len(data), prot, kind, len(fixups)) + data + b''.join(struct.pack('<I', x) for x in fixups)

def v0(*records):
    return struct.pack('<II', MAGIC, len(records)) + b''.join(records)

def memory_guard():
    for line in Path('/proc/meminfo').read_text().splitlines():
        if line.startswith('MemAvailable:'):
            available_kib = int(line.split()[1])
            if available_kib < 6 * 1024 * 1024:
                raise RuntimeError(f'MemAvailable below 6 GiB guard: {available_kib} KiB; stopping local tests')
            return
    raise RuntimeError('MemAvailable is unavailable; refusing to start local test hosts')

class Host:
    def __init__(self, library, threads=0, env=None, legacy=False, host_name='host'):
        memory_guard()
        self.tmp = tempfile.TemporaryDirectory(prefix='refresh-test-')
        self.path = self.tmp.name + '/agent.sock'
        host = str(BUILD/host_name)
        prefix = os.environ.get('REFRESH_TEST_ASAN_LIBRARY', '')
        preload = (str(Path(library).resolve())+':'+prefix if prefix and os.environ.get('REFRESH_TEST_AGENT_FIRST') else
                   (prefix + ':' if prefix else '') + str(Path(library).resolve()))
        environ = dict(os.environ, LD_PRELOAD=preload)
        for key in list(environ):
            if key.startswith(('REFRESH_', 'E077_', 'E106_', 'E084_', 'E054_', 'E099_', 'E121_', 'E123_')):
                del environ[key]
        environ.update({'E077_AGENT': self.path, 'E106_HOST_EXE': host} if legacy else
                       {'REFRESH_AGENT_SOCKET': self.path, 'REFRESH_HOST_EXE': host})
        environ.update(env or {})
        self.p = subprocess.Popen([host, str(threads)], env=environ, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        line = self.line()
        assert line.startswith('ready '), (line, self.p.stderr.read())
        self.entry, self.ro, self.other_entry, self.cross_entry, self.cross_ro = [int(x, 16) for x in line.split()[1:]]
        for _ in range(500):
            if Path(self.path).exists(): break
            assert self.p.poll() is None, self.p.stderr.read()
            time.sleep(.01)
        else: raise AssertionError('agent did not start')

    def line(self):
        if not select.select([self.p.stdout], [], [], 15)[0]:
            self.p.kill(); self.p.wait()
            raise AssertionError(('host output timeout', self.p.stderr.read()))
        line = self.p.stdout.readline()
        if not line:
            self.p.wait(timeout=5)
            raise AssertionError(('host exited', self.p.returncode, self.p.stderr.read()))
        return line

    def command(self, command):
        self.p.stdin.write(command); self.p.stdin.flush()
        return self.line().strip()

    def request(self, message):
        memory_guard()
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
        line = self.command('v').split()
        assert line[0] == 'value' and line[2:] == ['errors', '0'], line
        return int(line[1])

    def close(self):
        try:
            if self.p.poll() is None:
                self.p.stdin.write('q'); self.p.stdin.flush()
                self.p.wait(timeout=10)
            stderr = self.p.stderr.read()
            assert self.p.returncode == 0, (self.p.returncode, stderr)
            assert 'AddressSanitizer' not in stderr and 'runtime error:' not in stderr, stderr
            self.stderr = stderr
        finally:
            if self.p.poll() is None: self.p.kill(); self.p.wait()
            self.tmp.cleanup()

def protocol_suite(library):
    h = Host(library, legacy=True)
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
        assert all(key in reply for key in ('scan_first_root123', 'scan_us123', 'scan_bytes123', 'scan_hits123', 'scan_retained123', 'scan_candidates123', 'scan_errors123', 'reclaimed123')), reply
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

def v1(*records, flags=0, txid=42, total=None, version=1):
    body = b''.join(records)
    return struct.pack('<4sHHIIQ', b'RFR1', version, flags, len(records), txid,
                       24 + len(body) if total is None else total) + body

def patch_records(h, target=0x20000000, value=2, data=True):
    jump = b'\xe9' + struct.pack('<i', target-h.entry-5)
    records = [record(target, b'\xb8' + struct.pack('<I', value) + b'\xc3', 5, 0),
               record(h.entry, jump, 5, 1)]
    if data: records.append(record(h.ro, struct.pack('<I', 23), 1, 2))
    return records

def ro_value(h):
    return int(h.command('r').split()[1])

def oracle_values(after=False):
    exe = BUILD / ('oracle-after' if after else 'host')
    return tuple(map(int, subprocess.check_output([str(exe), 'o'], text=True).split()))

def unwind_records(h, target=0x20000000):
    # Minimal x86-64 CIE: CFA=rsp+8, RIP at CFA-8. New code is a leaf.
    cie = struct.pack('<I', 0) + b'\x01zR\0\x01\x78\x10\x01\x1b\x0c\x07\x08\x90\x01'
    cie += b'\0' * (-len(cie) % 4)
    cie = struct.pack('<I', len(cie)) + cie
    eh_addr = target + 4096
    pc_offset = len(cie) + 8
    fde = struct.pack('<IiI', len(cie) + 4, target-(eh_addr+pc_offset), 6) + b'\0'
    fde += b'\0' * (-len(fde) % 4)
    frames = cie + struct.pack('<I', len(fde)) + fde
    return [record(target, b'\xb8\x02\0\0\0\xc3', 5, 0),
            record(eh_addr, frames, 1, 0),
            record(eh_addr, struct.pack('<Q', len(frames)), 1, 3),
            record(h.entry, b'\xe9' + struct.pack('<i', target-h.entry-5), 5, 1)]

def mapping_permissions(h, addresses):
    mappings = []
    for line in Path(f'/proc/{h.p.pid}/maps').read_text().splitlines():
        fields = line.split()
        start, end = (int(x, 16) for x in fields[0].split('-'))
        mappings.append((start, end, fields[1]))
    return [next((perms for start, end, perms in mappings if start <= a < end), None) for a in addresses]

def activation_suite(library):
    with tempfile.TemporaryDirectory(prefix='refresh-activation-') as tmp:
        prefix = os.environ.get('REFRESH_TEST_ASAN_LIBRARY', '')
        preload = (prefix + ':' if prefix else '') + str(Path(library).resolve())
        env = {k: v for k, v in os.environ.items() if not k.startswith(('REFRESH_', 'E077_', 'E106_'))}
        env.update(LD_PRELOAD=preload, REFRESH_AGENT_SOCKET=tmp+'/agent.sock')
        result = subprocess.run([str(BUILD/'host'), 'o'], env=env, text=True, capture_output=True, timeout=10)
        assert result.returncode == 0 and result.stdout.strip() == '1 17', result
        assert 'REFRESH_HOST_EXE is required' in result.stderr and not Path(tmp+'/agent.sock').exists(), result
        env['REFRESH_HOST_EXE'] = '/not/the/host'
        result = subprocess.run([str(BUILD/'host'), 'o'], env=env, text=True, capture_output=True, timeout=10)
        assert result.returncode == 0 and result.stdout.strip() == '1 17' and not Path(tmp+'/agent.sock').exists(), result
    print('activation: missing v1 host rejected, inherited child executable filtered PASS')

def locale_suite(library):
    h = Host(library, env={'REFRESH_TEST_LOCALE': 'en_DK.utf8'})
    try:
        decimal = h.command('l')
        if decimal == 'locale unavailable':
            print('locale: comma-decimal locale unavailable (SKIP)')
            return
        assert decimal == 'locale ,', decimal
        reply = h.request(v1(flags=1))
        assert reply['ok'] and isinstance(reply['agent_us'], (float, int)), reply
        reply = h.request(v1(*patch_records(h)))
        assert reply['ok'] and h.value() == 2 and h.command('n') == decimal, reply
        print('locale: application comma decimals preserved, reply JSON uses valid C decimals PASS')
    finally:
        h.close()

def pie_suite(library):
    h = Host(library, host_name='host-pie')
    try:
        base = h.request(v1(flags=1))['load_base']
        assert base > 0
        entry, ro = h.entry-base, h.ro-base
        target = 0x20000000
        records = [record(target, b'\xb8\x02\0\0\0\xc3', 5, 0),
                   record(target+256, struct.pack('<Q', entry), 5, 0, [0]),
                   record(entry, b'\xe9'+struct.pack('<i', target-entry-5), 5, 1),
                   record(ro, struct.pack('<I', 23), 1, 2)]
        reply = h.request(v1(*records))
        assert reply['ok'] and reply['pages'] == 1, reply
        assert (h.value(), ro_value(h)) == oracle_values(True)
        with open(f'/proc/{h.p.pid}/mem', 'rb', buffering=0) as mem:
            fixed = struct.unpack('<Q', os.pread(mem.fileno(), 8, base+target+256))[0]
        assert fixed == h.entry, (fixed, h.entry)
        assert mapping_permissions(h, [base+target, h.entry, h.ro]) == ['r-xp', 'r-xp', 'r--p']
        print('PIE: nonzero ASLR base, entry/data relocation, live absolute fixup, final page protections, stock C oracle PASS')
    finally:
        h.close()

def v1_suite(library):
    before, after = oracle_values(), oracle_values(True)
    assert before == (1, 17) and after == (2, 23), (before, after)
    h = Host(library, env={'E077_AGENT': '/invalid/legacy/path', 'E084_MODE': 'ip'})
    try:
        assert (h.value(), ro_value(h)) == before
        ping = h.request(v1(flags=1, txid=987))
        assert ping['ok'] and ping['code'] == 'OK' and ping['generation'] == 0 and ping['txid'] == 987, ping
        assert ping['mode'] == 4, ping
        hook = h.request(v1(flags=2))
        assert hook['ok'] and hook['callback_invoked'] and hook['generation'] == 0, hook
        bad = [v1(version=2), v1(flags=3), v1(flags=1, total=25),
               v1(total=(256*1024*1024)+1), v1(total=23), v1(total=25),
               v1(record(0x20000000, b'x', 1, 0), total=24),
               v1(record(0xffffffffffffffff, b'x', 1, 0)),
               v1(record(h.entry, b'x'*5, 5, 1), total=24+28+4)]
        for message in bad:
            reply = h.request(message)
            assert not reply['ok'] and reply['code'] == 'BAD_PROTOCOL' and reply['generation'] == 0, reply
        # Mapping an existing application page must not clobber it or retain a staged page.
        reply = h.request(v1(record(0x20000000, b'x', 1, 0), record(h.entry, b'x', 1, 0)))
        assert reply['code'] == 'MMAP_FAILED' and reply['pages'] == 0, reply
        reply = h.request(v1(record(0x20000000, b'x', 1, 0), record(0x20000010, b'x', 5, 0)))
        assert reply['code'] == 'BAD_PROTOCOL' and reply['pages'] == 0, reply
        reply = h.request(v1(record(h.entry, b'x', 1, 2)))
        assert reply['code'] == 'NOT_ORIGINAL_RO', reply
        reply = h.request(v1(*patch_records(h)))
        assert reply['ok'] and reply['generation'] == 1 and reply['pages'] == 1, reply
        assert (h.value(), ro_value(h)) == after
        assert h.request(v1(flags=1))['generation'] == 1
        assert h.request(v1(*patch_records(h)))['code'] == 'OVERLAP'
        assert (h.value(), ro_value(h)) == after
        print('v1: PING/HOOK/txid, bounds/refusals/cleanup, env precedence, stock C oracle 1/17 -> 2/23 PASS')
    finally:
        h.close()
    assert 'deprecated' not in h.stderr, h.stderr
    # All v0 aliases produce one warning, independent of how many are used.
    h = Host(library, legacy=True, env={'E121_DEBUG': '0', 'E099_AUTO_HOOK': '1', 'E106_INHERITED_SEGMENTS': ''})
    try:
        reply = h.request(v1(*patch_records(h)))
        assert reply['ok'] and reply['callback_invoked'], reply
    finally:
        h.close()
    assert h.stderr.count('deprecated') == 1, h.stderr
    h = Host(library, env={'REFRESH_PARK_SIGNAL': '12'})
    try:
        assert h.request(v1(*patch_records(h)))['ok']
    finally:
        h.close()
    # Release must ignore fault injection even if a controller inherited the test env.
    h = Host(library, env={'REFRESH_TESTING_FAIL_AT': '1'})
    try:
        assert h.request(v1(*patch_records(h)))['ok'] and h.value() == 2
    finally:
        h.close()

def rollback_suite(library):
    count = 0
    before, after = oracle_values(), oracle_values(True)
    for phase in ('after', 'write', 'restore'):
        for index in range(3):
            h = Host(library, env={'REFRESH_TESTING_FAIL_AT': str(index), 'REFRESH_TESTING_FAIL_PHASE': phase})
            try:
                reply = h.request(v1(*patch_records(h)))
                assert reply['code'] == 'ROLLED_BACK' and not reply['ok'], reply
                assert reply['generation'] == 0 and reply['pages'] == 0 and reply['unwind_active'] == 0, reply
                assert (h.value(), ro_value(h)) == before
                assert mapping_permissions(h, [0x20000000, h.entry, h.ro]) == ['---p', 'r-xp', 'r--p']
                assert h.command('c') == 'cleared'
                # Reuse the very same staged address: rollback must have restored its reservation.
                reply = h.request(v1(*patch_records(h)))
                assert reply['ok'] and reply['generation'] == 1 and reply['pages'] == 1, reply
                assert (h.value(), ro_value(h)) == after
                count += 1
            finally:
                h.close()
    # Reverse-order undo on two writes to the same entry and on another entry.
    h = Host(library, env={'REFRESH_TESTING_FAIL_AT': '3'})
    try:
        r = patch_records(h, data=False)
        r += [record(h.other_entry, b'\xe9' + struct.pack('<i', 0x20000000-h.other_entry-5), 5, 1), r[1]]
        reply = h.request(v1(*r))
        assert reply['code'] == 'ROLLED_BACK' and h.value() == 1 and h.command('g') == 'g 1', reply
        assert h.command('c') == 'cleared'
        assert h.request(v1(*r))['ok'] and h.value() == 2 and h.command('g') == 'g 2'
        count += 1
    finally:
        h.close()
    # Staged unwind registration is removed on refusal at each descriptor/page/entry record.
    for index in range(4):
        h = Host(library, env={'REFRESH_TESTING_FAIL_AT': str(index)})
        try:
            reply = h.request(v1(*unwind_records(h)))
            assert reply['code'] == 'ROLLED_BACK' and reply['pages'] == 0 and reply['unwind_active'] == 0, reply
            assert reply['unwind_registered_total'] == reply['unwind_deregistered_total'], reply
            assert h.value() == 1
            assert h.command('c') == 'cleared'
            reply = h.request(v1(*unwind_records(h)))
            assert reply['ok'] and reply['unwind_active'] == 1 and h.value() == 2, reply
            count += 1
        finally:
            h.close()
    # Original writes that straddle two pages must restore both pages' permissions.
    for phase in ('after', 'write', 'restore'):
        for index in (1, 2):
            h = Host(library, env={'REFRESH_TESTING_FAIL_AT': str(index), 'REFRESH_TESTING_FAIL_PHASE': phase})
            try:
                r = [record(0x20000000, b'\xb8\x02\0\0\0\xc3', 5, 0),
                     record(h.cross_entry, b'\xe9' + struct.pack('<i', 0x20000000-h.cross_entry-5), 5, 1),
                     record(h.cross_ro, struct.pack('<I', 23), 1, 2)]
                reply = h.request(v1(*r))
                assert reply['code'] == 'ROLLED_BACK' and reply['pages'] == 0, reply
                assert h.command('x') == 'cross 1' and h.command('z') == 'cross_ro 17'
                assert mapping_permissions(h, [h.cross_entry, h.cross_entry+4, h.cross_ro, h.cross_ro+3]) == ['r-xp', 'r-xp', 'r--p', 'r--p']
                assert h.command('c') == 'cleared'
                reply = h.request(v1(*r))
                assert reply['ok'], reply
                assert h.command('x') == 'cross 2' and h.command('z') == 'cross_ro 23'
                count += 1
            finally:
                h.close()
    print(f'rollback: {count} injected failures (all records, write/restore/after, repeated entries, unwind) + later good patches PASS')

def server_suite(library):
    h = Host(library)
    try:
        # Oversize must be rejected without waiting for a body or EOF.
        with socket.socket(socket.AF_UNIX) as c:
            c.settimeout(2); c.connect(h.path)
            c.sendall(v1(total=(256*1024*1024)+1))
            reply = json.loads(c.recv(4096))
            assert reply['code'] == 'BAD_PROTOCOL', reply
        # A partially sent header must time out and let the next connection through.
        with socket.socket(socket.AF_UNIX) as c:
            c.settimeout(8); c.connect(h.path); c.sendall(b'RFR1')
            reply = json.loads(c.recv(4096))
            assert reply['code'] == 'BAD_PROTOCOL', reply
        assert h.request(v1(flags=1))['ok']
        # A disconnected client cannot deliver SIGPIPE to the application.
        for _ in range(5):
            with socket.socket(socket.AF_UNIX) as c:
                c.connect(h.path); c.sendall(v1(flags=1))
        assert h.request(v1(flags=1))['ok'] and h.value() == 1
        print('server: oversize rejected before body/EOF, stalled header times out, disconnected clients, later PING PASS')
    finally:
        h.close()

def late_handler_suite(library):
    h = Host(library, env={'REFRESH_TESTING_HANDLER_DELAY_US': '150000'})
    try:
        message = v1(*patch_records(h))
        reply = h.request(message)
        assert reply['code'] == 'PARK_TIMEOUT' and reply['pages'] == 0, reply
        assert reply['generation'] == 0 and h.value() == 1, reply
        reply = h.request(message)
        assert reply['ok'] and h.value() == 2, reply
        print('parking: handler delayed past timeout, release drains in-flight handler, same-address retry succeeds PASS')
    finally:
        h.close()

def threaded_suite(library):
    h = Host(library, threads=4)
    refusals = 0
    try:
        for i in range(200):
            value = 2 if i % 2 == 0 else 1
            message = v1(*patch_records(h, target=0x20000000 + i*4096, value=value, data=False), txid=i)
            for _ in range(10):
                reply = h.request(message)
                if reply['ok']: break
                assert reply['code'] in ('STACK_COLLISION', 'PARK_TIMEOUT'), reply
                refusals += 1; time.sleep(.05)
            else: raise AssertionError(('persistent refusal', i, reply))
            assert reply['generation'] == i+1 and reply['threads'] == 5 and h.value() == value, (i, reply)
        print(f'threads: 4 busy callers, 200/200 patches, 0 invalid values/crashes, {refusals} safe refusals PASS')
    finally:
        h.close()

if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--equivalence', action='store_true')
    parser.add_argument('--testing')
    parser.add_argument('libraries', nargs='+')
    args = parser.parse_args()
    outcomes = [protocol_suite(lib) for lib in args.libraries]
    assert all(x == outcomes[0] for x in outcomes), outcomes
    if len(outcomes) > 1: print('old/new protocol outcomes identical PASS')
    if not args.equivalence:
        library = args.libraries[0]
        activation_suite(library)
        v1_suite(library)
        locale_suite(library)
        pie_suite(library)
        if args.testing:
            rollback_suite(args.testing)
            late_handler_suite(args.testing)
        server_suite(library)
        threaded_suite(library)
