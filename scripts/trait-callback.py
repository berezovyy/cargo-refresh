#!/usr/bin/env python3
"""M4-T3 forwarded runtime callback regression. Hold the rustd/quiet lane."""
import argparse, importlib.util, json, os, signal, subprocess, sys
sys.dont_write_bytecode=True
from pathlib import Path
parser=argparse.ArgumentParser()
for option in ['project','out','controller']:parser.add_argument('--'+option,type=Path,required=True)
parser.add_argument('--allow-wrong',action='store_true')
parser.add_argument('--expect-path',choices=['live','restart','stock'])
a=parser.parse_args();a.edit='';a.bin=None;a.strict=False
spec=importlib.util.spec_from_file_location('integration',Path(__file__).with_name('integration-session.py'))
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
f=a.project/'src/main.rs';original=f.read_text();s=None;child=None;log=None
assert original.count('-> u64 { 1 }')==1, 'callback edit marker missing or ambiguous'
try:
 s=m.Session(a);s.wait(lambda:m.http(s.port)==(200,'1'))
 before=max((e.get('revision',0) for e in s.read()),default=0)
 f.write_text(original.replace('-> u64 { 1 }','-> u64 { 2 }'))
 event=s.wait(lambda:next((e for e in s.read() if e['event']=='save' and e.get('revision',0)>before),None))
 result,binary,env=s.stock();assert result.returncode==0,result.stdout
 port=m.free_port();log=(s.out/'oracle-app.log').open('w')
 child=subprocess.Popen([binary],cwd=s.project,env=env|{'PORT':str(port)},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
 stock=s.wait(lambda:m.http(port));assert stock==(200,'2'),stock
 actual=m.http(s.port)
 summary={'path':event['path'],'reason':event['reason'],'actual':actual,'stock':stock,'stock_match':actual==stock}
 (s.out/'completed.json').write_text(json.dumps(summary,indent=2));print(json.dumps(summary),flush=True)
 assert a.allow_wrong or actual==stock,summary
 assert a.expect_path is None or event['path']==a.expect_path,summary
finally:
 if child and child.poll() is None:os.killpg(child.pid,signal.SIGTERM);child.wait(10)
 if log:log.close()
 if s:s.close()
 f.write_text(original)
