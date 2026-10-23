#!/usr/bin/env python3
import sys
import time
from integration import Host, record, v1, patch_records

def run(library):
    h = Host(library, host_name='host-storm')
    refused = 0
    exiting_refusals = 0
    try:
        for i in range(1000):
            value = 2 if i % 2 == 0 else 1
            message = v1(*patch_records(h, target=0x20000000+i*4096, value=value, data=False))
            for attempt in range(10):
                reply = h.request(message)
                if reply['ok']: break
                assert reply['code'] in ('PARK_TIMEOUT', 'STACK_COLLISION'), reply
                # A child can be inside libc's exit trampoline after unregistering.
                # It must be refused safely too; it has not entered new user code.
                if reply['unregistered_threads']:
                    assert reply['code'] == 'PARK_TIMEOUT', reply
                    exiting_refusals += 1
                refused += 1
                time.sleep(.01)
            else: raise AssertionError(('persistent storm refusal', i, reply))
            assert reply['generation'] == i+1 and reply['unregistered_threads'] == 0, reply
            if i % 10 == 0: assert h.value() == value
        stats = list(map(int, h.command('s').split()[1:]))
        assert min(stats) > 0, stats
        assert h.value() == 1
        print(f'storm: 1000/1000 patches, {refused} safe retries ({exiting_refusals} exiting-thread refusals), created/exited/epoll-EINTR/sleep-EINTR/events/sleeps={stats} PASS')
    finally:
        h.close()
    h = Host(library, threads='raw', host_name='host-storm')
    try:
        reply = h.request(v1(*patch_records(h)))
        assert reply['code'] == 'PARK_TIMEOUT' and reply['unregistered_threads'] >= 1, reply
        assert reply['attempts'] == 1 and reply['pages'] == 0 and reply['generation'] == 0, reply
        assert h.value() == 1
        print('raw clone: unregistered thread gives immediate PARK_TIMEOUT, separate count, no writes PASS')
    finally:
        h.close()

def task_race(library):
    h = Host(library, threads='race', host_name='host-storm', env={'REFRESH_TESTING_TASK_RACE': '1'})
    try:
        reply = h.request(v1(*patch_records(h)))
        assert reply['code'] == 'PARK_TIMEOUT' and reply['attempts'] == 2, reply
        assert reply['unregistered_threads'] == 1 and reply['pages'] == 0 and h.value() == 1, reply
        print('second listing: raw clone born after first listing is detected; retry refuses unknown stack, no writes PASS')
    finally:
        h.close()

if __name__ == '__main__':
    run(sys.argv[1])
    if len(sys.argv) > 2: task_race(sys.argv[2])
