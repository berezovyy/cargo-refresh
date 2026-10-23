from pathlib import Path
import collections,json,sys
root=Path(sys.argv[1]).resolve()
rows=[json.loads(l) for l in (root/'session/live-session/session.jsonl').read_text().splitlines()]
live=[r for r in rows if r['path']=='live']
dirs=sorted(p for p in (root/'goldens').iterdir() if p.is_dir() and (p/'result.json').exists())
assert len(dirs)==len(live)>=15,(len(dirs),len(live))
required={'comment-insert','new-helper','handler-apply','handler-revert'}
assert required <= {r['kind'] for r in live}
tracing=[];goldens=[]
for d,row in zip(dirs,live):
 result=json.loads((d/'result.json').read_text())
 assert result['ok']
 data=result['data_records']
 if any(r['kind']=='tracing-caller-metadata' for r in data):tracing.append(d.name)
 meta={'golden':d.name,'save':row['index'],'kind':row['kind'],'wire_bytes':result['wire_bytes'],'generation':result['manifest_generation'],'tracing_records':sum(r['kind']=='tracing-caller-metadata' for r in data),'restored_entries':len(result['restored']),'restored_data':len(result['data_restored'])}
 (d/'save.json').write_text(json.dumps(meta,indent=2));goldens.append(meta)
assert any(g['kind']=='comment-insert' and g['tracing_records'] for g in goldens),'line-shift must exercise tracing Metadata'
oracle=[json.loads(l) for l in (root/'session/stock-oracle/oracle.jsonl').read_text().splitlines()]
assert len(oracle)==len(rows)==28
assert all(r.get('exact_endpoint_match') for r in oracle)
size=sum(p.stat().st_size for p in (root/'goldens').rglob('*') if p.is_file())
assert size <= 300*1024*1024,size
summary={'saves':len(rows),'paths':dict(collections.Counter(r['path'] for r in rows)),'golden_count':len(dirs),'stock_matches':len(oracle),'golden_bytes':size,'tracing_goldens':tracing,'goldens':goldens}
(root/'capture-summary.json').write_text(json.dumps(summary,indent=2))
print(json.dumps({k:v for k,v in summary.items() if k!='goldens'},indent=2))
