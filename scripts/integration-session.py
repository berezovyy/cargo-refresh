#!/usr/bin/env python3
"""Hand-run end-to-end controller checks. Hold the rustd lane around this script.
Copies fixtures before invoking; project must be a disposable work directory.
"""
import argparse,json,os,pathlib,signal,socket,subprocess,sys,time,urllib.request,urllib.error

def free_port():
    with socket.socket() as s:s.bind(('127.0.0.1',0));return s.getsockname()[1]
def http(port,path='/'):
    try:
        with urllib.request.urlopen(f'http://127.0.0.1:{port}{path}',timeout=.3) as r:return r.status,r.read().decode()
    except (OSError,urllib.error.URLError):return None
class Session:
    def __init__(self,args,app_args=(),env=None):
        self.args=args;self.project=args.project.resolve();self.out=args.out.resolve();self.out.mkdir(parents=True,exist_ok=False)
        self.events=self.out/'events.jsonl';self.port=free_port();self.rows=[]
        self.env=os.environ|{'RUSTUP_TOOLCHAIN':'1.99.0','PORT':str(self.port),'REFRESH_MEMORY_MIN_BYTES':str(6*1024**3),'TOKIO_WORKER_THREADS':'2'}|(env or {})
        self.log=(self.out/'controller.log').open('w');self.revision=0
        command=[str(args.controller.resolve()),'refresh','run','--events',str(self.events),'-v']
        if args.edit:command+=['--edit',args.edit]
        if args.bin:command+=['--bin',args.bin,'--package',args.bin]
        if args.strict:command+=['--strict']
        if app_args:command+=['--',*[value.replace('{PORT}',str(self.port)) for value in app_args]]
        samples=[]
        while True:
            load=pathlib.Path('/proc/loadavg').read_text().strip();mem=next(l for l in pathlib.Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemAvailable:'))
            samples.append({'time':time.time(),'loadavg':load,'mem_available':mem});(self.out/'preflight.json').write_text(json.dumps(samples,indent=2))
            if float(load.split()[0])<=2:break
            time.sleep(5)
        self.proc=subprocess.Popen(command,cwd=self.project,env=self.env,stdout=self.log,stderr=subprocess.STDOUT)
        try:self.wait(lambda:any(r['event']=='ready' for r in self.read()),300)
        except BaseException:self.close();raise
    def read(self):
        if not self.events.exists():return []
        text=self.events.read_text();lines=text.splitlines()
        if not text.endswith('\n'):lines=lines[:-1]
        return [json.loads(line) for line in lines]
    def application_pid(self):
        children=set()
        for path in pathlib.Path(f'/proc/{self.proc.pid}/task').glob('*/children'):
            try:children.update(path.read_text().split())
            except FileNotFoundError:pass
        found=[]
        for pid in children:
            try:
                executable=os.readlink(f'/proc/{pid}/exe')
                if executable.startswith('/tmp/refresh-run-') and executable.endswith('/app'):found.append(int(pid))
            except FileNotFoundError:pass
        assert len(found)==1,found
        return found[0]
    def wait(self,predicate,timeout=120):
        end=time.monotonic()+timeout
        while time.monotonic()<end:
            result=predicate()
            if result:return result
            if self.proc.poll() is not None:raise RuntimeError('controller exited: '+(self.out/'controller.log').read_text()[-6000:])
            time.sleep(.01)
        raise TimeoutError((self.out/'controller.log').read_text()[-6000:])
    def save(self,files,kind,value=None,error=False,path='/'):
        before=max((r.get('revision',0) for r in self.read()),default=0)
        for rel,content in files.items():(self.project/rel).write_text(content)
        started=time.monotonic()
        event=self.wait(lambda:next((r for r in self.read() if r['event']=='save' and r['revision']>before),None))
        if error:
            if kind=='borrow-error':
                # Optimistic delivery may occur; the checked snapshot must be restored.
                self.wait(lambda:any(r['event'] in ('revert','check_failed') and r.get('revision')==event['revision'] for r in self.read()) or event['path']=='error')
            assert event['path'] in ('waiting','error','live','restart','stock'),event
        if value is not None:self.wait(lambda:(r:=http(self.port,path)) and value in r[1])
        observed=time.monotonic();self.revision=event['revision']
        if not error and event['path'] in ('live','restart') and not self.args.strict:
            self.wait(lambda:any(r['event']=='check_ok' and r.get('revision')==event['revision'] for r in self.read()))
        row={'kind':kind,'event':event,'path':event['path'],'delivered':True,'visible_ms':(observed-started)*1000,'first_probe_ms':observed*1000,'loadavg':pathlib.Path('/proc/loadavg').read_text().strip(),'mem_available':next(l for l in pathlib.Path('/proc/meminfo').read_text().splitlines() if l.startswith('MemAvailable:'))}
        self.rows.append(row);self.flush();return row
    def stock(self):
        target=self.project/'target/refresh/oracle';command=['nice','-n','19','cargo','build','--offline','-j','4','--message-format=json']
        if self.args.bin:command+=['--bin',self.args.bin]
        if lane:=os.environ.get('REFRESH_LANE_TOOL'):command=[lane,'build','--',*command]
        env={k:v for k,v in self.env.items() if k not in ('RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','LD_PRELOAD','CARGO_BUILD_TARGET')}
        env['CARGO_TARGET_DIR']=str(target)
        result=subprocess.run(command,cwd=self.project,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True,timeout=240)
        (self.out/f'{len(self.rows):02d}-stock.log').write_text(result.stdout)
        binary=next((json.loads(l).get('executable') for l in result.stdout.splitlines() if l.startswith('{') and json.loads(l).get('executable')),None)
        return result,binary,env
    def oracle_tiny(self,row,expected,error=False):
        result,binary,env=self.stock()
        if error:assert result.returncode!=0,result.stdout;row['oracle_match']=True
        else:
            assert result.returncode==0,result.stdout
            port=free_port();log=(self.out/f'{len(self.rows):02d}-oracle-app.log').open('w')
            child=subprocess.Popen([binary],cwd=self.project,env=env|{'PORT':str(port)},stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
            try:
                response=self.wait(lambda:http(port),15);actual=http(self.port)
                assert response==actual,(response,actual);assert response==(200,str(expected)),response;row['oracle_match']=True
            finally:os.killpg(child.pid,signal.SIGTERM);child.wait(10);log.close()
        self.flush()
    def flush(self):
        temporary=self.out/'session.tmp';temporary.write_text(json.dumps(self.rows,indent=2));temporary.replace(self.out/'session.json')
    def close(self):
        if self.proc.poll() is None:self.proc.terminate()
        try:self.proc.wait(20)
        except subprocess.TimeoutExpired:self.proc.kill();self.proc.wait()
        self.log.close()

def tiny(args):
    s=Session(args)
    source=(s.project/'src/main.rs').read_text();manifest=(s.project/'Cargo.toml').read_text()
    try:
        s.wait(lambda:http(s.port)==(200,'1'))
        def save(kind,text,value,error=False,extra=None):
            nonlocal source
            source=text;row=s.save({'src/main.rs':source}|(extra or {}),kind,str(value),error)
            allowed={'body':('live',),'new-helper':('live',),'comment-top':('live','restart'),'struct-field':('restart','stock'),'async-handler':('restart','stock'),'syntax-error':('waiting',),'borrow-error':('live','error'),'cargo-manifest':('stock',),'identical':('skip',),'revert':('live','stock')}
            assert row['path'] in allowed[kind],(kind,row)
            s.oracle_tiny(row,value,error);print(kind,row['path'],'PASS',flush=True);return row
        save('body',source.replace('number: 1','number: 2'),2)
        save('new-helper',source.replace('number: 2','number: helper()')+'\nfn helper() -> u64 { 3 }\n',3)
        save('comment-top','// Saved comment.\n'+source,3)
        save('struct-field',source.replace('number: u64 }','number: u64, flag: bool }').replace('number: helper() }','number: helper(), flag: true }'),3)
        save('async-handler',source.replace('value().to_string()','(value() + 1).to_string()'),4)
        good=source;save('syntax-error',source+'\nfn broken( {\n',4,True)
        bad=good.replace('fn helper() -> u64 { 3 }','fn helper() -> u64 { let number = 6; let borrowed = &mut number; *borrowed }')
        save('borrow-error',bad,4,True);assert (s.project/'src/main.rs').read_text()==bad,'auto-revert rewrote editor source'
        save('cargo-manifest',good.replace('fn helper() -> u64 { 3 }','fn helper() -> u64 { 7 }'),8,extra={'Cargo.toml':manifest+'\n# manifest save\n'})
        save('identical',source,8)
        save('revert',source.replace('fn helper() -> u64 { 7 }','fn helper() -> u64 { 3 }'),4)
        assert len(s.rows)==10
        assert not any(r['event']=='application_exit' for r in s.read()),'application crashed'
        (s.out/'completed.json').write_text(json.dumps({'saves':10,'oracle_matches':10,'paths':[r['path'] for r in s.rows]}))
    finally:s.close()

def request(port,spec):
    data=json.dumps(spec['body']).encode() if 'body' in spec else None
    req=urllib.request.Request(f"http://127.0.0.1:{port}{spec['path']}",method=spec['method'],data=data,headers={'content-type':'application/json'})
    try:response=urllib.request.urlopen(req,timeout=2)
    except urllib.error.HTTPError as e:response=e
    with response:
        text=response.read().decode()
        try:body=json.loads(text)
        except ValueError:body=text
        return {'status':response.status,'body':body}
def checkpoint(path):
    import struct
    documents=[]
    with path.open('rb') as f:
        for offset in (0,4*1024*1024):
            f.seek(offset);header=f.read(32)
            if header[:8]!=b'STARENA1':continue
            generation,n,wanted=struct.unpack_from('<QQQ',header,8);payload=f.read(n);digest=0xcbf29ce484222325
            for byte in payload:digest=((digest^byte)*0x100000001b3)&((1<<64)-1)
            if digest==wanted:documents.append((generation,json.loads(payload)))
    return max(documents,key=lambda d:d[0])[1]
def write_arena(path,document):
    import struct
    payload=json.dumps(document,separators=(',',':')).encode();digest=0xcbf29ce484222325
    for byte in payload:digest=((digest^byte)*0x100000001b3)&((1<<64)-1)
    with path.open('wb') as f:f.truncate(8*1024*1024);f.seek(0);f.write(b'STARENA1'+struct.pack('<QQQ',1,len(payload),digest)+payload)
def canonical(responses):
    import copy
    responses=copy.deepcopy(responses)
    responses[0]['body']='\n'.join(line for line in responses[0]['body'].splitlines() if not line.startswith(('pid=','ticks=')))
    return responses

def web(args):
    import importlib.util
    sys.dont_write_bytecode=True
    spec=importlib.util.spec_from_file_location('scenarios',args.scenarios);scenarios=importlib.util.module_from_spec(spec);spec.loader.exec_module(scenarios)
    steps=list(scenarios.generate(args.project));assert len(steps)==64
    arena=args.out.resolve()/'arena'
    s=Session(args,env={'STATE_ARENA':str(arena),'RUST_LOG':'off'})
    try:
        s.wait(lambda:http(s.port,'/__probe'))
        ids={}
        for resource,body in zip(scenarios.RESOURCES,scenarios.SEEDS):
            response=request(s.port,{'method':'POST','path':'/v1/'+resource,'body':body});assert response['status']==201,response;ids[resource]=response['body']['data']['id']
        for index,step in enumerate(steps,1):
            expected=scenarios.probe_value(step['expected'])
            # Invalid snapshots retain the last checked response, including optimistic borrow edits.
            previous=request(s.port,{'method':'GET','path':'/__probe'})
            target=(dict(line.split('=',1) for line in previous['body'].splitlines() if '=' in line)['probe'] if step.get('error') else expected)
            files={rel:text for rel,text in step['sources'].items() if (s.project/rel).read_text()!=text}
            row=s.save(files,step['kind'],target,bool(step.get('error')),path='/__probe')
            if not step.get('error'):
                specs=scenarios.requests(step['expected'],ids)
                responses=[request(s.port,spec) for spec in specs];scenarios.validate(responses,step['expected'],ids)
                doc=checkpoint(arena);(s.out/f'{index:02d}-checkpoint.json').write_text(json.dumps(doc))
            result,binary,env=s.stock()
            if step.get('error'):
                assert result.returncode!=0,result.stdout
                row['oracle_match']=True
                if step['kind']=='borrow-error':assert (s.project/scenarios.HAND).read_text()==step['sources'][scenarios.HAND]
            else:
                assert result.returncode==0,result.stdout
                stock_arena=s.out/'stock-arena';write_arena(stock_arena,doc)
                req=s.out/'requests.json';req.write_text(json.dumps(specs))
                oracle=subprocess.run([binary],cwd=s.project,env=env|{'STATE_ARENA':str(stock_arena),'FAST_REFRESH_TEST_REQUESTS':str(req),'RUST_LOG':'off'},stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,timeout=20)
                assert oracle.returncode==0,oracle.stderr
                stock=json.loads(oracle.stdout);assert canonical(stock)==canonical(responses),(index,canonical(stock),canonical(responses))
                row['oracle_match']=True;row['responses']=responses
            s.flush();print(index,step['kind'],row['path'],'PASS',flush=True)
        assert not any(r['event']=='application_exit' for r in s.read()),'application crashed'
        (s.out/'completed.json').write_text(json.dumps({'saves':64,'valid':62,'oracle_matches':64,'paths':{path:sum(r['path']==path for r in s.rows) for path in set(r['path'] for r in s.rows)}}))
    finally:s.close()

def mdbook(args):
    rendered(args, 'mdbook')

def zola(args):
    rendered(args, 'zola')

def rendered(args, app):
    import tomllib
    session=tomllib.loads(args.session.read_text());steps=session['saves'];assert len(steps)==30
    book=args.book.resolve()
    if app=='mdbook':
        app_args=['serve',str(book),'--hostname','127.0.0.1','--port','{PORT}'];probe='/chapter.html'
    else:
        app_args=['--root',str(book),'serve','--interface','127.0.0.1','--port','{PORT}','--output-dir',str(book/'output'),'--force'];probe='/'
    s=Session(args,app_args=app_args,env={'CFLAGS':'-std=gnu17','CARGO_PROFILE_DEV_DEBUG':'0','RUST_LOG':'off'})
    try:
        s.wait(lambda:http(s.port,probe))
        for step in steps:
            previous_pid=s.application_pid()
            files={}
            for edit in step['edits']:
                path=edit['file'];text=files.get(path,(s.project/path).read_text());assert text.count(edit['find'])==1,(step['id'],edit)
                files[path]=text.replace(edit['find'],edit['replace'],1)
            row=s.save(files,step['kind'],step['observation']['contains'],step.get('expect_build')=='error',path=probe)
            expected=json.loads((args.expected/f"{step['id']:02d}.json").read_text())
            observed=http(s.port,probe);assert observed==(expected['observations'][0]['status'],expected['observations'][0]['body']),(step['id'],'response differs from stock',observed)
            row['application_pid']=s.application_pid()
            if step.get('expect_build')!='error':
                assert (row['application_pid']==previous_pid)==(row['path']=='live'),(step['id'],row['path'],previous_pid,row['application_pid'])
            row['oracle_match']=True;s.flush();print(step['id'],step['kind'],row['path'],'PASS',flush=True)
        assert not any(r['event']=='application_exit' for r in s.read()),'application crashed'
        (s.out/'completed.json').write_text(json.dumps({'saves':30,'valid':28,'oracle_matches':30,'paths':{path:sum(r['path']==path for r in s.rows) for path in set(r['path'] for r in s.rows)}}))
    finally:s.close()

def resilience(args):
    import re
    s=Session(args)
    source=(s.project/'src/main.rs').read_text()
    def text(value):return source.replace('number: 1',f'number: {value}')
    try:
        s.wait(lambda:http(s.port)==(200,'1'))
        first=s.save({'src/main.rs':text(2)},'warm-live','2');assert first['path']=='live',first
        daemon=pathlib.Path(next(r['message'] for r in s.read() if r['event']=='compiler_ready'))/'daemon.log'
        pid=int(re.search(r'of pid (\d+) listens',daemon.read_text()).group(1))
        command=pathlib.Path(f'/proc/{pid}/cmdline').read_bytes();assert b'rustd' in command and b'daemon' in command,command
        os.kill(pid,signal.SIGKILL)
        failed=s.save({'src/main.rs':text(3)},'daemon-killed','3');assert failed['path']=='stock',failed
        recovering=s.save({'src/main.rs':text(4)},'save-during-reset','4');assert recovering['path']=='stock',recovering
        s.wait(lambda:any(r['event']=='worker_reset' and r.get('revision')==recovering['event']['revision'] for r in s.read()),240)
        baseline=s.save({'src/main.rs':text(5)},'recovery-baseline','5');assert baseline['path']=='restart',baseline
        live=s.save({'src/main.rs':text(6)},'recovered-live','6');assert live['path']=='live',live
        before=len([r for r in s.read() if r['event']=='save'])
        for value in range(100,200):(s.project/'src/main.rs').write_text(text(value))
        (s.project/'src/main.rs').write_text(text(200))
        s.wait(lambda:http(s.port)==(200,'200'),240)
        final=s.wait(lambda:next((r for r in reversed(s.read()) if r['event']=='save' and r['revision']>live['event']['revision'] and r['path'] in ('live','restart','stock')),None))
        row={'kind':'rapid-saves-final','path':final['path'],'event':final,'delivered':True};s.rows.append(row);s.oracle_tiny(row,200)
        assert len([r for r in s.read() if r['event']=='save'])-before<=101
        assert not any(r['event']=='application_exit' for r in s.read()),'application crashed'
        s.flush();(s.out/'completed.json').write_text(json.dumps({'daemon_kill_fallback':True,'saves_during_reset':True,'recovered_live':True,'rapid_final_value':200,'oracle_matches':1}))
    finally:s.close()

def strict(args):
    import shutil
    fixture=pathlib.Path(__file__).resolve().parent.parent/'tests/tiny-axum'
    for name in ['Cargo.toml','src/main.rs']:shutil.copyfile(fixture/name,args.project/name)
    args.strict=True;s=Session(args);source=(s.project/'src/main.rs').read_text()
    try:
        s.wait(lambda:http(s.port)==(200,'1'))
        live=s.save({'src/main.rs':source.replace('number: 1','number: 2')},'strict-live','2');assert live['path']=='live',live;s.oracle_tiny(live,2)
        bad=source.replace('fn value() -> u64 { Value { number: 1 }.number }','fn value() -> u64 { let number = 6; let borrowed = &mut number; *borrowed }')
        error=s.save({'src/main.rs':bad},'borrow-error','2',True);assert error['path']=='error',error
        assert 'build_start' not in error['event']['phases'] and 'patch_sent' not in error['event']['phases']
        assert (s.project/'src/main.rs').read_text()==bad
        assert not any(r['event'] in ('tainted','revert','application_exit') for r in s.read())
        s.oracle_tiny(error,2,True)
        (s.out/'completed.json').write_text(json.dumps({'strict_borrow_error_blocked_before_build':True,'editor_preserved':True,'oracle_matches':2}))
    finally:s.close()

def triggers(args):
    """Exercise post-ACK ordering, rejection/skip, and failed notification restart."""
    import http.server, threading
    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            assert self.path=='/refresh',self.path
            server.observed.append(http(server.app_port))
            self.send_response(server.status);self.end_headers()
        def log_message(self,*unused):pass
    server=http.server.HTTPServer(('127.0.0.1',0),Handler)
    server.status=204;server.observed=[]
    task=threading.Thread(target=server.serve_forever,daemon=True);task.start()
    source=(args.project/'src/main.rs').read_text();manifest=args.project/'Cargo.toml'
    manifest.write_text(manifest.read_text()+f'\n[package.metadata.refresh]\nafter_patch_touch = ["refresh-input.md"]\nafter_patch_http = "http://127.0.0.1:{server.server_port}/refresh"\n')
    touched=args.project/'refresh-input.md';touched.write_bytes(b'Unchanged input.\n')
    s=None
    try:
        s=Session(args);server.app_port=s.port;s.wait(lambda:http(s.port)==(200,'1'))
        for value,status,path in [(2,204,'live'),(3,503,'restart'),(4,204,'live')]:
            server.status=status
            row=s.save({'src/main.rs':source.replace('number: 1',f'number: {value}')},'notification',str(value))
            assert row['path']==path,row
            assert server.observed[-1]==(200,str(value)),server.observed
            assert touched.read_bytes()==b'Unchanged input.\n'
            s.oracle_tiny(row,value)
        assert len(server.observed)==3,server.observed
        current=(s.project/'src/main.rs').read_text()
        rejected=s.save({'src/main.rs':current+'\nfn incomplete( {\n'},'syntax-error','4',True)
        assert rejected['path']=='waiting',rejected;s.oracle_tiny(rejected,4,True)
        restored=s.save({'src/main.rs':current},'identical','4');assert restored['path']=='skip',restored
        assert len(server.observed)==3,server.observed
        touched.unlink()
        failed=s.save({'src/main.rs':source.replace('number: 1','number: 5')},'missing-touch','5')
        assert failed['path']=='restart',failed;s.oracle_tiny(failed,5)
        assert len(server.observed)==3 and not touched.exists()
        assert sum(r['event']=='after_patch_failed' for r in s.read())==2
        assert not any(r['event']=='application_exit' for r in s.read())
        (s.out/'completed.json').write_text(json.dumps({'post_ack_gets':server.observed,'failure_restarts':2,'rejected_and_skip_did_not_notify':True,'touch_content_preserved':True,'oracle_matches':5}))
    finally:
        if s:s.close()
        server.shutdown();server.server_close();task.join()

def main():
    parser=argparse.ArgumentParser();parser.add_argument('mode',choices=['tiny','web','mdbook','zola','resilience','strict','triggers']);parser.add_argument('--controller',type=pathlib.Path,required=True);parser.add_argument('--project',type=pathlib.Path,required=True);parser.add_argument('--out',type=pathlib.Path,required=True);parser.add_argument('--edit',default='');parser.add_argument('--strict',action='store_true');parser.add_argument('--bin');parser.add_argument('--scenarios',type=pathlib.Path);parser.add_argument('--session',type=pathlib.Path);parser.add_argument('--expected',type=pathlib.Path);parser.add_argument('--book',type=pathlib.Path);args=parser.parse_args();globals()[args.mode](args)
if __name__=='__main__':main()
