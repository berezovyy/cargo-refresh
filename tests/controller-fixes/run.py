#!/usr/bin/env python3
"""M4-T6 regressions. Hold the rustd lane; pass disposable disk directories.
The 100-save test is a correctness stress, with no latency/performance claim.
"""
import argparse, importlib.util, json, os, re, shutil, signal, threading, time
from pathlib import Path
from types import SimpleNamespace
P = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('session', P/'scripts/integration-session.py')
base = importlib.util.module_from_spec(spec); spec.loader.exec_module(base)

def fixture(args, name, extra='', extra_files=None, transform=lambda s:s):
    project = args.work/name
    shutil.copytree(P/'tests/tiny-axum', project, ignore=shutil.ignore_patterns('target'), dirs_exist_ok=True)
    source = transform((project/'src/main.rs').read_text()+extra)
    (project/'src/main.rs').write_text(source)
    for rel,text in (extra_files or {}).items(): (project/rel).write_text(text)
    cfg = SimpleNamespace(project=project, out=args.work/(name+'-run'+args.tag), controller=args.controller,
                          edit='', bin=None, strict=False)
    session=base.Session(cfg)
    if '[refresh] compiler ready' not in (session.out/'controller.log').read_text():
        session.close()
        raise RuntimeError('regression requires an admitted compiler baseline')
    return session, source

def write(s, files):
    # Stage outside the watched project, as a real editor's final rename.
    for rel, text in files.items():
        stage=s.out/'stage'; stage.write_text(text); stage.replace(s.project/rel)

def recovery(args):
    s, source = fixture(args, 'recovery')
    try:
        s.wait(lambda:base.http(s.port)==(200,'1'))
        first=s.save({'src/main.rs':source.replace('number: 1','number: 2')},'warm-live','2')
        assert first['path']=='live',first
        daemon=Path(next(e['message'] for e in s.read() if e['event']=='compiler_ready'))/'daemon.log'
        pid=int(re.search(r'of pid (\d+) listens',daemon.read_text()).group(1))
        assert b'rustd' in Path(f'/proc/{pid}/cmdline').read_bytes()
        os.kill(pid,signal.SIGKILL)
        failed=s.save({'src/main.rs':source.replace('number: 1','number: 3')},'forced-fallback','3')
        assert failed['path']=='stock',failed
        stamps=[]
        def burst():
            for i in range(100):
                stamps.append(time.monotonic())
                write(s,{'src/main.rs':source.replace('number: 1',f'number: {1000+i}')})
                time.sleep(.15)
        task=threading.Thread(target=burst);task.start();task.join()
        s.wait(lambda:base.http(s.port)==(200,'1099'),240)
        s.wait(lambda:any(e['event']=='save' and e['revision']>first['event']['revision'] for e in s.read()),240)
        events=s.read();live=[e for e in events if e['event']=='save' and e.get('path')=='live' and e['revision']>first['event']['revision']]
        assert live,'no return to live during 100 saves'
        resets=[e for e in events if e['event']=='worker_reset_start']
        assert len(resets)==1,resets
        assert not any(e['event']=='worker_reset_superseded' for e in events)
        row={'event':live[-1],'path':'live','kind':'rapid-recovery-final'};s.rows.append(row);s.oracle_tiny(row,1099)
        first_live_save=sum(t<=live[0]['monotonic'] for t in stamps)
        result={'writes':100,'cadence_seconds':.15,'reset_starts':len(resets),'live_events':len(live),'first_live_save':first_live_save,
                'first_live_revision':live[0]['revision'],'oracle_matches':1}
        (s.out/'completed.json').write_text(json.dumps(result,indent=2));print(result,flush=True)
    finally:s.close()

def cfg(args):
    extra='\n'+''.join(f'#[cfg({condition})]\nfn disabled_{i}() {{\n    let _ = {i};\n}}\n' for i,condition in enumerate(['not(unix)','windows','test']))
    extra+='\n#[cfg(test)]\nmod tests {\n    macro_rules! hidden { () => { 7 }; }\n}\n'
    s,source=fixture(args,'cfg',extra)
    try:
        s.wait(lambda:base.http(s.port)==(200,'1'))
        current=source
        for i in range(3):
            current=current.replace(f'let _ = {i};',f'let _ = {i+200};')
            row=s.save({'src/main.rs':current},f'cfg-disabled-{i}','1')
            assert row['path']=='skip' and row['event']['reason']=='no effect on this build',row
            s.oracle_tiny(row,1)
        current=current.replace('() => { 7 }', '() => { 90 }')
        row=s.save({'src/main.rs':current},'cfg-disabled-macro','1')
        assert row['path']=='skip' and row['event']['reason']=='no effect on this build',row
        s.oracle_tiny(row,1)
        current=current.replace('number: 1','number: 2')
        row=s.save({'src/main.rs':current},'enabled-after-noop','2')
        assert row['path']=='live',row;s.oracle_tiny(row,2)
        current=current.replace('number: u64 }','number: u64, flag: bool }').replace('number: 2 }','number: 2, flag: true }')
        row=s.save({'src/main.rs':current},'layout-after-noop','2')
        assert row['path']=='restart',row;s.oracle_tiny(row,2)
        current=current.replace('number: 2,','number: 3,')
        row=s.save({'src/main.rs':current},'live-after-layout','3')
        assert row['path']=='live',row;s.oracle_tiny(row,3)
        (s.out/'completed.json').write_text(json.dumps({'cfg_noops':4,'enabled_live':2,'layout_restart':1,'oracle_matches':7}))
    finally:s.close()

def history(args):
    s, source = fixture(args, 'history', '\n#[cfg(test)] mod tests { fn unused() {} }\n')
    try:
        s.wait(lambda:base.http(s.port)==(200,'1'))
        # Added lines defeat the older position-preserving cfg projection. Full
        # transactional code/data revalidation must authorize this no-op.
        current=source.replace('fn unused() {}', 'fn unused() {\n    assert_eq!(1, 1);\n}')
        row=s.save({'src/main.rs':current},'history-test-lines','1')
        assert row['path']=='skip',row;s.oracle_tiny(row,1)
        current += '\n// a history comment after the last item\n'
        row=s.save({'src/main.rs':current},'history-comment','1')
        assert row['path']=='skip',row;s.oracle_tiny(row,1)
        starts=sum(e['event']=='worker_reset_start' for e in s.read())
        # Unknown name diagnostics need not occur in the controller's historical
        # error-code allowlist. A failed stock fallback must retain the compiler.
        broken=current+'\nfn extra() { missing_crate::call(); }\n'
        row=s.save({'src/main.rs':broken},'history-error-retains-baseline',None)
        assert row['path']=='error',row;s.oracle_tiny(row,None,error=True)
        fixed=current.replace('number: 1','number: 2')
        row=s.save({'src/main.rs':fixed},'history-good-after-error','2')
        assert row['path']=='live',row;s.oracle_tiny(row,2)
        assert sum(e['event']=='worker_reset_start' for e in s.read())==starts
        # The stock-only include path fails before the daemon sees the bad save.
        row=s.save({'src/main.rs':fixed+'\ninclude!("missing_history_input.rs");\n'},'history-stock-error',None)
        assert row['path']=='error',row;s.oracle_tiny(row,None,error=True)
        row=s.save({'src/main.rs':fixed.replace('number: 2','number: 3')},'history-good-after-stock-error','3')
        assert row['path']=='live',row;s.oracle_tiny(row,3)
        assert sum(e['event']=='worker_reset_start' for e in s.read())==starts
        (s.out/'completed.json').write_text(json.dumps({'noops':2,'error_saves':2,'live_after_error':2,'oracle_matches':6,'resets':0}))
    finally:s.close()

def history_revert(args):
    # Delay only the frozen stock check, so a following save deterministically
    # supersedes an optimistic edit before its borrow-check result arrives.
    tools=args.work/'tools';tools.mkdir()
    cargo=shutil.which('cargo');assert cargo
    wrapper=tools/'cargo'
    wrapper.write_text('#!/usr/bin/env python3\nimport os,sys,time\n'
                       'if len(sys.argv)>1 and sys.argv[1]=="check":time.sleep(2)\n'
                       f'os.execv({cargo!r},[{cargo!r},*sys.argv[1:]])\n')
    wrapper.chmod(0o755)
    path=os.environ['PATH'];os.environ['PATH']=str(tools)+':'+path
    s=None
    try:
        s, source=fixture(args,'history-revert')
        s.wait(lambda:base.http(s.port)==(200,'1'))
        broken=source.replace('number: u64 }','number: u64, flag: bool }').replace('number: 1 }','number: 2, flag: true }')+'\nfn extra() { missing_crate::call(); }\n'
        row=s.save({'src/main.rs':broken},'optimistic-before-noop','2',error=True)
        assert row['path']=='restart',row
        assert not any(e['event']=='revert' for e in s.read()),'check delay did not exercise supersession'
        row=s.save({'src/main.rs':broken+'\n// later no-codegen save\n'},'checked-noop-rejects-prior-live',error=True)
        s.wait(lambda:any(e['event']=='revert' and e['revision']==row['event']['revision'] for e in s.read()))
        s.wait(lambda:base.http(s.port)==(200,'1'))
        assert row['path']=='error',row
        s.oracle_tiny(row,None,error=True)
        assert 'missing_crate' in (s.project/'src/main.rs').read_text(),'editor was overwritten'
        row=s.save({'src/main.rs':source.replace('number: 1','number: 3')},'good-after-noop-revert','3')
        assert row['path'] in ('live','restart'),row;s.oracle_tiny(row,3)
        assert not any(e['event']=='worker_reset_start' for e in s.read())
        (s.out/'completed.json').write_text(json.dumps({'optimistic_edits':1,'no_effect_rejections':1,'reverts':1,'oracle_matches':2,'resets':0}))
    finally:
        if s:s.close()
        os.environ['PATH']=path

def split(args):
    s,source=fixture(args,'split','\nmod extra;\n',
                     {'src/extra.rs':'pub fn offset() -> u64 { 0 }\n'},
                     lambda s:s.replace('Value { number: 1 }.number',
                                        'Value { number: 1 }.number + extra::offset()'))
    try:
        s.wait(lambda:base.http(s.port)==(200,'1'))
        # Split a source save into two close notifications. The compiler can see
        # the second file state before the watcher queues its later revision.
        last=1
        for n in range(2,32):
            before=max(e.get('revision',0) for e in s.read())
            current=source.replace('number: 1',f'number: {n}')
            write(s,{'src/main.rs':current})
            time.sleep([.003,.007,.015][n%3])
            write(s,{'src/extra.rs':f'pub fn offset() -> u64 {{ {n*100} }}\n'})
            s.wait(lambda:base.http(s.port)==(200,str(n*101)),120)
            s.wait(lambda:any(e['event']=='save' and e.get('revision',0)>before for e in s.read()),120)
            last=n*101
        events=s.read();stock=[e for e in events if e['event']=='save' and e.get('path')=='stock']
        assert not stock,stock
        reused=sum(e['event']=='compiler_pending_reused' for e in events)
        assert reused,'stress did not exercise pending manifest reuse'
        row={'kind':'split-final','path':'live'};s.rows.append(row);s.oracle_tiny(row,last)
        (s.out/'completed.json').write_text(json.dumps({'split_pairs':30,'pending_reused':reused,'superseded':sum(e['event']=='superseded' for e in events),'stock':len(stock),'oracle_matches':1}))
    finally:s.close()

if __name__=='__main__':
    ap=argparse.ArgumentParser();ap.add_argument('mode',choices=['recovery','cfg','history','history_revert','split']);ap.add_argument('--controller',type=Path,required=True);ap.add_argument('--work',type=Path,required=True);ap.add_argument('--tag',default='');args=ap.parse_args();args.work.mkdir(parents=True,exist_ok=True)
    globals()[args.mode](args)
