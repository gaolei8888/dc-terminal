#!/usr/bin/env python3
"""Privileged collector: descend beneath a trusted Docker volume using dirfds."""
import hashlib,importlib.machinery,importlib.util,json,os,re,sys
from pathlib import Path
loader=importlib.machinery.SourceFileLoader('dct_publish',str(Path(__file__).resolve().parent.parent/'bin'/'dct-publish'))
spec=importlib.util.spec_from_loader(loader.name,loader);module=importlib.util.module_from_spec(spec);loader.exec_module(module)
def main():
 work,state,project,directory=sys.argv[1:]
 if not (project=='current' or re.fullmatch('[a-f0-9]{32}',project)):raise ValueError('Invalid project')
 parts=directory.split('/')
 if directory=='.':parts=[]
 if any(not p or p.startswith('.') or '\\' in p or '\x00' in p for p in parts):raise ValueError('Invalid output directory')
 namespace='legacy' if project=='current' else 'library'
 root=work if project=='current' else state
 descend=parts if project=='current' else ['student-projects','work',project]+parts
 fd=os.open(root,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
 try:
  for component in descend:
   nextfd=os.open(component,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=fd);os.close(fd);fd=nextfd
  files,_=module.collect(None,root_fd=fd)
 finally:os.close(fd)
 relative=('/'.join(parts) or '.') if project=='current' else '/'.join([project]+parts)
 print(json.dumps({'projectKey':hashlib.sha256((namespace+':'+relative).encode()).hexdigest(),'files':files},ensure_ascii=False,separators=(',',':')))
if __name__=='__main__':
 try:main()
 except Exception:
  print('Cannot collect static output safely',file=sys.stderr);sys.exit(1)
