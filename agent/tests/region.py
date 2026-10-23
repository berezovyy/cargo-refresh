"""Reservation ownership, allocator fallback, bounds and startup refusal."""
import os
from pathlib import Path
import sys
from integration import Host, BUILD, v1, record, mapping_permissions

def run(library):
    for name in ('host', 'host-pie'):
        h=Host(library,host_name=name)
        try:
            ping=h.request(v1(flags=1)); base=ping['load_base']
            assert ping['region_ok'] and ping['region_used']==0, ping
            start,end=ping['region_start'],ping['region_end']
            assert end-start==1<<30 and (start-base)%(2<<20)==0, ping
            assert mapping_permissions(h,[start,end-4096])==['---p','---p']
            assert h.command('m')=='allocated 335544320'
            assert h.value()==1 and h.request(v1(flags=1))['region_ok']
            for line in Path(f'/proc/{h.p.pid}/maps').read_text().splitlines():
                if '[heap]' in line and line.split()[1].startswith('rw'):
                    a,b=(int(x,16) for x in line.split()[0].split('-'))
                    assert b<=start or a>=end, line
            for target in (start-base-4096,end-base):
                r=h.request(v1(record(target,b'x',1,0)))
                assert r['code']=='MMAP_FAILED' and r['pages']==0, r
            r=h.request(v1(record(start-base,b'reserved!',1,0)))
            assert r['ok'] and r['region_used']==4096, r
            assert mapping_permissions(h,[start,start+4096])==['r--p','---p']
            assert h.request(v1(record(start-base,b'x',1,0)))['code']=='OVERLAP'
        finally:h.close()
        prefix=os.environ.get('REFRESH_TEST_ASAN_LIBRARY','')
        preload=':'.join(x for x in (prefix,str(Path(library).resolve()),str(BUILD/'block-region.so')) if x)
        h=Host(library,host_name=name,env={'LD_PRELOAD':preload,'REFRESH_TEST_BLOCK_REGION':'1'})
        try:
            ping=h.request(v1(flags=1))
            assert not ping['region_ok'] and ping['region_end']-ping['region_start']==1<<30,ping
            target=ping['region_start']-ping['load_base']
            r=h.request(v1(record(target,b'overwrite',1,0)))
            assert r['code']=='MMAP_FAILED' and not r['region_ok'] and r['attempts']==0,r
            assert mapping_permissions(h,[ping['region_start']])==['rw-p']
            with open(f'/proc/{h.p.pid}/mem','rb',buffering=0) as mem:
                mem.seek(ping['region_start']);assert mem.read(8)==b'unowned!'
            assert h.value()==1
        finally:h.close()
    print('region: PIE/non-PIE reservation, 320 MiB malloc/touch/free, bounds, owned patch, retained overlap, startup conflict preserves unowned sentinel PASS')
if __name__=='__main__':run(sys.argv[1])
