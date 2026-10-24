#!/usr/bin/env node
// Synthetic local CLI + actual NativeTemporalRecorder processes; no provider or real roots.
import {createHash} from 'node:crypto';
import {chmodSync,cpSync,mkdtempSync,readFileSync,rmSync,writeFileSync} from 'node:fs';
import {tmpdir} from 'node:os';
import {join,resolve} from 'node:path';
import {spawnSync} from 'node:child_process';
import {performance} from 'node:perf_hooks';
import assert from 'node:assert/strict';
import {NativeTemporalRecorder} from '../sdk/temporal/index.js';

const args=new Map();
for(let i=2;i<process.argv.length;i+=2)args.set(process.argv[i],process.argv[i+1]);
for(const key of ['--baseline','--candidate','--seed','--output'])if(!args.get(key))throw new Error('explicit binaries, seed builder and output required');
const binaries=Object.fromEntries(['baseline','candidate'].map(name=>[name,resolve(args.get('--'+name))]));
const seedBinary=resolve(args.get('--seed')),output=resolve(args.get('--output'));
const mode=args.get('--fixture-cache')??'warm',sizes=(args.get('--sizes')??'1000,10000').split(',').map(Number);
assert.ok(['warm','evict'].includes(mode)&&sizes.length&&sizes.every(n=>Number.isInteger(n)&&n>=20&&n<=10000));
const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('AWS_')&&!key.startsWith('OOJURO_')));
Object.assign(env,{AWS_CONFIG_FILE:'/dev/null',AWS_SHARED_CREDENTIALS_FILE:'/dev/null',AWS_EC2_METADATA_DISABLED:'true'});
for(const key of Object.keys(process.env))if(key.startsWith('AWS_')||key.startsWith('OOJURO_'))delete process.env[key];
Object.assign(process.env,{AWS_CONFIG_FILE:'/dev/null',AWS_SHARED_CREDENTIALS_FILE:'/dev/null',AWS_EC2_METADATA_DISABLED:'true'});
const sha=bytes=>createHash('sha256').update(bytes).digest('hex');
const stream={namespace:'synthetic',workflow_id:'workflow',run_id:'run',generation:'generation-1'};
const sourceId=sha(JSON.stringify(stream)).slice(0,32);
const eventStream={namespace:'synthetic',workflow_id:'workflow',run_id:'run'};
const eviction=`import os,pathlib,sys\nfor p in pathlib.Path(sys.argv[1]).rglob('*'):\n if p.is_file() and not p.is_symlink():\n  fd=os.open(p,os.O_RDONLY|os.O_NOFOLLOW)\n  try:os.posix_fadvise(fd,0,0,os.POSIX_FADV_DONTNEED)\n  finally:os.close(fd)\n`;
function evict(root){if(mode==='evict')assert.equal(spawnSync('python3',['-c',eviction,root],{env,encoding:'utf8'}).status,0);}
function call(binary,root,command,options=[],input){
 const started=performance.now();
 const result=spawnSync(binary,[command,'--root',root,...options.map(String)],{env,input,encoding:'utf8',timeout:60000,maxBuffer:2<<20});
 const elapsed=performance.now()-started;
 assert.equal(result.status,0,`synthetic ${command} refused`);
 return [JSON.parse(result.stdout),elapsed];
}
function distribution(values){const v=[...values].sort((a,b)=>a-b);return {count:v.length,p50_ms:(v[9]+v[10])/2,p95_ms:v[Math.ceil(v.length*.95)-1],min_ms:v[0],max_ms:v.at(-1),samples_ms:values};}
const report={schema:1,scope:'synthetic local Temporal native CLI and actual SDK fresh processes',fixture_cache:mode,
 order:['baseline','candidate','candidate','baseline'],sizes,samples:20,
 binaries:Object.fromEntries(Object.entries(binaries).map(([name,path])=>[name,{sha256:sha(readFileSync(path))}])),
 seed_binary_sha256:sha(readFileSync(seedBinary)),runs:[]};
const scratch=mkdtempSync(join(tmpdir(),'oojuro-temporal-abba-'));chmodSync(scratch,0o700);
try{
 const policy=join(scratch,'policy.json');
 writeFileSync(policy,JSON.stringify({version:1,record:true,paused:false,record_thinking:true,index_thinking:true,exclude:[],denied_tools:[]}),{mode:0o600});
 for(const size of sizes){
  const seedRoot=join(scratch,'seed-'+size);
  call(binaries.baseline,seedRoot,'setup',['--harness','temporal','--writer','temporal:synthetic-benchmark','--agent',7,'--policy',policy]);
  const seeded=spawnSync(seedBinary,[seedRoot,String(size)],{env,encoding:'utf8',timeout:180000,maxBuffer:1<<20});
  assert.equal(seeded.status,0,'bounded seed builder failed: '+seeded.stderr);
  const seed=JSON.parse(seeded.stdout);assert.equal(seed.records,size);assert.equal(seed.source_id,sourceId);
  const rawSeed=join(scratch,'seed-raw-'+size);
  call(binaries.baseline,seedRoot,'restore-raw',['--source-id',sourceId,'--out',rawSeed,'--allow-raw-export']);
  const initialRaw=readFileSync(rawSeed);assert.equal(sha(initialRaw),seed.raw_sha256);
  for(let order=0;order<report.order.length;order++){
   const name=report.order[order],binary=binaries[name],other=binaries[name==='baseline'?'candidate':'baseline'];
   const root=join(scratch,`run-${size}-${order}`);cpSync(seedRoot,root,{recursive:true});chmodSync(root,0o700);
   const recorder=new NativeTemporalRecorder({binary,root,generation:stream.generation,timeoutMs:60000});
   const times={native_append:[],sdk_record:[],position:[],get:[],search:[]},extra=[];
   let cursor=seed.bytes,sequence=size;
   for(let i=0;i<20;i++){
    const payload=`CLI synthetic needle${String(size+i).padStart(8,'0')} λ\r\n\0終`;
    const frame=JSON.stringify({version:1,id:'cli-new-'+i,sequence:++sequence,valid_us:123456,kind:'input',tool:null,operation_id:null,status:null,payload,attributes:{}})+'\n';
    const request=JSON.stringify({source_id:sourceId,stream,cursor,frame});
    evict(root);
    const [ack,elapsed]=call(binary,root,'temporal-append',['--allow-raw-source'],request);
    times.native_append.push(elapsed);assert.equal(ack.outcome.first_id,sequence);cursor=ack.end;extra.push(Buffer.from(frame));
    const [stored]=call(other,root,'get',['--id',sequence]);assert.equal(stored.output.event.text,JSON.stringify(payload));
    if(i===0){const [retry]=call(binary,root,'temporal-append',['--allow-raw-source'],request);assert.deepEqual(retry,ack);}
   }
   for(let i=0;i<20;i++){
    const payload=`SDK synthetic needle${String(size+20+i).padStart(8,'0')} λ\r\n\0終`;
    const event={id:'sdk-new-'+i,stream:eventStream,valid_us:123456,kind:'input',payload};
    evict(root);const began=performance.now();const ack=await recorder.record(event);times.sdk_record.push(performance.now()-began);
    assert.equal(ack.sequence,++sequence);assert.equal(ack.outcome.first_id,sequence);cursor=ack.end;
    extra.push(Buffer.from(JSON.stringify({version:1,id:event.id,valid_us:event.valid_us,kind:event.kind,tool:null,operation_id:null,status:null,payload,attributes:{},sequence})+'\n'));
   }
   for(let i=0;i<20;i++){
    evict(root);const [position,t]=call(binary,root,'temporal-position',[],JSON.stringify({source_id:sourceId,stream,event_id:'missing-'+i}));
    times.position.push(t);assert.equal(position.cursor,cursor);assert.equal(position.sequence,sequence);assert.equal(position.existing,null);
    const id=Math.floor(i*(size-1)/19)+1;
    evict(root);const [stored,getTime]=call(binary,root,'get',['--id',id]);times.get.push(getTime);
    assert.equal(stored.output.event.text,JSON.stringify(`synthetic Temporal needle${String(id-1).padStart(8,'0')} λ\r\n\0終`));
    evict(root);const [found,searchTime]=call(binary,root,'search',['--query',`needle${String(id-1).padStart(8,'0')}`]);times.search.push(searchTime);
    assert.deepEqual(found.hits.map(hit=>hit.id),[id]);
   }
   const restored=join(scratch,`restored-${size}-${order}`);
   call(other,root,'restore-raw',['--source-id',sourceId,'--out',restored,'--allow-raw-export']);
   const expected=Buffer.concat([initialRaw,...extra]),actual=readFileSync(restored);assert.deepEqual(actual,expected);
   report.runs.push({size,order,binary:name,seed,raw_sha256:sha(actual),timings:Object.fromEntries(Object.entries(times).map(([key,value])=>[key,distribution(value)])),
    correctness:'original payload text, exact native retry ACK, assigned IDs, source cursor/sequence, unique lexical hits, exact complete raw bytes, cross-binary reopen'});
   console.log(`qualified ${size} ${order} ${name}`);
  }
 }
 writeFileSync(output,JSON.stringify(report,null,2)+'\n');
}finally{rmSync(scratch,{recursive:true,force:true});}
