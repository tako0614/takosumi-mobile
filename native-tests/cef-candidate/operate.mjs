// Anonymous standalone CEF candidate. No Web API replacement or real login.
import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir, open, lstat, readdir, readlink } from 'node:fs/promises';
import { createServer } from 'node:net';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import path from 'node:path';
import { waitForGtkConfirm } from './gtk-readiness.mjs';
import { PROFILE_IDS, probeProfiles } from './profiles.mjs';

const repo=fileURLToPath(new URL('../../',import.meta.url));
const data=process.argv[2], app=process.argv[3], mode=process.argv[4], binary=process.argv[5];
const files=process.argv[6]??'cdp';
assert(['cdp','native-picker'].includes(files));
assert(files!=='native-picker'||app==='excalidraw');
assert(binary && path.isAbsolute(binary));
assert((await lstat(binary)).isFile() && !(await lstat(binary)).isSymbolicLink());
const urls={photopea:'https://www.photopea.com/',excalidraw:'https://excalidraw.com/'};
assert(data && path.isAbsolute(data) && urls[app] && ['inspect','operate','profiles','profile-popup'].includes(mode));
const profileMode=mode==='profiles'||mode==='profile-popup';
assert(!profileMode||files==='cdp');
assert(process.env.TAKOSUMI_CEF_RUNNER_CASE===data,'Use run.py to own the desktop session');
assert((await lstat(data)).isDirectory() && !(await lstat(data)).isSymbolicLink());
const nonce=(await readFile(path.join(data,'.owned-browser-reference'),'utf8')).trim();
assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(nonce));
const input=path.join(data,app==='photopea'?'SumiProbeWork.svg':'SumiProbeWork.excalidraw');
await readFile(input);
const {confirmBrowserCleanup}=await import(pathToFileURL(path.join(repo,'native-tests/browser/lifecycle.ts')));
const env={...Object.fromEntries(Object.entries(process.env).filter(([key])=>['PATH','HOME','USER','LOGNAME','LANG','LC_ALL','LC_CTYPE','DISPLAY','XAUTHORITY','DBUS_SESSION_BUS_ADDRESS'].includes(key))),PYTHONDONTWRITEBYTECODE:'1',TAKOSUMI_CEF_CASE:data,LD_LIBRARY_PATH:path.dirname(binary)};
for(const [key,dir] of Object.entries({XDG_DATA_HOME:'data',XDG_CONFIG_HOME:'config',XDG_CACHE_HOME:'cache',XDG_RUNTIME_DIR:'runtime',TMPDIR:'tmp'})) {
  env[key]=path.join(data,dir); await mkdir(env[key],{mode:0o700});
}
await mkdir(path.join(data,'Downloads'),{mode:0o700});
const log=await open(path.join(data,'browser.log'),'wx',0o600);
let browser,socket,exitPromise,sessionId,closed=false,error;
const pending=new Map(), events=[], downloads=[];
const navigation=[], documentRequests=new Set();
let next=0;
const receipt={engine:'standalone CEF154 candidate',app,mode,files,nonce,operationPassed:false,events:downloads,navigation};
async function desktop(tool,args,allowMissing=false,raw=false) {
  const child=spawn(tool,args,{env,stdio:['ignore','pipe','pipe']});
  const chunks=[];let err='';child.stdout.on('data',d=>chunks.push(d));child.stderr.on('data',d=>err+=d);
  const timer=setTimeout(()=>child.kill('SIGKILL'),4000);
  const status=await new Promise((resolve,reject)=>{child.once('exit',resolve);child.once('error',reject)}).finally(()=>clearTimeout(timer));
  assert(status===0||(allowMissing&&status===1),`${tool} failed: ${err}`);
  const out=Buffer.concat(chunks);return raw?out:out.toString('utf8').trim();
}
async function visibleWindows() {
  const ids=(await desktop('xdotool',['search','--onlyvisible','--name','.*'],true)).split(/\s+/).filter(Boolean);
  return Promise.all(ids.map(async id=>({id,title:await desktop('xdotool',['getwindowname',id])})));
}
async function nativeDialog(before,kind) {
  const deadline=Date.now()+8000;let added=[];
  while(Date.now()<deadline) {
    added=(await visibleWindows()).filter(w=>!before.some(old=>old.id===w.id));
    if(added.length)break;
    await delay(50);
  }
  const dialogs=added.filter(w=>kind==='open'
    ?['Open','Open File','Open Files','Select a file this site can read'].includes(w.title)
    :(/^Save(?: .+)?$/.test(w.title)||w.title==='Warning: this site can see edits you make'));
  assert(dialogs.length===1,`No unique native ${kind} dialog: ${JSON.stringify(added)}`);
  return {before,added,id:dialogs[0].id};
}
async function selectNativePath(dialog,selected,prefix) {
  const id=dialog.id;
  await desktop('import',['-window','root',path.join(data,`${prefix}-dialog.png`)]);
  // DISPLAY belongs exclusively to the runner's new Xvfb server.
  await desktop('xdotool',['windowfocus','--sync',id]);
  assert(await desktop('xdotool',['getwindowfocus'])===id,'Native dialog must own focus');
  await desktop('xdotool',['key','--clearmodifiers','ctrl+l']);
  // Save As initially selects only the stem, leaving its extension in the
  // entry. Replace the whole value so the requested output stays exact.
  await desktop('xdotool',['key','--clearmodifiers','ctrl+a']);
  await desktop('xdotool',['type','--clearmodifiers','--delay','1','--',selected]);
  await desktop('import',['-window','root',path.join(data,`${prefix}-selected.png`)]);
  const geometry=await desktop('xdotool',['getwindowgeometry','--shell',id]);
  const dimensions=Object.fromEntries(geometry.split('\n').filter(line=>/^(WIDTH|HEIGHT)=\d+$/.test(line)).map(line=>line.split('=')));
  assert(Number(dimensions.WIDTH)>=800&&Number(dimensions.HEIGHT)>=500,'Unexpected native dialog geometry');
  const point={x:Number(dimensions.WIDTH)-52,y:Number(dimensions.HEIGHT)-23};
  dialog.confirm={geometry,point};
  dialog.confirm.readiness=await waitForGtkConfirm(
    crop=>desktop('import',['-window',id,'-crop',crop,'-depth','8','RGB:-'],false,true),point,
  );
  // Click once after the inspected GTK confirm label becomes enabled.
  await desktop('xdotool',['mousemove','--window',id,String(point.x),String(point.y),'click','1']);
  const closeDeadline=Date.now()+8000;
  while(Date.now()<closeDeadline&&(await visibleWindows()).some(w=>w.id===id))await delay(50);
  assert(!(await visibleWindows()).some(w=>w.id===id),`Native ${prefix} dialog did not close`);
  dialog.closed=true;
  await desktop('import',['-window','root',path.join(data,`after-${prefix}.png`)]);
}
async function openNativeInput() {
  const before=await visibleWindows();await key('o',2);
  receipt.nativeChooser=await nativeDialog(before,'open');
  await selectNativePath(receipt.nativeChooser,input,'native-open');
  receipt.inputMethod='Actual File System Access native open dialog in the owned Xvfb session';
}
async function stage(name) {
  receipt.stage=name;
  const children=(await readFile(`/proc/${browser.pid}/task/${browser.pid}/children`,'utf8')).trim().split(/\s+/).filter(Boolean);
  receipt.processObservation=await Promise.all(children.map(async pid=>{
    try {const stat=(await readFile(`/proc/${pid}/stat`,'utf8')).split(') ')[1].split(' ');return {pid:Number(pid),state:stat[0],start:stat[19],wait:(await readFile(`/proc/${pid}/wchan`,'utf8')).trim()};}catch{return {pid:Number(pid),gone:true}};
  }));
  await writeFile(path.join(data,'progress.json'),JSON.stringify(receipt,null,2));
}
function command(method,params={},sid) {
  assert(socket?.readyState===WebSocket.OPEN,'Owned CDP must be open');
  const id=++next;
  return new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>{pending.delete(id);reject(Error(`CDP timeout: ${method}`));},5000);
    pending.set(id,{resolve,reject,timer});
    socket.send(JSON.stringify({id,method,params,...(sid?{sessionId:sid}:{})}));
  });
}
const page=(method,params={})=>command(method,params,sessionId);
async function evaluate(expression) {
  const r=await page('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});
  assert(!r.exceptionDetails,JSON.stringify(r.exceptionDetails));return r.result.value;
}
async function ready(expression,seconds=25) {
  const end=Date.now()+seconds*1000;
  while(Date.now()<end) {
    if(await evaluate(`Boolean(${expression})`))return;
    await delay(100);
  }
  throw Error(`Readiness expired: ${expression}; state=${JSON.stringify(await evaluate('({url:location.href,ready:document.readyState,body:document.body?.innerText.slice(0,2000)})'))}`);
}
async function key(key,modifiers=0) {
  const special={Enter:['Enter',13],ArrowRight:['ArrowRight',39]};
  const [code,value]=special[key]??['Key'+key.toUpperCase(),key.toUpperCase().charCodeAt(0)];
  const selected=[{bit:2,key:'Control',code:'ControlLeft',value:17},{bit:1,key:'Alt',code:'AltLeft',value:18},{bit:8,key:'Shift',code:'ShiftLeft',value:16}].filter(m=>modifiers&m.bit);
  let state=0;
  for(const m of selected){state|=m.bit;await page('Input.dispatchKeyEvent',{type:'rawKeyDown',key:m.key,code:m.code,windowsVirtualKeyCode:m.value,nativeVirtualKeyCode:m.value,modifiers:state});}
  const actualKey=(modifiers&8)&&key.length===1?key.toUpperCase():key;
  await page('Input.dispatchKeyEvent',{type:'rawKeyDown',key:actualKey,code,windowsVirtualKeyCode:value,nativeVirtualKeyCode:value,modifiers:state});
  await page('Input.dispatchKeyEvent',{type:'keyUp',key:actualKey,code,windowsVirtualKeyCode:value,nativeVirtualKeyCode:value,modifiers:state});
  for(const m of selected.reverse()){state&=~m.bit;await page('Input.dispatchKeyEvent',{type:'keyUp',key:m.key,code:m.code,windowsVirtualKeyCode:m.value,nativeVirtualKeyCode:m.value,modifiers:state});}
}
async function clickLabel(label) {
  const r=await evaluate(`(() => {const items=[...document.querySelectorAll('button')].filter(b=>{const r=b.getBoundingClientRect();return b.innerText.trim()===${JSON.stringify(label)}&&!b.disabled&&r.width>0&&r.top>=0&&r.bottom<=innerHeight});if(items.length<1)throw Error('Expected a visible landing button');const r=items[0].getBoundingClientRect();return {x:r.left+r.width/2,y:r.top+r.height/2}})()`);
  await page('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...r});
  await page('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...r});
}
async function screenshot(name) {
  const r=await page('Page.captureScreenshot',{format:'png'});
  await writeFile(path.join(data,name),Buffer.from(r.data,'base64'));
}
try {
  const reserve=createServer();
  await new Promise(resolve=>reserve.listen(0,'127.0.0.1',resolve));
  const port=reserve.address().port;
  await new Promise((resolve,reject)=>reserve.close(e=>e?reject(e):resolve()));
  env.TAKOSUMI_CEF_PORT=String(port);
  const requests=(profileMode?PROFILE_IDS:[PROFILE_IDS[0]]).map(profile=>({key:`web-home:${profile}:${new URL(urls[app]).origin}`,url:urls[app],title:'Native CEF candidate',profile,webHome:true,resume:false}));
  await writeFile(path.join(data,'request.json'),JSON.stringify(profileMode?requests:requests[0]));
  browser=spawn('python3',['-B',path.join(repo,'native-tests/browser/guard.py'),path.join(data,'guard.json'),binary,
    ...(files==='cdp'?['--headless=new']:[]),'--no-sandbox','--no-first-run','--no-default-browser-check','--disable-background-networking',
    '--disable-component-update','--disable-sync','--password-store=basic','--lang=en-US','--remote-debugging-address=127.0.0.1'],
    {env,stdio:['ignore','ignore',log.fd],detached:true});
  exitPromise=new Promise((resolve,reject)=>{browser.once('exit',resolve);browser.once('error',reject)});
  await log.close();
  const end=Date.now()+20000;let endpoint, nativePid;
  while(Date.now()<end && !endpoint) {
    assert(browser.exitCode===null,'Owned CEF process exited during startup');
    const children=(await readFile(`/proc/${browser.pid}/task/${browser.pid}/children`,'utf8')).trim().split(/\s+/).filter(Boolean);
    for(const pid of children) {
      try {if(await readlink(`/proc/${pid}/exe`)===binary)nativePid=pid;}catch{}
    }
    if(nativePid) {
      const entries=(await readFile('/proc/net/tcp','utf8')).split('\n').slice(1).map(line=>line.trim().split(/\s+/));
      const item=entries.find(row=>row[1]===`0100007F:${port.toString(16).toUpperCase().padStart(4,'0')}`&&row[3]==='0A');
      if(item) {
        const handles=await readdir(`/proc/${nativePid}/fd`);
        const links=await Promise.all(handles.map(fd=>readlink(`/proc/${nativePid}/fd/${fd}`).catch(()=>'')));
        assert(links.includes(`socket:[${item[9]}]`),'Refusing a debug endpoint outside the owned native CEF process');
        const info=await (await fetch(`http://127.0.0.1:${port}/json/version`,{signal:AbortSignal.timeout(5000)})).json();
        endpoint=new URL(info.webSocketDebuggerUrl).pathname;
      }
    }
    if(!endpoint)await delay(50);
  }
  assert(endpoint?.startsWith('/devtools/browser/'),'No owned native CEF browser endpoint');
  receipt.nativePid=Number(nativePid);
  socket=new WebSocket(`ws://127.0.0.1:${port}${endpoint}`);
  await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('CDP handshake timeout')),5000);socket.onopen=()=>{clearTimeout(timer);resolve()};socket.onerror=()=>{clearTimeout(timer);reject(Error('CDP handshake failed'))}});
  socket.onmessage=e=>{
    const m=JSON.parse(String(e.data));
    if(m.id){const p=pending.get(m.id);if(p){clearTimeout(p.timer);pending.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result)}}
    else {
      events.push(m);if(m.method?.startsWith('Browser.download'))downloads.push({method:m.method,params:m.params});
      if(m.sessionId===sessionId&&navigation.length<32) {
        const p=m.params;
        if(m.method==='Network.requestWillBeSent'&&p.type==='Document') {
          documentRequests.add(p.requestId);
          const url=new URL(p.request.url);
          navigation.push({kind:'request',requestId:p.requestId,frameId:p.frameId,url:url.origin+url.pathname});
        }else if(m.method==='Network.responseReceived'&&documentRequests.has(p.requestId)) {
          navigation.push({kind:'response',requestId:p.requestId,status:p.response.status});
        }else if(m.method==='Network.loadingFailed'&&documentRequests.has(p.requestId)) {
          navigation.push({kind:'failure',requestId:p.requestId,error:p.errorText,canceled:p.canceled});
        }
      }
    }
  };
  receipt.version=await command('Browser.getVersion');
  const protocol=await (await fetch(`http://127.0.0.1:${port}/json/protocol`,{signal:AbortSignal.timeout(5000)})).json();
  await writeFile(path.join(data,'protocol.json'),JSON.stringify(protocol.domains.filter(d=>['Browser','Page','DOM'].includes(d.domain)).map(d=>({domain:d.domain,commands:d.commands.filter(c=>['setDownloadBehavior','setInterceptFileChooserDialog','setFileInputFiles'].includes(c.name)),events:d.events?.filter(e=>['downloadWillBegin','downloadProgress','fileChooserOpened'].includes(e.name))})),null,2));
  if(profileMode) {
    await stage('native two-profile bindings and same-origin state');
    receipt.profiles=await probeProfiles({command,data,url:urls[app],nonce,popup:mode==='profile-popup'});
    receipt.profileVerified=receipt.profiles.passed===true;
  }else {
  const targetDeadline=Date.now()+10000;let target;
  while(Date.now()<targetDeadline&&!target) {
    const targets=(await command('Target.getTargets')).targetInfos.filter(t=>t.type==='page');
    assert(targets.length<=1,'Expected one native candidate browser');
    target=targets[0];if(!target)await delay(50);
  }
  assert(target && (target.url==='about:blank'||new URL(target.url).origin===new URL(urls[app]).origin));
  receipt.target=target;
  await command('Browser.setDownloadBehavior',{behavior:'allowAndName',downloadPath:path.join(data,'Downloads'),eventsEnabled:true,...(target.browserContextId?{browserContextId:target.browserContextId}:{})});
  sessionId=(await command('Target.attachToTarget',{targetId:target.targetId,flatten:true})).sessionId;
  await page('Page.enable');await page('Runtime.enable');await page('DOM.enable');await page('Network.enable');
  await stage('await actual CEF editor');
  if(app==='photopea') {
    await ready("document.body?.innerText.includes('Start using Photopea') || document.body?.innerText.includes('Open From Computer')");
    if(await evaluate("document.body.innerText.includes('Start using Photopea')"))await clickLabel('Start using Photopea');
    await ready("document.body?.innerText.includes('Open From Computer')");
  }else await ready("document.querySelector('.excalidraw canvas') !== null");
  receipt.context=await evaluate("({origin:location.origin,secure:isSecureContext,title:document.title,body:document.body.innerText.slice(0,1800),notification:typeof Notification,pushManager:typeof PushManager,fileSystemPicker:typeof showOpenFilePicker,serviceWorker:'serviceWorker' in navigator,tauri:typeof __TAURI_INTERNALS__,ipc:typeof window.ipc,wryHandler:typeof window.webkit?.messageHandlers?.ipc})");
  assert(receipt.context.origin===new URL(urls[app]).origin && receipt.context.secure===true);
  assert(['tauri','ipc','wryHandler'].every(key=>receipt.context[key]==='undefined'),'Remote native IPC must be absent');
  receipt.nativeProfile=JSON.parse(await readFile(path.join(data,'native-profile.json'),'utf8'));
  assert(receipt.nativeProfile.nativeGlobalContext===false && receipt.nativeProfile.profile==='11111111-1111-4111-8111-111111111111');
  assert(receipt.nativeProfile.nativeCache===path.join(data,'cef-profiles',receipt.nativeProfile.profile),'Actual UUID context path mismatch');

  receipt.inputs=await evaluate("[...document.querySelectorAll('input[type=file]')].map(i=>({accept:i.accept,multiple:i.multiple,html:i.outerHTML}))");
  await screenshot('editor.png');
  if(mode==='operate') {
    await stage('open owned input');
    if(app==='photopea') {
      // Use the site's existing input through the standard browser automation
      // API. This measures File API import/export, not the OS chooser UI.
      const doc=await page('DOM.getDocument');
      const inputs=await page('DOM.querySelectorAll',{nodeId:doc.root.nodeId,selector:'input[type=file]'});
      assert(inputs.nodeIds.length===1,'Exactly one existing top-document file input is required');
      receipt.inputMethod='CDP supplies the actual existing file input; OS picker not qualified';
      await page('DOM.setFileInputFiles',{files:[input],nodeId:inputs.nodeIds[0]});
    }else if(files==='native-picker') {
      await openNativeInput();
    }else {
      await page('Page.setInterceptFileChooserDialog',{enabled:true});
      const eventStart=events.length;await key('o',2);
      const end=Date.now()+8000;let chooser;
      while(Date.now()<end && !chooser) {
        chooser=events.slice(eventStart).find(e=>e.method==='Page.fileChooserOpened'&&e.sessionId===sessionId)?.params;
        if(!chooser)await delay(50);
      }
      assert(chooser?.backendNodeId,'No owned file input chooser event; no Web API fallback or override applied');
      receipt.chooser=chooser;
      await page('DOM.setFileInputFiles',{files:[input],backendNodeId:chooser.backendNodeId});
    }
    if(app==='photopea') {
      await ready("document.body.innerText.includes('SumiProbeWork')",12);
      await stage('export imported file');
      await key('s',11);await ready("document.body.innerText.includes('Save for web')",8);
      await screenshot('export-dialog.png');await key('Enter');
    }else {
      // Excalidraw keeps a generic document title after native import. Observe
      // the actual centered red fixture in its canvas before editing it.
      await ready("[...document.querySelectorAll('.excalidraw canvas')].some(c=>{const context=c.getContext('2d');if(!context||!c.width||!c.height)return false;const p=context.getImageData(Math.floor(c.width/2),Math.floor(c.height/2),1,1).data;return p[0]>240&&p[1]<15&&p[2]<15&&p[3]>240})",12);
      receipt.importedCanvas=await evaluate("[...document.querySelectorAll('.excalidraw canvas')].map(c=>({width:c.width,height:c.height,center:c.getContext('2d')?Array.from(c.getContext('2d').getImageData(Math.floor(c.width/2),Math.floor(c.height/2),1,1).data):null}))");
      await stage(files==='native-picker'?'save selected file through File System Access':'export imported file');
      await key('a',2);await key('ArrowRight');
      if(files==='native-picker')receipt.beforeSaveWindows=await visibleWindows();
      await key('s',2);
    }
    let file;
    if(app==='excalidraw'&&files==='native-picker') {
      // The site's File System Access save opens a native Save As chooser;
      // it does not emit Browser.download events. Choose a new owned output.
      file=path.join(data,'Downloads','SumiProbeSaved.excalidraw');
      await assert.rejects(lstat(file),{code:'ENOENT'});
      receipt.nativeSaveChooser=await nativeDialog(receipt.beforeSaveWindows,'save');
      await selectNativePath(receipt.nativeSaveChooser,file,'native-save');
      // A site toast alone is insufficient; verify the saved nonce/geometry
      // in the real file exactly once after the site's completion notice.
      await ready("document.body.innerText.includes('File saved.')",8);
      receipt.save={kind:'File System Access Save As',file,completion:'native chooser closed, site notice and actual file verification'};
      await screenshot('saved.png');
    }else {
    const downloadDeadline=Date.now()+15000;let finished;
    while(Date.now()<downloadDeadline && !finished) {
      assert(!downloads.some(e=>e.method==='Browser.downloadProgress'&&e.params.state==='canceled'),'Browser download canceled');
      finished=downloads.find(e=>e.method==='Browser.downloadProgress'&&e.params.state==='completed');
      if(!finished)await delay(50);
    }
    const began=downloads.filter(e=>e.method==='Browser.downloadWillBegin');
    assert(finished && began.length===1 && began[0].params.guid===finished.params.guid,'Exactly one actual browser download must finish');
    const guid=finished.params.guid;assert(/^[0-9a-f-]{36}$/.test(guid));
    const frame=(await page('Page.getFrameTree')).frameTree.frame;
    assert(began[0].params.frameId===frame.id,'Download must belong to the selected root page');
    file=path.join(data,'Downloads',guid);
    if(finished.params.filePath)assert(finished.params.filePath===file,'Unexpected browser destination');
    receipt.download={began:began[0].params,finished:finished.params,file};
    }
    const saved=await lstat(file);assert(saved.isFile()&&!saved.isSymbolicLink()&&saved.size>0&&saved.size<=1024*1024,'Missing or oversized actual export');
    const proof=spawn('python3',['-B','-c',
      'import sys,json;from pathlib import Path;sys.path.insert(0,sys.argv[1]);from browser_apps import verify_export;print(json.dumps(verify_export(Path(sys.argv[2]),sys.argv[3],sys.argv[4])))',
      path.join(repo,'native-tests/desktop'),file,app,nonce],{env,stdio:['ignore','pipe','pipe']});
    let out='',err='';proof.stdout.on('data',d=>out+=d);proof.stderr.on('data',d=>err+=d);
    const status=await new Promise(resolve=>proof.once('exit',resolve));assert(status===0,err);
    receipt.export=JSON.parse(out);receipt.exportVerified=true;
  }
  }
}catch(e){
  error=e;receipt.error=String(e);
  try {receipt.failedFrame=(await page('Page.getFrameTree')).frameTree.frame;}catch{}
  try {receipt.failedTargets=(await command('Target.getTargets')).targetInfos.filter(t=>t.type==='page');}catch{}
  if(receipt.nativePid) {
    try {
      const tasks=await readdir(`/proc/${receipt.nativePid}/task`);
      receipt.nativeWaits=await Promise.all(tasks.slice(0,24).map(async tid=>({tid:Number(tid),name:(await readFile(`/proc/${receipt.nativePid}/task/${tid}/comm`,'utf8')).trim(),wait:(await readFile(`/proc/${receipt.nativePid}/task/${tid}/wchan`,'utf8')).trim()})));
    }catch{}
  }
  try{await screenshot('failure.png')}catch{}
  if(files==='native-picker'){try{await desktop('import',['-window','root',path.join(data,'native-failure.png')])}catch{}}
}
finally {
  try {
    if(socket?.readyState===WebSocket.OPEN)await command('Browser.close').catch(()=>{});
    socket?.close();
    if(browser){
      let ended=await Promise.race([exitPromise.then(()=>true),delay(5000).then(()=>false)]);
      if(!ended){
        if(receipt.nativePid) {
          try {
            const tasks=await readdir(`/proc/${receipt.nativePid}/task`);
            receipt.shutdownWaits=await Promise.all(tasks.slice(0,24).map(async tid=>({tid:Number(tid),name:(await readFile(`/proc/${receipt.nativePid}/task/${tid}/comm`,'utf8')).trim(),wait:(await readFile(`/proc/${receipt.nativePid}/task/${tid}/wchan`,'utf8')).trim()})));
          }catch{}
        }
        browser.kill('SIGTERM');
        ended=await Promise.race([exitPromise.then(()=>true),delay(5000).then(()=>false)]);
      }
      assert(ended,'Owned guard cleanup did not finish');
      receipt.guard=JSON.parse(await readFile(path.join(data,'guard.json'),'utf8'));
      receipt.lifecycle=confirmBrowserCleanup(browser.exitCode,receipt.guard,!error);
      if(!error)assert(receipt.lifecycle.forcedCleanup===0,'Normal closure must not force cleanup');
      closed=true;
    }
  }catch(e){receipt.cleanupError=String(e);error??=e;}
  for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('Owned CDP closed'));}pending.clear();
  receipt.cleanupConfirmed=closed;
  receipt.operationPassed=mode==='operate' && receipt.exportVerified===true && closed && !error;
  receipt.profilePassed=profileMode && receipt.profileVerified===true && closed && !error;
  receipt.popupPassed=mode==='profile-popup' && receipt.profiles?.popup?.passed===true && closed && !error;
  await writeFile(path.join(data,'reference.json'),JSON.stringify(receipt,null,2)+'\n');
}
if(error){console.error(error);process.exitCode=1;}else console.log(`${mode==='operate'?'PASS actual export':profileMode?'PASS native profile state':'OBSERVATION ONLY'}: ${app} ${data}`);
