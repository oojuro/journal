#!/usr/bin/env python3
"""Real Linux fsync-EIO regression for application ACKs after verified reopen."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

SHIM = r'''
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>
int fsync(int fd) {
    static int (*actual)(int);
    if (!actual) actual = dlsym(RTLD_NEXT, "fsync");
    const char *target=getenv("OJO_JOURNAL_FAULT_FILE");
    const char *bound=getenv("OJO_JOURNAL_FAULT_AFTER");
    char link[64],path[4096];
    struct stat state;
    snprintf(link,sizeof(link),"/proc/self/fd/%d",fd);
    ssize_t n=readlink(link,path,sizeof(path)-1);
    if(n>=0) {
        path[n]=0;
        if(target && bound && strcmp(path,target)==0 && fstat(fd,&state)==0
           && state.st_size>strtoll(bound,0,10)) { errno=EIO;return -1; }
    }
    return actual(fd);
}
'''

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',required=True,type=Path)
    binary=parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix='ojo-app-sync-fault-') as temporary:
        scratch=Path(temporary);scratch.chmod(0o700)
        source=scratch/'fault.c';source.write_text(SHIM)
        shim=scratch/'fault.so'
        subprocess.run(['cc','-shared','-fPIC','-O2','-o',str(shim),str(source),'-ldl'],check=True,capture_output=True)
        results=[]
        for plaintext in [True,False]:
            root=scratch/('raw' if plaintext else 'encrypted')
            def run(command,*options,env=None,success=True):
                result=subprocess.run([str(binary),command,'--root',str(root),*options],env=env,capture_output=True,text=True,timeout=30)
                if success:assert result.returncode==0,result.stderr
                else:assert result.returncode!=0,'unproven fsync returned an application acknowledgement'
                return json.loads(result.stdout) if success else result
            run('init',*(['--plaintext'] if plaintext else []))
            original=scratch/'original.txt';original.write_text('Synthetic seed original\n')
            args=['--session-id','sync-fault-session','--event-id','seed','--text-file',str(original)]
            first=run('record',*args)
            target=root/'journals'/(first['session']+'.ojo');before=target.stat().st_size
            original.write_text('Synthetic durable exact café\0 original\r\n')
            args[3]='unknown-sync-event'
            fault=dict(os.environ,LD_PRELOAD=str(shim),OJO_JOURNAL_FAULT_FILE=str(target),OJO_JOURNAL_FAULT_AFTER=str(before))
            run('record',*args,env=fault,success=False)
            uncertain=target.read_bytes();assert len(uncertain)>before,'fault did not follow a completed append'
            run('record',*args,env=fault,success=False)
            assert target.read_bytes()==uncertain,'failed confirmation rewrote the original intent'
            confirmed=run('record',*args);assert confirmed['stored']==0
            assert target.read_bytes()==uncertain,'exact acknowledged retry appended a duplicate'
            record=run('get','--session',first['session'],'--id','1')
            assert record['input']['text']==original.read_bytes().decode()
            assert run('status')['sessions'][0]['records']==2
            results.append({'encrypted':not plaintext,'initial_sync_error_refused_ack':True,'reopened_sync_error_refused_ack':True,'confirmed_retry_stored':0,'exact_original_and_bytes_preserved':True})
        print(json.dumps({'synthetic_only':True,'real_linux_fsync_eio':True,'cases':results}))

if __name__=='__main__':main()
