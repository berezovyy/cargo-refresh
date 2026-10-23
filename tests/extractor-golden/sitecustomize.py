"""Task-local instrumentation; activated only by M2_GOLDENS."""
import os
if os.environ.get('M2_GOLDENS'):
 import sys,pathlib,gzip,json,hashlib,shutil
 V=pathlib.Path(os.environ['M2_V4'])
 sys.path.insert(0,str(V/'gw/scripts'))
 import manifest,scenarios
 root=pathlib.Path(os.environ['M2_GOLDENS']);root.mkdir(parents=True,exist_ok=True)
 original_update=manifest.State.update
 def update(self,data):
  result=original_update(self,data)
  if result:
   if self.generation==1:self.m2_history=[]
   self.m2_history.append(data)
  return result
 manifest.State.update=update
 original_send=manifest.ManifestExtractor.send
 def send(self,path,sock,artifact=None):
  # Freeze pre-application inputs; output comes from the real Python send.
  number=sum(p.is_dir() and p.name.isdigit() for p in root.iterdir());folder=root/f'{number:03d}';folder.mkdir()
  baseline=bytes(self.x.base.data)  # already normalized, equivalent to on-disk RELATIVE normalization
  digest=hashlib.sha256(baseline).hexdigest();basefile=root/(digest+'.elf.gz')
  if not basefile.exists():basefile.write_bytes(gzip.compress(baseline,compresslevel=1))
  for n,data in enumerate(self.state.m2_history):
   (folder/f'{n:03d}.manifest.gz').write_bytes(gzip.compress(data,compresslevel=1))
  (folder/'candidate.elf.gz').write_bytes(gzip.compress(pathlib.Path(path).read_bytes(),compresslevel=1))
  # Previous successful applies may precede the latest manifest in this epoch.
  meta={'functions':sorted(self.x.symbols),'baseline':basefile.name,'allow_tracing':self.x.allow_tracing,'previous':sorted(self.previous),
        'previous_data':{str(a):b.hex() for a,b in self.previous_data.items()},
        'applied_pages':{str(a):[p,k,b.hex(),list(f)] for a,p,k,b,f in self.applied_pages.values()}}
  (folder/'input.json').write_text(json.dumps(meta))
  result=original_send(self,path,sock,str(folder/'expected.wire'))
  (folder/'result.json').write_text(json.dumps(result))
  return result
 manifest.ManifestExtractor.send=send
 original_generate=scenarios.generate
 def generate(project):
  steps=list(original_generate(project))
  from copy import deepcopy
  apply=deepcopy(steps[0]);apply['kind']='handler-apply'
  revert=deepcopy(apply);revert['kind']='handler-revert';revert['sources'][scenarios.HAND]=(project/scenarios.HAND).read_text();revert['expected']['label']='S1:v1'
  return iter([apply,revert]+steps[:62])
 scenarios.generate=generate
