// Observe the actual bundled App's browser children. No replacement shell IPC.
import assert from 'node:assert/strict';
import { lstat, readFile, readdir, readlink, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const data=process.argv[2], nonce=path.basename(data), proof={passed:false,scope:'Disposable Linux actual App/browser process fixture; no real login or Push'};
const phase=process.env.TAKOSUMI_BROWSER_PROCESS_PHASE??'', suffix=phase?`-${phase}`:'';
// Reads only this case's synthetic cookie; site cookies are not inspected.
const cookieOf=key=>`(document.cookie.split('; ').find(c=>c.startsWith(${JSON.stringify(key+'=')}))?.slice(${key.length+1})??null)`;
assert(['','restart','reset','notify','files'].includes(phase),'Unknown browser fixture phase');
const origin=({excalidraw:'https://excalidraw.com',photopea:'https://www.photopea.com'})[process.env.TAKOSUMI_BROWSER_PROCESS_APP];
assert(origin,'Only an explicit anonymous app fixture is allowed');
const desktop=(program,args)=>execFileSync(program,args,{encoding:'utf8',timeout:5000}).trim();
// Name the active operation and its exact page, so a failure snapshot inspects
// that profile rather than whichever client happened to attach last.
const step=(name,page)=>{proof.stage={name,pid:page?.native.nativePid,profile:page?.process.profile};};
// The same guard process (start identity) and its kernel state, or null once reaped.
const guardState=async pid=>{try{const f=(await readFile(`/proc/${pid}/stat`,'utf8')).split(') ').at(-1).split(/\s+/);return{state:f[0],start:f[19]};}catch(e){if(e.code==='ENOENT')return null;throw e;}};
const json=async file=>JSON.parse(await readFile(path.join(data,file),'utf8'));
async function marker(file) {
  const end=Date.now()+30000;
  while(Date.now()<end) {
    // Markers are written non-atomically: an incomplete file is not yet ready.
    try{return await json(file);}catch(e){if(e.code!=='ENOENT'&&!(e instanceof SyntaxError))throw e;}
    await delay(50);
  }
  throw Error(`Missing App checkpoint: ${file}`);
}
async function settled(test,message) {
  const end=Date.now()+10000;
  while(Date.now()<end){if(await test())return;await delay(50);}
  throw Error(message);
}
class Client {
  constructor(socket){this.socket=socket;this.next=0;this.pending=new Map();this.events=[];socket.onmessage=e=>{
    const m=JSON.parse(String(e.data)), p=this.pending.get(m.id);
    if(p){clearTimeout(p.timer);this.pending.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);}
    else if(m.method){this.events.push(m);if(this.events.length>4096)this.events.shift();}
  };
    socket.onclose=event=>{
      (proof.cdpCloses??=[]).push({profile:this.binding?.process.profile,code:event.code,reason:event.reason.slice(0,256),pending:this.pending.size});
      this.rejectPending(Error(`Owned CDP socket closed (${event.code})`));
    };
    socket.onerror=()=>this.rejectPending(Error('Owned CDP socket error'));
  }
  rejectPending(error){for(const p of this.pending.values()){clearTimeout(p.timer);p.reject(error);}this.pending.clear();}
  command(method,params={},sessionId){const id=++this.next;return new Promise((resolve,reject)=>{
    if(this.socket.readyState!==WebSocket.OPEN){reject(Error('Owned CDP socket is not open'));return;}
    const timer=setTimeout(()=>{this.pending.delete(id);proof.commandTimeout={method,sessionId,events:diagnosticEvents(this.events)};reject(Error(`CDP timeout ${method}`));},5000);
    this.pending.set(id,{resolve,reject,timer});this.socket.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));
  });}
}
const startTime=async pid=>(await readFile(`/proc/${pid}/stat`,'utf8')).split(') ').at(-1).split(/\s+/)[19];
function diagnosticEvents(events) {
  return events.filter(e=>e.method.includes('detached')||e.method.includes('Crashed')||e.method.startsWith('Page.javascriptDialog')||e.method.startsWith('Page.frame')).slice(-128).map(e=>{
    const p=e.params??{},frame=p.frame??{};
    return {method:e.method,sessionId:e.sessionId,frameId:p.frameId??frame.id,parentId:frame.parentId,url:p.url??frame.url,
      type:p.type,result:p.result,reason:p.reason,disposition:p.disposition,messageLength:typeof p.message==='string'?p.message.length:undefined};
  });
}
// A finished guard's generation is gone by design: never snapshot it. Prefer
// the failed stage's exact page; otherwise use a still-live owned client.
// The guard creates its receipt when it starts and writes JSON only at its end.
async function guardEnded(receipt) {
  let text;
  try{text=await readFile(receipt,'utf8');}catch(e){if(e.code==='ENOENT')return false;throw e;}
  if(!text)return false;
  try{JSON.parse(text);return true;}catch(e){if(e instanceof SyntaxError)return false;throw e;}
}
// A guard's complete receipt that records a normal, unforced end.
async function normalReceipt(file,message) {
  let receipt;
  await settled(async()=>{
    if(!(await guardEnded(file)))return false;
    receipt=JSON.parse(await readFile(file,'utf8'));return true;
  },message);
  assert.equal(receipt.rootExit,0,message);assert.equal(receipt.complete,true);assert.equal(receipt.kernelNoChildren,true);
  assert.equal(receipt.forcedCleanup,0);assert.deepEqual(receipt.remaining,[]);assert.deepEqual(receipt.cleanupErrors,[]);assert.equal(receipt.error,null);
  return receipt;
}
async function failureClient() {
  const live=[];
  for(const c of clients) {
    if(!c.binding)continue;
    try{if(!(await guardEnded(c.binding.process.guardReceipt)))live.push(c);}
    catch(e){proof.snapshotTargetError=String(e);return undefined;}
  }
  proof.snapshotTarget={stage:proof.stage?.name,liveProfiles:live.map(c=>c.binding.process.profile)};
  return live.find(c=>c.binding.process.opened.nativePid===proof.stage?.pid)??live[0];
}
async function failureSnapshot(client) {
  const binding=client?.binding;
  if(!binding||client.snapshotAttempted)return;
  client.snapshotAttempted=true;
  try {
    proof.processSnapshot=JSON.parse(desktop('python3',['-B',fileURLToPath(new URL('./browser_snapshot.py',import.meta.url)),
      '--data',data,'--profile',binding.process.profile,'--native-start',binding.nativeStart,'--guard-start',binding.guardStart]));
  }catch(error){proof.snapshotError=String(error);}
  try {
    const owner=JSON.parse(desktop('python3',['-B',fileURLToPath(new URL('../cef-candidate/x11-owner.py',import.meta.url)),String(binding.process.opened.windowId)]));
    assert.equal(owner.pid,binding.process.opened.nativePid);
    desktop('import',['-window','root',path.join(data,'failure-desktop.png')]);
    proof.failureWindowOwner=owner;
  }catch(error){proof.screenshotError=String(error);}
  proof.failureEvents=diagnosticEvents(client.events);
}
async function attach(process) {
  // Set before any await, so an attach failure never inherits the previous page's stage.
  proof.stage={name:'attach',pid:process.opened.nativePid,profile:process.profile};
  const native=process.opened, inspection=JSON.parse(await readFile(path.join(process.directory,'inspection.json'),'utf8'));
  assert.equal(inspection.pid,native.nativePid);
  const rows=(await readFile('/proc/net/tcp','utf8')).split('\n').slice(1).map(line=>line.trim().split(/\s+/));
  const row=rows.find(r=>r[1]===`0100007F:${inspection.port.toString(16).toUpperCase().padStart(4,'0')}`&&r[3]==='0A');
  assert(row,'Only a loopback native inspection listener is allowed');
  const fds=await readdir(`/proc/${native.nativePid}/fd`);
  const links=await Promise.all(fds.map(fd=>readlink(`/proc/${native.nativePid}/fd/${fd}`).catch(()=>'')));
  assert(links.includes(`socket:[${row[9]}]`),'Attested native PID must own the endpoint');
  const info=await (await fetch(`http://127.0.0.1:${inspection.port}/json/version`,{signal:AbortSignal.timeout(5000)})).json();
  const endpoint=new URL(info.webSocketDebuggerUrl);assert.equal(endpoint.hostname,'127.0.0.1');assert.equal(endpoint.port,String(inspection.port));
  const socket=new WebSocket(endpoint);
  await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('CDP handshake timeout')),5000);socket.onopen=()=>{clearTimeout(timer);resolve();};socket.onerror=()=>{clearTimeout(timer);reject(Error('CDP handshake error'));};});
  const c=new Client(socket);clients.push(c);
  c.binding={process,nativeStart:await startTime(native.nativePid),guardStart:await startTime(process.pid)};
  const targets=(await c.command('Target.getTargets')).targetInfos.filter(t=>t.type==='page');
  assert.equal(targets.length,1,'Exactly one initial document per process');
  const target=targets[0], session=(await c.command('Target.attachToTarget',{targetId:target.targetId,flatten:true})).sessionId;
  const send=(method,params={})=>c.command(method,params,session);
  const evaluate=async(expression,userGesture=false)=>{
    const result=await send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true,userGesture});
    assert(!result.exceptionDetails,JSON.stringify(result.exceptionDetails));return result.result.value;
  };
  proof.stage={name:'external Page enable',pid:native.nativePid};
  await send('Page.enable');
  try {
    const content=origin.includes('excalidraw')?"Boolean(document.querySelector('.excalidraw'))":"document.body?.innerText.includes('Start using Photopea') || document.body?.innerText.includes('Open From Computer')";
    await settled(()=>evaluate(`location.origin===${JSON.stringify(origin)} && document.readyState==='complete' && (${content})`),'Actual anonymous app document did not load');
    if(origin.includes('photopea') && await evaluate("document.body.innerText.includes('Start using Photopea')")) {
      proof.stage={name:'Photopea landing button',pid:native.nativePid};
      // The site has more than one equivalent CTA. Use the first visible
      // enabled button, as the existing standalone driver does, and verify
      // the hit target rather than requiring a unique DOM label.
      const button=await evaluate("(()=>{const nodes=[...document.querySelectorAll('button')].filter(e=>{const r=e.getBoundingClientRect();return e.innerText.trim()==='Start using Photopea'&&!e.disabled&&r.width>0&&r.top>=0&&r.bottom<=innerHeight});if(!nodes.length)return null;const e=nodes[0],r=e.getBoundingClientRect(),point={x:r.x+r.width/2,y:r.y+r.height/2},hit=document.elementFromPoint(point.x,point.y);if(!hit||!(hit===e||e.contains(hit)))throw Error('Start button obscured');return{point,visibleMatches:nodes.length}})()");
      if(!button) {
        // Photopea dismisses its landing itself once it remembers an earlier start
        // (e.g. after an App restart). Entering the editor is still required.
        await settled(()=>evaluate("document.body?.innerText.includes('Open From Computer')"),'Visible Start button missing');
        (proof.landingAutoStarted??=[]).push({pid:native.nativePid});
      } else {
        (proof.landingButtons??=[]).push({pid:native.nativePid,...button});
        // Chrome treats a fully covered window as hidden and holds its input; an
        // owned stacked-window control timed out CDP input exactly as ph17 did.
        // Raise this exact window first, as a user clicking it would.
        const visibleBefore=await evaluate("document.visibilityState");
        desktop('xdotool',['windowraise',String(native.windowId)]);
        await settled(()=>evaluate("document.visibilityState==='visible'"),'Raised Photopea window did not become visible');
        (proof.landingRaise??=[]).push({pid:native.nativePid,window:native.windowId,visibleBefore});
        // Record what the page receives, to tell an input-routing drop from page behaviour (ph14/ph16).
        await evaluate("(()=>{window.__sumiInput=[];for(const t of ['pointermove','pointerdown','mousedown','pointerup','mouseup','click'])addEventListener(t,e=>{if(window.__sumiInput.length<64)window.__sumiInput.push({t,trusted:e.isTrusted,x:Math.round(e.clientX),y:Math.round(e.clientY),target:e.target?.tagName})},true);return true})()");
        // Move first, then press/release with button state, as a real pointer does.
        await send('Input.dispatchMouseEvent',{type:'mouseMoved',button:'none',buttons:0,...button.point});
        await send('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',buttons:1,clickCount:1,...button.point});
        await send('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',buttons:0,clickCount:1,...button.point});
        await settled(()=>evaluate("document.body?.innerText.includes('Open From Computer')"),'Photopea editor did not open').catch(async error=>{
          // Same bound; only split a start click that never ran addPP() from a slow editor load.
          const d=proof.landingDiagnosis=await evaluate("(()=>{const cap=document.getElementById('cap');return{capHidden:cap?getComputedStyle(cap).display==='none':null,added:typeof added==='boolean'?added:null,focus:document.hasFocus(),visibility:document.visibilityState,editorScripts:[...document.scripts].filter(s=>s.src.includes('/code/')).length,input:window.__sumiInput??null}})()").catch(e=>({error:String(e)}));
          throw d.capHidden===false&&d.added===false?Error('Photopea start click did not reach the page'):error;
        });
      }
    }
  } catch(error) {
    proof.startupFailure={native,error:String(error)};
    await failureSnapshot(c);
    if(!proof.commandTimeout)proof.startupFailure.document=await evaluate("({url:location.href,ready:document.readyState,title:document.title,canvas:document.querySelectorAll('canvas').length,body:document.body?.innerText.slice(0,500)})").catch(e=>({error:String(e)}));
    throw error;
  }
  proof.stage={name:'actual frame metadata',pid:native.nativePid};
  const frame=(await send('Page.getFrameTree')).frameTree.frame;
  const owner=JSON.parse(desktop('python3',['-B',fileURLToPath(new URL('../cef-candidate/x11-owner.py',import.meta.url)),String(native.windowId)]));
  assert.equal(owner.pid,native.nativePid);
  const page={process,native,target,frame,owner,c,send,evaluate};
  const context=await evaluate("({origin:location.origin,secure:isSecureContext,tauri:typeof __TAURI_INTERNALS__,ipc:typeof window.ipc,wry:typeof window.webkit?.messageHandlers?.ipc})");
  assert.equal(context.secure,true);for(const key of ['tauri','ipc','wry'])assert.equal(context[key],'undefined');
  page.context=context;return page;
}
// A later launch on the same App data. restart: each profile's storage and
// cookie from the first launch survive and stay separate. reset: Work was
// cleared from the home, so only Personal keeps its state, and a clear of the
// running Personal was refused.
async function relaunchFlow(kind) {
  proof.scope=`Disposable Linux actual App ${kind} on the same data; no real login or Push`;
  // notify: start the owned notification server before the App opens any
  // browser, as on a desktop session; CEF chooses its route at startup.
  if(kind==='notify')await startNotifier();
  const opened=await marker(`app-browser-process-${kind}-open.json`);proof.opened=opened;
  const pages=[];
  for(const name of ['Work','Personal']) {
    const connection=opened.connections.find(c=>c.name===name);assert(connection);
    const process=opened.processes.find(p=>p.profile===connection.profile);assert(process);
    pages.push(await attach(process));
  }
  const [,personal]=pages,key=`sumi_app_process_${nonce}`;
  step(`per-profile state after ${kind}`);
  if(kind==='notify')return notifyProbes(pages);
  if(kind==='files')return filesProbe(pages);
  proof.persisted=await Promise.all(pages.map(page=>page.evaluate(`({storage:localStorage.getItem(${JSON.stringify(key)}),cookie:${cookieOf(key)}})`)));
  const kept={storage:`1:${nonce}`,cookie:`1:${nonce}`};
  if(kind==='reset') {
    assert.deepEqual(proof.persisted,[{storage:null,cookie:null},kept],'Only Work site data may be cleared');
    assert.equal(opened.runningClearRefused,true,'Clearing a running profile must be refused');
    // The clear's liveness signal is CEF's own process singleton: it must name
    // the running Personal browser's exact native PID.
    assert.equal(String(opened.personalSingleton).split('-').at(-1),String(personal.native.nativePid),'CEF must mark the running Personal with its process singleton');
  } else {
    assert.deepEqual(proof.persisted,[{storage:`0:${nonce}`,cookie:`0:${nonce}`},kept],'Each profile must keep its own site storage and cookie across App restart');
  }
  await writeFile(path.join(data,`${kind}-inspected`),'ready',{flag:'wx'});
  await closePersonalAfterRelaunch(personal,kind);
  proof.passed=true;
}
async function closePersonalAfterRelaunch(personal,kind) {
  step(`Personal native close after ${kind}`,personal);
  desktop('xdotool',['windowraise',String(personal.native.windowId)]);
  await settled(()=>personal.evaluate("document.visibilityState==='visible'"),'Raised Personal window did not become visible');
  desktop('xdotool',['windowfocus','--sync',String(personal.native.windowId)]);
  desktop('xdotool',['key','--clearmodifiers','ctrl+shift+w']);
  await settled(async()=>{
    if(!(await guardEnded(personal.process.guardReceipt)))return false;
    const receipt=JSON.parse(await readFile(personal.process.guardReceipt,'utf8'));
    assert.equal(receipt.rootExit,0);assert.equal(receipt.complete,true);assert.equal(receipt.kernelNoChildren,true);
    assert.equal(receipt.forcedCleanup,0);assert.deepEqual(receipt.remaining,[]);assert.deepEqual(receipt.cleanupErrors,[]);assert.equal(receipt.error,null);
    proof.personalClosed=receipt;return true;
  },`Personal guard did not finish after ${kind}`);
  step(`Work ordinary exit after ${kind}`);
  await writeFile(path.join(data,`${kind}-request-exit`),'ready',{flag:'wx'});
}

// notify (runner --notifications): native Chrome permission UI and real OS
// notifications per profile, reusing the CEF candidate's inspected probes.
// Permissions are only ever chosen through the native prompt, never by CDP.
const desktopAsync=async(tool,args,allowMissing=false)=>{
  try{return desktop(tool,args);}catch(e){if(allowMissing&&e.status===1)return String(e.stdout??'').trim();throw e;}
};
let notifier=null;
async function startNotifier() {
  const binary=process.env.TAKOSUMI_BROWSER_PROCESS_NOTIFIER;
  assert(binary&&path.isAbsolute(binary),'The owned notification daemon is required');
  const {startNotificationBackend}=await import('../cef-candidate/notification-backend.mjs');
  notifier=await startNotificationBackend({binary,data,nonce,env:process.env,desktop:desktopAsync});
  await writeFile(path.join(data,'notify-backend-ready'),'ready',{flag:'wx'});
}
async function notifyProbes(pages) {
  const [,personal]=pages;
  await writeFile(path.join(data,'notify-inspected'),'ready',{flag:'wx'});
  const {probeNotifications}=await import('../cef-candidate/notifications.mjs');
  const {inspectPermissions}=await import('../cef-candidate/permissions.mjs');
  // The probes need the attested native PID and a process-local browser command.
  const probePages=pages.map(page=>({...page,nativePid:page.native.nativePid,browserCommand:(method,params)=>page.c.command(method,params)}));
  step('native permissions and OS notifications');
  const backend=notifier;
  try {
    proof.permissions=await inspectPermissions({pages:probePages,data,nonce,desktop:desktopAsync,decide:true,
      command:()=>{throw Error('Unscoped browser command is forbidden across processes');},
      notificationProbe:({pages,windows})=>probeNotifications({pages,windows,data,nonce,desktop:desktopAsync,backend})});
    assert(proof.permissions.permissionPassed&&proof.permissions.nativeNotifications.passed,'Native permission and notification probes must pass');
  } finally {
    await backend.stop();
  }
  await closePersonalAfterRelaunch(personal,'notify');
  proof.passed=true;
}
// files (runner --files, Excalidraw only): the actual OS file chooser through
// File System Access on the App path. Native Open of the owned input and Save
// As to a new path in Work's own Downloads, each dialog attributed to Work's
// native browser; the saved scene is verified once with the standalone oracle.
async function filesProbe(pages) {
  const [work,personal]=pages;
  assert(origin.includes('excalidraw'),'The native chooser probe is inspected for Excalidraw only');
  const input=process.env.TAKOSUMI_BROWSER_PROCESS_FILES_INPUT,filesNonce=process.env.TAKOSUMI_BROWSER_PROCESS_FILES_NONCE;
  assert(input&&path.isAbsolute(input)&&filesNonce,'The owned file input is required');
  const desktopIo=async(tool,args,allowMissing=false,raw=false)=>{
    try{const out=execFileSync(tool,args,{timeout:5000,maxBuffer:64*1024*1024});return raw?out:out.toString('utf8').trim();}
    catch(e){if(allowMissing&&e.status===1)return raw?Buffer.alloc(0):String(e.stdout??'').trim();throw e;}
  };
  const {nativeFiles,key,ready}=await import('./native-files.mjs');
  const files=nativeFiles({desktop:desktopIo,data});
  const owner=id=>JSON.parse(desktop('python3',['-B',fileURLToPath(new URL('../cef-candidate/x11-owner.py',import.meta.url)),String(id)]));
  const front=async()=>{
    // Personal opened last and covers Work; Chrome holds input to a covered window.
    desktop('xdotool',['windowraise',String(work.native.windowId)]);
    await settled(()=>work.evaluate("document.visibilityState==='visible'"),'Raised Work window did not become visible');
    desktop('xdotool',['windowfocus','--sync',String(work.native.windowId)]);
  };
  const personalDownloads=path.join(personal.process.directory,'Downloads'),listing=async()=>(await readdir(personalDownloads)).sort();
  proof.files={input,started:Date.now(),personalDownloadsBefore:await listing()};
  step('native Open in Work',work);
  await front();
  await ready(work.evaluate,"document.querySelector('.excalidraw canvas') !== null");
  // Excalidraw uses File System Access whenever the page has it; without it the
  // site falls back to <input type=file> and a download, a different route.
  proof.files.fileSystemAccess=await work.evaluate("typeof showOpenFilePicker==='function'&&typeof showSaveFilePicker==='function'");
  assert.equal(proof.files.fileSystemAccess,true,'The page must use File System Access, not the input/download fallback');
  const before=await files.visibleWindows();
  await key(work.send,'o',2);
  const open=await files.nativeDialog(before,'open');
  proof.files.openOwner=owner(open.id);
  assert.equal(proof.files.openOwner.pid,work.native.nativePid,"The Open dialog must belong to Work's native browser");
  await files.selectNativePath(open,input,'files-native-open');
  proof.files.open=open;
  await ready(work.evaluate,"[...document.querySelectorAll('.excalidraw canvas')].some(c=>{const context=c.getContext('2d');if(!context||!c.width||!c.height)return false;const p=context.getImageData(Math.floor(c.width/2),Math.floor(c.height/2),1,1).data;return p[0]>240&&p[1]<15&&p[2]<15&&p[3]>240})",12);
  proof.files.openedMs=Date.now()-proof.files.started;
  await writeFile(path.join(data,'files-inspected'),'ready',{flag:'wx'});
  step('native Save As in Work',work);
  await front();
  await key(work.send,'a',2);await key(work.send,'ArrowRight');
  const output=path.join(work.process.directory,'Downloads','SumiProbeSaved.excalidraw');
  await assert.rejects(lstat(output),{code:'ENOENT'});
  const beforeSave=await files.visibleWindows();
  await key(work.send,'s',2);
  const save=await files.nativeDialog(beforeSave,'save');
  proof.files.saveOwner=owner(save.id);
  assert.equal(proof.files.saveOwner.pid,work.native.nativePid,"The Save As dialog must belong to Work's native browser");
  await files.selectNativePath(save,output,'files-native-save');
  proof.files.save=save;
  await ready(work.evaluate,"document.body.innerText.includes('File saved.')",8);
  const saved=await lstat(output);
  assert(saved.isFile()&&!saved.isSymbolicLink()&&saved.size>0&&saved.size<=1024*1024,'Missing or oversized actual Save As output');
  proof.files.export=JSON.parse(desktop('python3',['-B','-c','import sys,json;from pathlib import Path;sys.path.insert(0,sys.argv[1]);from browser_apps import verify_export;print(json.dumps(verify_export(Path(sys.argv[2]),sys.argv[3],sys.argv[4])))',
    fileURLToPath(new URL('.',import.meta.url)),output,'excalidraw',filesNonce]));
  assert.deepEqual(await listing(),proof.files.personalDownloadsBefore,"Personal's Downloads must not change");
  proof.files.savedMs=Date.now()-proof.files.started;
  await closePersonalAfterRelaunch(personal,'files');
  proof.passed=true;
}
// A sign-in style popup: window.open from Work opens a native popup in Work's
// own browser process (same profile). It shares Work's storage and cookie,
// reports Work as its opener, and hands a result back by postMessage before
// closing itself; Personal sees neither. Synthetic state, not provider login.
async function popupProbe(work,personal,key,expected,sample) {
  // The app's own page, as the cef-candidate probe uses: a different document
  // could carry a different opener policy and sever the opener.
  const state={method:'Standard window.open/postMessage/window.close with synthetic state, not provider login'};
  const ref=`sumi_popup_${nonce.replaceAll('-','')}`, url=`${origin}/?${ref}`, result=`popup:${nonce}`;
  const pages=async page=>(await page.c.command('Target.getTargets')).targetInfos.filter(t=>t.type==='page');
  assert.deepEqual((await pages(work)).map(t=>t.targetId),[work.target.targetId],'Work must have one document before its popup');
  state.accepted=await work.evaluate(`(()=>{window.__sumiMessages=[];window.__sumiOnMessage=e=>{if(e.data?.sumi===${JSON.stringify(nonce)})window.__sumiMessages.push({origin:e.origin,fromPopup:e.source===globalThis[${JSON.stringify(ref)}],data:e.data})};addEventListener('message',window.__sumiOnMessage);globalThis[${JSON.stringify(ref)}]=window.open(${JSON.stringify(url)},${JSON.stringify(ref)},'popup,width=600,height=500');return Boolean(globalThis[${JSON.stringify(ref)}])})()`,true);
  assert.equal(state.accepted,true,'Popup request must be accepted');
  let target;
  await settled(async()=>{
    const found=(await pages(work)).filter(t=>t.targetId!==work.target.targetId);
    assert(found.length<=1,'Unexpected popup family');
    target=found.find(t=>t.openerId===work.target.targetId);return Boolean(target);
  },'Popup did not appear in Work\'s own browser process');
  state.target={targetId:target.targetId,openerId:target.openerId,url:target.url};
  assert.deepEqual((await pages(personal)).map(t=>t.targetId),[personal.target.targetId],'The popup must not appear in Personal');
  const session=(await work.c.command('Target.attachToTarget',{targetId:target.targetId,flatten:true})).sessionId;
  const evaluate=async(expression,userGesture=false)=>{
    const value=await work.c.command('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true,userGesture},session);
    assert(!value.exceptionDetails,JSON.stringify(value.exceptionDetails));return value.result.value;
  };
  // Same 25 s load bound as the cef-candidate popup probe.
  const end=Date.now()+25000;let loaded=false;
  while(Date.now()<end&&!loaded) {
    loaded=await evaluate(`location.origin===${JSON.stringify(origin)}&&document.readyState==='complete'`).catch(()=>false);
    if(!loaded)await delay(100);
  }
  assert(loaded,'Popup document must load');
  state.context=await evaluate("({origin:location.origin,secure:isSecureContext,opener:window.opener!==null,openerOrigin:window.opener?.location.origin,tauri:typeof __TAURI_INTERNALS__,ipc:typeof window.ipc,wry:typeof window.webkit?.messageHandlers?.ipc})");
  assert.equal(state.context.origin,origin);assert.equal(state.context.secure,true);
  assert.equal(state.context.opener,true);assert.equal(state.context.openerOrigin,origin);
  for(const field of ['tauri','ipc','wry'])assert.equal(state.context[field],'undefined');
  state.sharedBefore=await evaluate(`({storage:localStorage.getItem(${JSON.stringify(key)}),cookie:${cookieOf(key)}})`);
  assert.deepEqual(state.sharedBefore,{storage:expected[0].storage,cookie:expected[0].cookie},'The popup must share Work\'s storage and cookie');
  await evaluate(`(()=>{localStorage.setItem(${JSON.stringify(key)},${JSON.stringify(result)});document.cookie=${JSON.stringify(`${key}=${result}; Max-Age=86400; Path=/; Secure; SameSite=Lax`)};window.opener.postMessage({sumi:${JSON.stringify(nonce)},result:${JSON.stringify(result)}},${JSON.stringify(origin)});setTimeout(()=>window.close(),0);return true})()`,true);
  await settled(async()=>await work.evaluate(`globalThis[${JSON.stringify(ref)}].closed`)&&!(await pages(work)).some(t=>t.targetId===target.targetId),'Popup did not close back to Work');
  state.messages=await work.evaluate('window.__sumiMessages');
  assert.deepEqual(state.messages,[{origin,fromPopup:true,data:{sumi:nonce,result}}],'Work must receive exactly its popup\'s result');
  state.workAfter=await work.evaluate(`({storage:localStorage.getItem(${JSON.stringify(key)}),cookie:${cookieOf(key)}})`);
  assert.deepEqual(state.workAfter,{storage:result,cookie:result},'Work must see the state its popup wrote');
  assert.deepEqual(await sample(personal),expected[1],'Personal must see neither the popup nor its state');
  await settled(()=>work.evaluate("document.visibilityState==='visible'"),'Work must be visible after its popup closed');
  // Restore Work's first-launch state, which the later phases check.
  await work.evaluate(`(()=>{localStorage.setItem(${JSON.stringify(key)},${JSON.stringify(expected[0].storage)});document.cookie=${JSON.stringify(`${key}=${expected[0].cookie}; Max-Age=86400; Path=/; Secure; SameSite=Lax`)};removeEventListener('message',window.__sumiOnMessage);delete window.__sumiOnMessage;delete window.__sumiMessages;delete globalThis[${JSON.stringify(ref)}];return true})()`);
  assert.deepEqual(await sample(work),expected[0],'Work must keep its document after the popup');
  state.passed=true;return state;
}
// A popup left open when the App quits closes with its profile: the native
// close chain covers it, and the runner then requires a normal guard and App exit.
async function openPopupForQuit(page) {
  const ref=`sumi_quit_popup_${nonce.replaceAll('-','')}`, url=`${origin}/?${ref}`;
  assert.equal(await page.evaluate(`(()=>{globalThis[${JSON.stringify(ref)}]=window.open(${JSON.stringify(url)},${JSON.stringify(ref)},'popup,width=500,height=400');return Boolean(globalThis[${JSON.stringify(ref)}])})()`,true),true,'Popup request must be accepted');
  let target;
  await settled(async()=>{target=(await page.c.command('Target.getTargets')).targetInfos.find(t=>t.type==='page'&&t.openerId===page.target.targetId);return Boolean(target);},'Popup did not appear before the final quit');
  return {targetId:target.targetId,openerId:target.openerId,profile:page.process.profile};
}
async function firstLaunch() {
  const opened=await marker('app-browser-process-open.json');proof.opened=opened;
  // A guard forked on a retiring blocking-pool thread ended ~10 s after the
  // last App command (ph13). Before any page step, send no App command past
  // that keep-alive and require every native guard to be live; attach below
  // then re-attests each native PID, window and document.
  step('idle past blocking keep-alive');
  const openSeen=Date.now();
  // The same guard process (start identity), not a zombie, and no final receipt.
  const guardStarts=await Promise.all(opened.processes.map(process=>guardState(process.pid)));
  await delay(12000);
  for(const [index,process] of opened.processes.entries()) {
    const now=await guardState(process.pid),before=guardStarts[index];
    assert(before&&now&&now.start===before.start&&!['Z','X'].includes(now.state),`Browser guard for ${process.profile} was not the same live process after the idle hold`);
    assert(!(await guardEnded(process.guardReceipt)),`Browser guard for ${process.profile} ended during the idle hold`);
  }
  proof.idleHold={milliseconds:Date.now()-openSeen,guardsLive:opened.processes.length};
  const pages=[];
  for(const name of ['Work','Personal']) {
    const connection=opened.connections.find(c=>c.name===name);assert(connection);
    const process=opened.processes.find(p=>p.profile===connection.profile);assert(process);
    const page=await attach(process);pages.push(page);
  }
  const [work,personal]=pages,key=`sumi_app_process_${nonce}`;
  const sample=page=>page.evaluate(`({document:globalThis[${JSON.stringify(key)}],storage:localStorage.getItem(${JSON.stringify(key)}),cookie:${cookieOf(key)},origin:location.origin,frame:window===top})`);
  proof.bindings=pages.map(p=>({native:p.native,target:p.target,frame:p.frame,owner:p.owner,context:p.context}));
  step('separate initial state');
  for(const [index,page] of pages.entries()) {
    assert.equal(await page.evaluate(`localStorage.getItem(${JSON.stringify(key)})`),null);
    assert.equal(await page.evaluate(cookieOf(key)),null);
    await page.evaluate(`(()=>{globalThis[${JSON.stringify(key)}]=${JSON.stringify(`${index}:${nonce}`)};localStorage.setItem(${JSON.stringify(key)},globalThis[${JSON.stringify(key)}]);document.cookie=${JSON.stringify(`${key}=${index}:${nonce}; Max-Age=86400; Path=/; Secure; SameSite=Lax`)};return true})()`);
  }
  const expected=proof.initial=await Promise.all(pages.map(sample));
  assert.notEqual(expected[0].storage,expected[1].storage);
  // Login state lives in cookies: each profile sees only its own value.
  assert.deepEqual(expected.map(e=>e.cookie),[`0:${nonce}`,`1:${nonce}`],'Each profile must see only its own cookie');
  // Precondition for the return check below: raise Personal over Work (any
  // app, any creation order) and require Chrome to report Work hidden.
  // Without that, the check is vacuous.
  step('Work covered before resume',work);
  desktop('xdotool',['windowraise',String(personal.native.windowId)]);
  await settled(()=>work.evaluate("document.visibilityState==='hidden'"),'Precondition: Personal must cover Work before resume');
  proof.inspectReadyAfterOpenMs=Date.now()-openSeen;
  await writeFile(path.join(data,'inspect-ready'),'ready',{flag:'wx'});
  step('common UI resume');
  proof.resumed=await marker('app-browser-process.json');
  assert.deepEqual(await Promise.all(pages.map(sample)),expected,'UI resume must retain both exact documents and storage');
  // Returning to Work from the common home must bring its window to the front.
  step('Work brought to front by resume',work);
  await settled(()=>work.evaluate("document.visibilityState==='visible'"),'Resumed Work window stayed hidden behind another profile');
  proof.resumeFront={work:'visible',precondition:'hidden'};
  // A site file export lands only in that profile's own Downloads directory.
  step('Work Blob download',work);
  const fileName=`sumi-${nonce}.txt`,fileText=`work:${nonce}`;
  const ownDownloads=path.join(work.process.directory,'Downloads'),otherDownloads=path.join(personal.process.directory,'Downloads');
  await work.evaluate(`(()=>{const a=document.createElement('a');a.href=URL.createObjectURL(new Blob([${JSON.stringify(fileText)}],{type:'text/plain'}));a.download=${JSON.stringify(fileName)};document.body.append(a);a.click();a.remove();return true})()`,true);
  await settled(async()=>{try{return (await readFile(path.join(ownDownloads,fileName),'utf8'))===fileText;}catch(e){if(e.code==='ENOENT')return false;throw e;}},'Work download did not land in its own Downloads');
  assert.deepEqual((await readdir(otherDownloads)).filter(name=>name.includes(nonce)),[],'Work download must not reach Personal Downloads');
  proof.download={profile:work.process.profile,file:fileName,bytes:Buffer.byteLength(fileText),otherProfileClean:true};
  step('Work same-origin popup',work);
  proof.popup=await popupProbe(work,personal,key,expected,sample);
  // Chrome's real beforeunload path needs sticky user activation. CDP's
  // userGesture is synthetic and is reported as such, not a real IdP click.
  const unloadStart=work.c.events.length;
  step('Work beforeunload setup',work);
  await work.evaluate("(()=>{window.__sumiBeforeUnload=e=>{e.preventDefault();e.returnValue='';};addEventListener('beforeunload',window.__sumiBeforeUnload);return true})()",true);
  // Sequential App quit (owner's decision): profiles close oldest first, so
  // Work closes first; cancelling its native beforeunload stops the quit and
  // Personal is never asked to close.
  const personalStart=personal.c.events.length;
  await writeFile(path.join(data,'request-exit'),'ready',{flag:'wx'});
  step('Work native beforeunload on sequential quit',work);
  await settled(()=>Promise.resolve(work.c.events.slice(unloadStart).some(e=>e.method==='Page.javascriptDialogOpening'&&e.params.type==='beforeunload')),'Native beforeunload dialog did not open');
  proof.dialog=work.c.events.slice(unloadStart).find(e=>e.method==='Page.javascriptDialogOpening');
  assert.equal(new URL(proof.dialog.params.url).origin,origin);
  assert(!personal.c.events.slice(personalStart).some(e=>e.method.startsWith('Page.javascriptDialog')),'Personal must not be asked to close before Work agrees');
  assert.equal(await guardEnded(personal.process.guardReceipt),false,'Personal must still be running during Work\'s dialog');
  desktop('import',['-window','root',path.join(data,'beforeunload.png')]);
  desktop('xdotool',['windowfocus','--sync',String(work.native.windowId)]);
  desktop('xdotool',['key','--clearmodifiers','Escape']);
  await settled(()=>Promise.resolve(work.c.events.slice(unloadStart).some(e=>e.method==='Page.javascriptDialogClosed'&&e.params.result===false)),'Native Escape did not cancel beforeunload');
  assert.deepEqual(await Promise.all(pages.map(sample)),expected,'A cancelled sequential quit must keep every document');
  step('sequential quit stopped at the cancel',work);
  await writeFile(path.join(data,'cancel-inspected'),'ready',{flag:'wx'});
  proof.blocked=await marker('app-browser-process-blocked.json');
  assert.equal(proof.blocked.length,2,'Both profiles must remain after the cancelled quit');
  proof.cancelled=await marker('app-browser-process-cancelled.json');
  assert.deepEqual(await Promise.all(pages.map(sample)),expected);
  // The native Page observer is now enabled by the first real close attempt.
  // Exercise its handling of remote-controlled long dialog text while active.
  // A page may replace window.alert (Photopea binds an in-page toast), which
  // never reaches a native dialog. Use a pristine same-origin realm's native
  // alert on this exact top window and record the page's own binding name.
  step('Work long native alert',work);
  const previousClosed=work.c.events.filter(e=>e.method==='Page.javascriptDialogClosed').length;
  proof.pageAlertName=await work.evaluate("String(window.alert?.name)");
  await work.evaluate("(()=>{const f=document.createElement('iframe');f.hidden=true;document.documentElement.append(f);const native=f.contentWindow.alert;if(typeof native!=='function'||native.name!=='alert')throw Error('Pristine native alert unavailable');window.__sumiAlertFrame=f;setTimeout(()=>{try{native.call(window,'x'.repeat(9001));}catch(e){window.__sumiAlertError=String(e);}},0);return true})()");
  // The dialog must belong to Work's exact top frame and origin, as the old top-document alert did.
  const longAlert=e=>e.method==='Page.javascriptDialogOpening'&&e.params.type==='alert'&&e.params.message.length===9001&&
    e.params.frameId===work.frame.id&&URL.canParse(e.params.url)&&new URL(e.params.url).origin===origin;
  await settled(()=>Promise.resolve(work.c.events.some(longAlert)),'Long native alert did not open').catch(async error=>{
    // An open dialog blocks evaluation; record observed alerts and read the page error only when none opened.
    proof.longAlertDialogs=work.c.events.filter(e=>e.method==='Page.javascriptDialogOpening'&&e.params.type==='alert')
      .map(e=>({length:e.params.message.length,frameId:e.params.frameId,url:e.params.url,topFrame:e.params.frameId===work.frame.id}));
    if(!proof.longAlertDialogs.length)proof.longAlertError=await work.evaluate("window.__sumiAlertError??null").catch(e=>String(e));
    throw error;
  });
  const alertEvent=work.c.events.find(longAlert);
  desktop('xdotool',['windowfocus','--sync',String(work.native.windowId)]);
  desktop('xdotool',['key','--clearmodifiers','Escape']);
  await settled(()=>Promise.resolve(work.c.events.filter(e=>e.method==='Page.javascriptDialogClosed').length>previousClosed),'Long native alert did not close');
  assert.deepEqual(await sample(work),expected[0],'Long alert must not kill or replace the browser document');
  proof.longAlert={messageLength:9001,closed:true,documentRetained:true,source:'pristine same-origin realm',pageAlertName:proof.pageAlertName,
    frameId:alertEvent.params.frameId,topFrame:true,url:alertEvent.params.url};
  await work.evaluate("(()=>{window.__sumiAlertFrame.remove();delete window.__sumiAlertFrame;removeEventListener('beforeunload',window.__sumiBeforeUnload);delete window.__sumiBeforeUnload;return true})()");
  // The accepted sequential trade-off: Work (oldest) now agrees to leave and
  // closes normally first; Personal cancels, so the quit stops with Work closed
  // and Personal kept.
  step('Personal cancels after Work closed',personal);
  const personalUnload=personal.c.events.length;
  await personal.evaluate("(()=>{window.__sumiBeforeUnload=e=>{e.preventDefault();e.returnValue='';};addEventListener('beforeunload',window.__sumiBeforeUnload);return true})()",true);
  await writeFile(path.join(data,'request-partial-exit'),'ready',{flag:'wx'});
  await settled(()=>Promise.resolve(personal.c.events.slice(personalUnload).some(e=>e.method==='Page.javascriptDialogOpening'&&e.params.type==='beforeunload')),'Personal beforeunload did not open after Work closed');
  // The App drops Work only after its normal receipt, before asking Personal:
  // check once, at the moment Personal's dialog is open, without polling.
  assert.equal(await guardEnded(work.process.guardReceipt),true,'Work must have closed before Personal is asked');
  proof.workClosedFirst=await normalReceipt(work.process.guardReceipt,'Work must have closed normally');
  desktop('xdotool',['windowfocus','--sync',String(personal.native.windowId)]);
  desktop('xdotool',['key','--clearmodifiers','Escape']);
  await settled(()=>Promise.resolve(personal.c.events.slice(personalUnload).some(e=>e.method==='Page.javascriptDialogClosed'&&e.params.result===false)),'Native Escape did not cancel Personal beforeunload');
  assert.deepEqual(await sample(personal),expected[1],'The cancelled Personal must keep its document');
  await writeFile(path.join(data,'partial-inspected'),'ready',{flag:'wx'});
  proof.partial=await marker('app-browser-process-partial.json');
  assert.deepEqual(proof.partial.map(process=>process.profile),[personal.process.profile],'Only Personal remains after Work closed and Personal cancelled');
  await personal.evaluate("(()=>{removeEventListener('beforeunload',window.__sumiBeforeUnload);delete window.__sumiBeforeUnload;return true})()");
  step('Personal popup open at the final quit',personal);
  proof.finalPopup=await openPopupForQuit(personal);
  step('Personal final exit',personal);
  await writeFile(path.join(data,'request-final-exit'),'ready',{flag:'wx'});
  proof.passed=true;
}
const clients=[];
try {
  if(phase)await relaunchFlow(phase);
  else await firstLaunch();
}catch(error){
  proof.error=String(error);
  if(!proof.processSnapshot&&clients.length) {
    const target=await failureClient();
    if(target)await failureSnapshot(target);
    else {
      // No live process to inspect, e.g. the stage page crashed: still keep its in-memory events.
      const stage=clients.find(c=>c.binding?.process.opened.nativePid===proof.stage?.pid);
      if(stage)proof.failureEvents=diagnosticEvents(stage.events);
    }
  }
  throw error;
}
finally {
  for(const c of clients)c.socket.close();
  if(notifier&&!notifier.proof.stopped)await notifier.stop().catch(e=>{proof.notifierStopError=String(e);});
  await writeFile(path.join(data,`browser-observation${suffix}.json`),JSON.stringify(proof,null,2)+'\n');
}
