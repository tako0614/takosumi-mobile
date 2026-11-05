// Observe the actual bundled App's browser children. No replacement shell IPC.
import assert from 'node:assert/strict';
import { readFile, readdir, readlink, writeFile } from 'node:fs/promises';
import { execFileSync } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const data=process.argv[2], nonce=path.basename(data), proof={passed:false,scope:'Disposable Linux actual App/browser process fixture; no real login or Push'};
const origin=({excalidraw:'https://excalidraw.com',photopea:'https://www.photopea.com'})[process.env.TAKOSUMI_BROWSER_PROCESS_APP];
assert(origin,'Only an explicit anonymous app fixture is allowed');
const desktop=(program,args)=>execFileSync(program,args,{encoding:'utf8',timeout:5000}).trim();
// Name the active operation and its exact page, so a failure snapshot inspects
// that profile rather than whichever client happened to attach last.
const step=(name,page)=>{proof.stage={name,pid:page?.native.nativePid,profile:page?.process.profile};};
const json=async file=>JSON.parse(await readFile(path.join(data,file),'utf8'));
async function marker(file) {
  const end=Date.now()+30000;
  while(Date.now()<end) {
    try{return await json(file);}catch(e){if(e.code!=='ENOENT')throw e;}
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
async function failureClient() {
  const live=[];
  for(const c of clients) {
    if(!c.binding)continue;
    try{await readFile(c.binding.process.guardReceipt);}
    catch(e){if(e.code==='ENOENT'){live.push(c);continue;}proof.snapshotTargetError=String(e);return undefined;}
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
      const button=await evaluate("(()=>{const nodes=[...document.querySelectorAll('button')].filter(e=>{const r=e.getBoundingClientRect();return e.innerText.trim()==='Start using Photopea'&&!e.disabled&&r.width>0&&r.top>=0&&r.bottom<=innerHeight});if(!nodes.length)throw Error('Visible Start button missing');const e=nodes[0],r=e.getBoundingClientRect(),point={x:r.x+r.width/2,y:r.y+r.height/2},hit=document.elementFromPoint(point.x,point.y);if(!hit||!(hit===e||e.contains(hit)))throw Error('Start button obscured');return{point,visibleMatches:nodes.length}})()");
      (proof.landingButtons??=[]).push({pid:native.nativePid,...button});
      await send('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...button.point});
      await send('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...button.point});
      await settled(()=>evaluate("document.body?.innerText.includes('Open From Computer')"),'Photopea editor did not open');
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
const clients=[];
try {
  const opened=await marker('app-browser-process-open.json');proof.opened=opened;
  const pages=[];
  for(const name of ['Work','Personal']) {
    const connection=opened.connections.find(c=>c.name===name);assert(connection);
    const process=opened.processes.find(p=>p.profile===connection.profile);assert(process);
    const page=await attach(process);pages.push(page);
  }
  const [work,personal]=pages,key=`sumi_app_process_${nonce}`;
  const sample=page=>page.evaluate(`({document:globalThis[${JSON.stringify(key)}],storage:localStorage.getItem(${JSON.stringify(key)}),origin:location.origin,frame:window===top})`);
  proof.bindings=pages.map(p=>({native:p.native,target:p.target,frame:p.frame,owner:p.owner,context:p.context}));
  step('separate initial state');
  for(const [index,page] of pages.entries()) {
    assert.equal(await page.evaluate(`localStorage.getItem(${JSON.stringify(key)})`),null);
    await page.evaluate(`(()=>{globalThis[${JSON.stringify(key)}]=${JSON.stringify(`${index}:${nonce}`)};localStorage.setItem(${JSON.stringify(key)},globalThis[${JSON.stringify(key)}]);return true})()`);
  }
  const expected=proof.initial=await Promise.all(pages.map(sample));
  assert.notEqual(expected[0].storage,expected[1].storage);
  await writeFile(path.join(data,'inspect-ready'),'ready',{flag:'wx'});
  step('common UI resume');
  proof.resumed=await marker('app-browser-process.json');
  assert.deepEqual(await Promise.all(pages.map(sample)),expected,'UI resume must retain both exact documents and storage');
  // Chrome's real beforeunload path needs sticky user activation. CDP's
  // userGesture is synthetic and is reported as such, not a real IdP click.
  const unloadStart=work.c.events.length;
  step('Work beforeunload setup',work);
  await work.evaluate("(()=>{window.__sumiBeforeUnload=e=>{e.preventDefault();e.returnValue='';};addEventListener('beforeunload',window.__sumiBeforeUnload);return true})()",true);
  await writeFile(path.join(data,'request-exit'),'ready',{flag:'wx'});
  step('multi-profile quit refusal');
  proof.blocked=await marker('app-browser-process-blocked.json');
  assert.equal(proof.blocked.length,2);assert.deepEqual(await Promise.all(pages.map(sample)),expected,'Refused multi-profile quit must preserve every document');
  assert(!work.c.events.slice(unloadStart).some(e=>e.method==='Page.javascriptDialogOpening'),'Multi-profile refusal must occur before any close/dialog');
  // Close Personal using the existing Chrome window shortcut, then require
  // native guard completion before asking App to close the sole Work window.
  step('Personal native close',personal);
  desktop('xdotool',['windowfocus','--sync',String(personal.native.windowId)]);
  desktop('xdotool',['key','--clearmodifiers','ctrl+shift+w']);
  // Require the exact parent-attested guard receipt, not target IDs from a
  // different process or a process-name search.
  await settled(async()=>{
    let receipt;try {receipt=JSON.parse(await readFile(personal.process.guardReceipt,'utf8'));}
    catch(e){if(e.code==='ENOENT'||e instanceof SyntaxError)return false;throw e;}
    assert.equal(receipt.rootExit,0);assert.equal(receipt.complete,true);assert.equal(receipt.kernelNoChildren,true);
    assert.equal(receipt.forcedCleanup,0);assert.deepEqual(receipt.remaining,[]);assert.deepEqual(receipt.cleanupErrors,[]);assert.equal(receipt.error,null);
    proof.personalClosed=receipt;return true;
  },'Personal guard did not finish');
  step('Work native beforeunload',work);
  await writeFile(path.join(data,'request-single-exit'),'ready',{flag:'wx'});
  await settled(()=>Promise.resolve(work.c.events.slice(unloadStart).some(e=>e.method==='Page.javascriptDialogOpening'&&e.params.type==='beforeunload')),'Native beforeunload dialog did not open');
  proof.dialog=work.c.events.slice(unloadStart).find(e=>e.method==='Page.javascriptDialogOpening');
  assert.equal(new URL(proof.dialog.params.url).origin,origin);
  desktop('import',['-window','root',path.join(data,'beforeunload.png')]);
  desktop('xdotool',['windowfocus','--sync',String(work.native.windowId)]);
  desktop('xdotool',['key','--clearmodifiers','Escape']);
  await settled(()=>Promise.resolve(work.c.events.slice(unloadStart).some(e=>e.method==='Page.javascriptDialogClosed'&&e.params.result===false)),'Native Escape did not cancel beforeunload');
  assert.deepEqual(await sample(work),expected[0],'Cancelled native close must retain the actual Work document');
  step('Work close cancelled',work);
  await writeFile(path.join(data,'cancel-inspected'),'ready',{flag:'wx'});
  proof.cancelled=await marker('app-browser-process-cancelled.json');
  assert.deepEqual(await sample(work),expected[0]);
  // The native Page observer is now enabled by the first real close attempt.
  // Exercise its handling of remote-controlled long dialog text while active.
  // A page may replace window.alert (Photopea binds an in-page toast), which
  // never reaches a native dialog. Use a pristine same-origin realm's native
  // alert on this exact top window and record the page's own binding name.
  step('Work long native alert',work);
  const previousClosed=work.c.events.filter(e=>e.method==='Page.javascriptDialogClosed').length;
  proof.pageAlertName=await work.evaluate("String(window.alert?.name)");
  await work.evaluate("(()=>{const f=document.createElement('iframe');f.hidden=true;document.documentElement.append(f);const native=f.contentWindow.alert;if(typeof native!=='function'||native.name!=='alert')throw Error('Pristine native alert unavailable');window.__sumiAlertFrame=f;setTimeout(()=>{try{native.call(window,'x'.repeat(9001));}catch(e){window.__sumiAlertError=String(e);}},0);return true})()");
  await settled(()=>Promise.resolve(work.c.events.some(e=>e.method==='Page.javascriptDialogOpening'&&e.params.type==='alert'&&e.params.message.length===9001)),'Long native alert did not open')
    .catch(async error=>{proof.longAlertError=await work.evaluate("window.__sumiAlertError??null").catch(e=>String(e));throw error;});
  desktop('xdotool',['windowfocus','--sync',String(work.native.windowId)]);
  desktop('xdotool',['key','--clearmodifiers','Escape']);
  await settled(()=>Promise.resolve(work.c.events.filter(e=>e.method==='Page.javascriptDialogClosed').length>previousClosed),'Long native alert did not close');
  assert.deepEqual(await sample(work),expected[0],'Long alert must not kill or replace the browser document');
  proof.longAlert={messageLength:9001,closed:true,documentRetained:true,source:'pristine same-origin realm',pageAlertName:proof.pageAlertName};
  await work.evaluate("(()=>{window.__sumiAlertFrame.remove();delete window.__sumiAlertFrame;removeEventListener('beforeunload',window.__sumiBeforeUnload);delete window.__sumiBeforeUnload;return true})()");
  step('Work final exit',work);
  await writeFile(path.join(data,'request-final-exit'),'ready',{flag:'wx'});
  proof.passed=true;
}catch(error){proof.error=String(error);if(!proof.processSnapshot&&clients.length)await failureSnapshot(await failureClient());throw error;}
finally {
  for(const c of clients)c.socket.close();
  await writeFile(path.join(data,'browser-observation.json'),JSON.stringify(proof,null,2)+'\n');
}
