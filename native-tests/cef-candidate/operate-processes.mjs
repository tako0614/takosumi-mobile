// Compare process-per-profile reuse of the same native CEF browser services.
// This is an experimental adapter boundary, not the installed Tauri runtime.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import { OwnedCefRuntime } from './runtime.mjs';
import { PROFILE_IDS } from './profiles.mjs';
import { inspectPermissions } from './permissions.mjs';
import { startNotificationBackend } from './notification-backend.mjs';
import { probeNotifications } from './notifications.mjs';

const [data,app,mode,binary,files,notifierBinary]=process.argv.slice(2);
const urls={photopea:'https://www.photopea.com/',excalidraw:'https://excalidraw.com/'};
assert(data&&path.isAbsolute(data)&&urls[app]&&mode==='site-notifications'&&files==='cdp');
assert(binary&&path.isAbsolute(binary)&&notifierBinary&&path.isAbsolute(notifierBinary));
assert.equal(process.env.TAKOSUMI_CEF_RUNNER_CASE,data,'Use the owned Python runner');
const nonce=(await readFile(path.join(data,'.owned-browser-reference'),'utf8')).trim();
assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(nonce));
const env={...Object.fromEntries(Object.entries(process.env).filter(([key])=>['PATH','HOME','USER','LOGNAME','LANG','LC_ALL','LC_CTYPE','DISPLAY','XAUTHORITY','DBUS_SESSION_BUS_ADDRESS'].includes(key))),PYTHONDONTWRITEBYTECODE:'1',LD_LIBRARY_PATH:path.dirname(binary)};
for(const [key,name] of Object.entries({XDG_DATA_HOME:'data',XDG_CONFIG_HOME:'config',XDG_CACHE_HOME:'cache',XDG_RUNTIME_DIR:'runtime',TMPDIR:'tmp'})) {
  env[key]=path.join(data,name);await mkdir(env[key],{mode:0o700});
}
async function desktop(tool,args,allowMissing=false) {
  const child=spawn(tool,args,{env,stdio:['ignore','pipe','pipe']});
  let out='',err='';child.stdout.on('data',chunk=>out+=chunk);child.stderr.on('data',chunk=>err+=chunk);
  const timer=setTimeout(()=>child.kill('SIGKILL'),4000);
  const status=await new Promise((resolve,reject)=>{child.once('exit',resolve);child.once('error',reject);}).finally(()=>clearTimeout(timer));
  assert(status===0||(allowMissing&&status===1),`${tool} failed: ${err}`);return out.trim();
}
const runtimes=PROFILE_IDS.map((profile,index)=>new OwnedCefRuntime({data:path.join(data,index===0?'w':'p'),binary,env,nonce,
  request:{key:`web-home:${profile}:${new URL(urls[app]).origin}`,url:urls[app],title:'Native CEF candidate',profile,webHome:true,resume:false}}));
const receipt={engine:'standalone CEF154 candidate',processLayout:'per-profile',app,mode,nonce,profilePassed:false,permissionPassed:false,notificationUiPassed:false,pushPassed:false,runtimes:runtimes.map(runtime=>runtime.proof)};
const stage=async name=>{receipt.stage=name;await writeFile(path.join(data,'progress.json'),JSON.stringify(receipt,null,2)+'\n');};
let backend,error;
try {
  await stage('owned notification backend');
  backend=await startNotificationBackend({binary:notifierBinary,data,nonce,env,desktop});
  await stage('two independent native runtimes');
  // Both guard handles are retained before waiting; partial startup still
  // takes the same finally path for every already-owned process.
  const started=await Promise.allSettled(runtimes.map(runtime=>runtime.start()));
  for(const result of started)if(result.status==='rejected')throw result.reason;
  assert.equal(new Set(runtimes.map(runtime=>runtime.nativePid)).size,2,'Actual native PIDs must differ');
  const deadline=Date.now()+25000;
  const pages=await Promise.all(runtimes.map(runtime=>runtime.bind(deadline)));
  assert.notEqual(pages[0].native.nativeCache,pages[1].native.nativeCache);
  assert.notEqual(pages[0].target.browserContextId,pages[1].target.browserContextId);
  receipt.bindings=pages.map(page=>({nativePid:page.nativePid,native:page.native,target:page.target,context:page.context}));
  const key=`sumi_process_state_${nonce}`,cookie=`sumi_process_${nonce.replaceAll('-','')}`,documentKey=`sumi_process_document_${nonce}`;
  const get=page=>page.evaluate(`localStorage.getItem(${JSON.stringify(key)})`);
  const cookies=async page=>(await page.send('Network.getCookies',{urls:[urls[app]]})).cookies.filter(item=>item.name===cookie);
  const put=async(page,value)=>{
    await page.evaluate(`localStorage.setItem(${JSON.stringify(key)},${JSON.stringify(value)})`);
    assert.equal((await page.send('Network.setCookie',{name:cookie,value,url:urls[app],secure:true,httpOnly:true,sameSite:'Lax'})).success,true);
  };
  const retained=async(page,value)=>{
    const snapshot=await page.evaluate(`({origin:location.origin,secure:isSecureContext,document:globalThis[${JSON.stringify(documentKey)}],value:localStorage.getItem(${JSON.stringify(key)})})`);
    assert.deepEqual(snapshot,{origin:new URL(urls[app]).origin,secure:true,document:page.native.profile+':'+nonce,value});
    assert.equal((await page.send('Page.getFrameTree')).frameTree.frame.id,page.native.frame.id);
    const actual=await cookies(page);
    assert.equal(actual.length,1);assert(actual[0].secure&&actual[0].httpOnly&&actual[0].value===value);
    assert.equal(await page.evaluate(`document.cookie.includes(${JSON.stringify(cookie)})`),false);
    return {snapshot,cookie:actual[0]};
  };
  await stage('same-origin state and document identity');
  receipt.initial=[await get(pages[0]),await get(pages[1])];assert.deepEqual(receipt.initial,[null,null]);
  for(const page of pages)assert.equal(await page.evaluate(`(()=>{if(globalThis[${JSON.stringify(documentKey)}])throw Error('Fresh document required');return globalThis[${JSON.stringify(documentKey)}]=${JSON.stringify(page.native.profile+':'+nonce)}})()`),page.native.profile+':'+nonce);
  await put(pages[0],`work:${nonce}`);
  assert.equal(await get(pages[1]),null);assert.deepEqual(await cookies(pages[1]),[]);
  await put(pages[1],`personal:${nonce}`);
  receipt.before=await Promise.all(pages.map((page,index)=>retained(page,`${index===0?'work':'personal'}:${nonce}`)));
  await stage('native permissions and same-tag OS actions');
  receipt.permissions=await inspectPermissions({pages,data,nonce,desktop,decide:true,
    // A process-local command is mandatory here: native window IDs may collide.
    command:()=>{throw Error('Unscoped browser command is forbidden across processes');},
    notificationProbe:({pages,windows})=>probeNotifications({pages,windows,data,nonce,desktop,backend})});
  assert(receipt.permissions.permissionPassed&&receipt.permissions.nativeNotifications.passed);
  receipt.after=await Promise.all(pages.map((page,index)=>retained(page,`${index===0?'work':'personal'}:${nonce}`)));
  await stage('close Work process and retain Personal');
  assert.equal((await pages[0].browserCommand('Target.closeTarget',{targetId:pages[0].target.targetId})).success,true);
  const closePath=path.join(runtimes[0].data,`native-closed-${pages[0].native.browserId}.json`);
  const closeDeadline=Date.now()+5000;let closed;
  while(Date.now()<closeDeadline&&!closed) {
    try{closed=JSON.parse(await readFile(closePath,'utf8'));}catch(failure){if(failure.code!=='ENOENT')throw failure;await delay(50);}
  }
  assert(closed,'Work native close callback required');
  assert.equal(closed.event.profile,PROFILE_IDS[0]);assert.equal(closed.binding.frame.id,pages[0].native.frame.id);
  receipt.workClose=closed;
  await runtimes[0].stop();
  assert(runtimes[1].guard.exitCode===null&&runtimes[1].guard.signalCode===null,'Personal process must remain live after Work closes');
  receipt.personalAfterWorkClose=await retained(pages[1],`personal:${nonce}`);
  receipt.functionalPassed=true;
}catch(failure) {
  error=failure;receipt.error=String(failure);
  try{await desktop('import',['-window','root',path.join(data,'native-failure.png')]);}catch{}
}finally {
  for(const runtime of runtimes)try{await runtime.stop();}catch(failure){receipt.cleanupErrors??=[];receipt.cleanupErrors.push(String(failure));error??=failure;}
  if(backend)try{await backend.stop();}catch(failure){receipt.backendCleanupError=String(failure);error??=failure;}
  receipt.backend=backend?.proof;
  receipt.cleanupConfirmed=runtimes.every(runtime=>runtime.proof.closed)&&backend?.proof.stopped&&!backend?.proof.forced;
  const passed=receipt.functionalPassed===true&&receipt.cleanupConfirmed===true&&!error;
  receipt.profilePassed=passed;receipt.permissionPassed=passed;receipt.notificationUiPassed=passed;
  await writeFile(path.join(data,'reference.json'),JSON.stringify(receipt,null,2)+'\n');
}
if(error){console.error(error);process.exitCode=1;}else console.log(`PASS process-per-profile native notification component: ${app} ${data}`);
