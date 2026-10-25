#!/usr/bin/env python3
"""Exercise the real native owner with synthetic agent/systemd command boundaries."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

AGENT = r'''#!/usr/bin/env python3
import json,os,pathlib,sys
path=pathlib.Path(os.environ['JOURNAL_SETUP_AGENT_CONFIG'])
config=json.loads(path.read_text()) if path.exists() else {'unrelated':{'name':'unrelated','transport':{'type':'stdio','command':'preserve','args':[]}}}
args=sys.argv[1:]
if args==['mcp','list','--json']:print(json.dumps(list(config.values())))
elif args[:2]==['mcp','add']:
 index=args.index('--');name=args[index-1];launcher=args[index+1:]
 assert name not in config
 config[name]={'name':name,'enabled':True,'transport':{'type':'stdio','command':launcher[0],'args':launcher[1:],'env':None}}
 path.write_text(json.dumps(config));os.chmod(path,0o600)
else:sys.exit(2)
'''
SYSTEMCTL = r'''#!/usr/bin/env python3
import json,os,pathlib,shlex,signal,subprocess,sys,time
home=pathlib.Path(os.environ['HOME']);path=home/'service-processes.json'
state=json.loads(path.read_text()) if path.exists() else {}
args=sys.argv[1:];name=args[-1]
def stop():
 if name in state:
  try:os.kill(state[name],signal.SIGTERM)
  except ProcessLookupError:pass
  # A lock/filename is not a liveness check. The recorded child must have
  # exited or reached the kernel's zombie state before the replacement starts.
  deadline=time.monotonic()+5
  while True:
   try:text=pathlib.Path('/proc/'+str(state[name])+'/stat').read_text();stopped=text[text.rindex(')')+2:].split()[0]=='Z'
   except FileNotFoundError:stopped=True
   if stopped:break
   assert time.monotonic()<deadline
   time.sleep(.02)
  del state[name]
def start():
 body=(home/'.config/systemd/user'/name).read_text()
 launch=next(line.removeprefix('ExecStart=') for line in body.splitlines() if line.startswith('ExecStart='))
 with open(home/'service.log','ab') as log:
  child=subprocess.Popen(shlex.split(launch),stdin=subprocess.DEVNULL,stdout=log,stderr=log,start_new_session=True)
 state[name]=child.pid
if args==['--user','daemon-reload']:sys.exit(0)
if args[1]=='enable':
 if name not in state:start()
elif args[1]=='restart':stop();start()
elif args[1]=='disable':stop()
else:sys.exit(2)
path.write_text(json.dumps(state));os.chmod(path,0o600)
'''

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',required=True,type=Path)
    binary=parser.parse_args().binary.resolve()
    assert binary.is_file()
    with tempfile.TemporaryDirectory(prefix='ojo-service-check-') as temporary:
        root=Path(temporary);root.chmod(0o700)
        tools=root/'tools';tools.mkdir()
        for name,body in [('codex',AGENT),('systemctl',SYSTEMCTL)]:
            path=tools/name;path.write_text(body);path.chmod(0o700)
        home=root/'home';home.mkdir(mode=0o700)
        source=root/'sessions';source.mkdir()
        store=root/'store'
        env=dict(os.environ,HOME=str(home),PATH=str(tools)+os.pathsep+os.environ['PATH'],JOURNAL_SETUP_AGENT_CONFIG=str(home/'agent.json'))
        socket=None
        def run(command,*options,success=True):
            result=subprocess.run([str(binary),command,'--root',str(store),*options],env=env,capture_output=True,text=True,timeout=45)
            if success:assert result.returncode==0,result.stderr
            else:assert result.returncode!=0
            return json.loads(result.stdout) if success else result
        try:
            connected=run('connect','--adapter','codex','--source-root',str(source),'--accept-local-capture')
            assert connected['service_started'] and connected['mcp']['mcp_original_verified']
            assert not connected['mcp']['agent_tool_call_verified']
            status=run('status');assert status['owner']['healthy']
            socket=Path(status['owner']['socket'])
            assert socket.parent==Path('/tmp')/('ojo-journal-'+str(os.getuid())) and socket.suffix=='.sock'
            original_config=(store/'root.json').read_bytes();original_key=(store/'key').read_bytes()
            agent_config=(home/'agent.json').read_bytes()
            assert run('connect','--adapter','codex','--source-root',str(source),'--accept-local-capture')['mcp']['registered']
            assert (home/'agent.json').read_bytes()==agent_config
            installed=store/'bin/ojo-journal';older=store/'bin/.older-synthetic';older.write_bytes(installed.read_bytes()+b'\0');older.chmod(0o700);older.replace(installed)
            run('connect','--adapter','codex','--source-root',str(source),'--accept-local-capture',success=False)
            assert run('upgrade-service')['owner_verified']
            assert (store/'root.json').read_bytes()==original_config and (store/'key').read_bytes()==original_key
            assert installed.read_bytes()==binary.read_bytes()
            assert run('status')['owner']['healthy']
            assert (home/'agent.json').read_bytes()==agent_config
            assert run('get','--session',connected['verification']['session'],'--id',str(connected['verification']['sequence']))['input']['text']
            print(json.dumps({'managed_connect':True,'actual_owner_handshake':True,'accepted_parser_original':True,'registered_mcp_original':True,'idempotent_reconnect':True,'explicit_service_upgrade':True,'keys_config_and_agent_entries_preserved':True,'synthetic_only':True}))
        finally:
            if socket is None and (store/'root.json').exists():
                try:
                    status=subprocess.run([str(binary),'status','--root',str(store)],env=env,capture_output=True,text=True,timeout=10)
                    if status.returncode==0 and json.loads(status.stdout).get('owner'):
                        socket=Path(json.loads(status.stdout)['owner']['socket'])
                except (OSError,ValueError,subprocess.TimeoutExpired):pass
            state=home/'service-processes.json'
            if state.exists():
                for pid in json.loads(state.read_text()).values():
                    try:os.kill(pid,signal.SIGKILL)
                    except ProcessLookupError:pass
                    deadline=time.monotonic()+5
                    while True:
                        try:text=Path('/proc',str(pid),'stat').read_text();stopped=text[text.rindex(')')+2:].split()[0]=='Z'
                        except FileNotFoundError:stopped=True
                        if stopped:break
                        assert time.monotonic()<deadline,'owned test service did not stop'
                        time.sleep(.02)
            # The bound live handshake identified this exact owned endpoint.
            # Remove it only after observing our service process stopped.
            if socket is not None:
                socket.unlink(missing_ok=True)

if __name__=='__main__':
    main()
