#!/usr/bin/env python3
"""Real Linux fsync-EIO checks for root ancestors and existing metadata retries."""
import argparse
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile

SHIM = r'''
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int fsync(int fd) {
    static int (*actual)(int);
    if (!actual) actual = dlsym(RTLD_NEXT, "fsync");
    const char *target=getenv("OJO_JOURNAL_NAMESPACE_FAULT");
    char link[64],path[4096];
    snprintf(link,sizeof(link),"/proc/self/fd/%d",fd);
    ssize_t n=readlink(link,path,sizeof(path)-1);
    if(n>=0) {
        path[n]=0;
        if(target && strcmp(path,target)==0) { errno=EIO;return -1; }
    }
    return actual(fd);
}
'''


def snapshot(root):
    return {
        str(path.relative_to(root)): (
            stat.S_IMODE(path.stat().st_mode),
            path.read_bytes() if path.is_file() else None,
        )
        for path in [root, *sorted(root.rglob('*'))]
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    binary = parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix='ojo-app-namespace-') as temporary:
        scratch = Path(temporary)
        scratch.chmod(0o700)
        source = scratch / 'fault.c'
        source.write_text(SHIM)
        shim = scratch / 'fault.so'
        subprocess.run(['cc', '-shared', '-fPIC', '-O2', '-o', str(shim), str(source), '-ldl'], check=True, capture_output=True)
        results = []

        def run(root, command='init', *, plaintext=False, target=None, success=True):
            env = dict(os.environ)
            if target is not None:
                env.update(LD_PRELOAD=str(shim), OJO_JOURNAL_NAMESPACE_FAULT=str(target))
            arguments = [str(binary), command, '--root', str(root)]
            if plaintext:
                arguments.append('--plaintext')
            result = subprocess.run(arguments, env=env, capture_output=True, text=True, timeout=30)
            if success:
                assert result.returncode == 0, result.stderr
                return json.loads(result.stdout)
            assert result.returncode != 0, 'unconfirmed namespace or metadata returned an application acknowledgement'
            return result

        for plaintext in [True, False]:
            scope = scratch / ('raw' if plaintext else 'encrypted')
            scope.mkdir(mode=0o700)
            root = scope / 'ancestor' / 'child' / 'root'
            # The first failure can leave a visible ancestor; it cannot turn a
            # subsequent identical attempt into an acknowledgement.
            run(root, plaintext=plaintext, target=scope, success=False)
            assert (scope / 'ancestor').is_dir()
            assert not (root / 'root.json').exists()
            run(root, plaintext=plaintext, target=scope, success=False)
            run(root, plaintext=plaintext)
            before = snapshot(scope)
            run(root, plaintext=plaintext, target=scope, success=False)
            run(root, 'status', target=scope, success=False)
            assert snapshot(scope) == before, 'confirmation modified existing contents or permissions'
            run(root, plaintext=plaintext)
            assert snapshot(scope) == before
            results.append({'encrypted': not plaintext, 'fault': 'ancestor_directory', 'initial_and_reopened_ack_refused': True, 'existing_bytes_and_modes_preserved': True})

            # A fully written root.json may remain visible after its fsync
            # failed. Both init retry and ordinary open must confirm it.
            metadata_root = scope / 'metadata-root'
            target = metadata_root / 'root.json'
            run(metadata_root, plaintext=plaintext, target=target, success=False)
            before = snapshot(metadata_root)
            run(metadata_root, plaintext=plaintext, target=target, success=False)
            run(metadata_root, 'status', target=target, success=False)
            assert snapshot(metadata_root) == before
            run(metadata_root, plaintext=plaintext)
            run(metadata_root, 'status')
            assert snapshot(metadata_root) == before
            results.append({'encrypted': not plaintext, 'fault': 'root_metadata', 'initial_and_reopened_ack_refused': True, 'existing_bytes_and_modes_preserved': True})

        root = scratch / 'key-root'
        target = root / 'key'
        run(root, target=target, success=False)
        original_key = target.read_bytes()
        assert len(original_key) == 32 and not (root / 'root.json').exists()
        run(root, target=target, success=False)
        assert not (root / 'root.json').exists(), 'unconfirmed key was published into a usable root'
        assert target.read_bytes() == original_key
        run(root)
        before = snapshot(root)
        run(root, target=target, success=False)
        run(root, 'status', target=target, success=False)
        assert snapshot(root) == before and target.read_bytes() == original_key
        run(root)
        assert snapshot(root) == before
        results.append({'encrypted': True, 'fault': 'encryption_key', 'initial_and_reopened_ack_refused': True, 'original_key_preserved': True})
        print(json.dumps({'synthetic_only': True, 'real_linux_fsync_eio': True, 'cases': results}))


if __name__ == '__main__':
    main()
