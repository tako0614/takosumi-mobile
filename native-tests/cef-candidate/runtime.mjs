// One explicitly owned CEF process. Reuses the existing pidfd guard and oracle.
import assert from 'node:assert/strict';
import { mkdir, open, readFile, readdir, readlink, writeFile } from 'node:fs/promises';
import { createServer } from 'node:net';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { confirmBrowserCleanup } from '../browser/lifecycle.ts';

const repo=fileURLToPath(new URL('../../',import.meta.url));

export class OwnedCefRuntime {
  constructor({data,binary,env,nonce,request}) {
    Object.assign(this,{data,binary,env:{...env,TAKOSUMI_CEF_CASE:data},nonce,request});
    this.pending=new Map();this.next=0;
    this.proof={profile:request.profile,data,closed:false};
    this.command=this.command.bind(this);
  }
  async start() {
    const {data,binary,env,request}=this;
    await mkdir(data,{mode:0o700});
    await writeFile(path.join(data,'.owned-browser-reference'),this.nonce,{flag:'wx'});
    await writeFile(path.join(data,'request.json'),JSON.stringify(request),{flag:'wx'});
    for(const [key,name] of Object.entries({HOME:'home',XDG_DATA_HOME:'data',XDG_CONFIG_HOME:'config',XDG_CACHE_HOME:'cache',XDG_RUNTIME_DIR:'runtime',TMPDIR:'tmp'})) {
      env[key]=path.join(data,name);await mkdir(env[key],{mode:0o700});
    }
    await mkdir(path.join(data,'Downloads'),{mode:0o700});
    const reserve=createServer();
    await new Promise(resolve=>reserve.listen(0,'127.0.0.1',resolve));
    const port=reserve.address().port;
    await new Promise((resolve,reject)=>reserve.close(error=>error?reject(error):resolve()));
    env.TAKOSUMI_CEF_PORT=String(port);
    const log=await open(path.join(data,'browser.log'),'wx',0o600);
    try {
      this.guard=spawn('python3',['-B',path.join(repo,'native-tests/browser/guard.py'),path.join(data,'guard.json'),binary,
        '--no-sandbox','--no-first-run','--no-default-browser-check','--disable-background-networking',
        '--disable-component-update','--disable-sync','--password-store=basic','--lang=en-US','--remote-debugging-address=127.0.0.1'],
        {env,stdio:['ignore','ignore',log.fd],detached:true});
      this.ended=new Promise(resolve=>{
        this.guard.once('exit',(code,signal)=>resolve({code,signal}));
        this.guard.once('error',error=>{this.spawnError=error;resolve({error:String(error)});});
      });
    }finally {await log.close();}
    const end=Date.now()+20000;let endpoint;
    while(Date.now()<end&&!endpoint) {
      assert(!this.spawnError&&this.guard.exitCode===null&&this.guard.signalCode===null,'Owned CEF guard must remain live during startup');
      const children=(await readFile(`/proc/${this.guard.pid}/task/${this.guard.pid}/children`,'utf8')).trim().split(/\s+/).filter(Boolean);
      const roots=[];
      for(const pid of children)if(await readlink(`/proc/${pid}/exe`).catch(()=>'')===binary)roots.push(pid);
      assert(roots.length<=1,'One native CEF root per owned guard');
      if(roots.length) {
        this.nativePid=Number(roots[0]);
        const rows=(await readFile('/proc/net/tcp','utf8')).split('\n').slice(1).map(line=>line.trim().split(/\s+/));
        const listener=rows.find(row=>row[1]===`0100007F:${port.toString(16).toUpperCase().padStart(4,'0')}`&&row[3]==='0A');
        if(listener) {
          const fds=await readdir(`/proc/${this.nativePid}/fd`);
          const links=await Promise.all(fds.map(fd=>readlink(`/proc/${this.nativePid}/fd/${fd}`).catch(()=>'')));
          assert(links.includes(`socket:[${listener[9]}]`),'Native PID must own its debug endpoint');
          const info=await (await fetch(`http://127.0.0.1:${port}/json/version`,{signal:AbortSignal.timeout(5000)})).json();
          endpoint=new URL(info.webSocketDebuggerUrl).pathname;
        }
      }
      if(!endpoint)await delay(50);
    }
    assert(endpoint?.startsWith('/devtools/browser/'),'No attested native CEF endpoint');
    this.proof.nativePid=this.nativePid;
    this.socket=new WebSocket(`ws://127.0.0.1:${port}${endpoint}`);
    await new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>reject(Error('Owned CDP handshake timeout')),5000);
      this.socket.onopen=()=>{clearTimeout(timer);resolve();};
      this.socket.onerror=()=>{clearTimeout(timer);reject(Error('Owned CDP handshake failed'));};
    });
    this.socket.onmessage=event=>{
      const message=JSON.parse(String(event.data));
      const pending=this.pending.get(message.id);
      if(pending) {
        clearTimeout(pending.timer);this.pending.delete(message.id);
        if(message.error)pending.reject(Error(JSON.stringify(message.error)));else pending.resolve(message.result);
      }
    };
    this.socket.onclose=()=>this.rejectPending();
    this.proof.version=await this.command('Browser.getVersion');
    assert.equal(this.proof.version.product,'Chrome/154.0.8037.98');
  }
  command(method,params={},sessionId) {
    assert(this.socket?.readyState===WebSocket.OPEN,'Owned CDP connection must be live');
    const id=++this.next;
    return new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>{this.pending.delete(id);reject(Error(`CDP timeout: ${method}`));},5000);
      this.pending.set(id,{resolve,reject,timer});
      this.socket.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));
    });
  }
  rejectPending() {
    for(const pending of this.pending.values()){clearTimeout(pending.timer);pending.reject(Error('Owned CDP closed'));}
    this.pending.clear();
  }
  async bind(deadline) {
    const marker=path.join(this.data,'native-roots-ready.json');let bindings;
    while(Date.now()<deadline&&!bindings) {
      try{bindings=JSON.parse(await readFile(marker,'utf8'));}catch(error){if(error.code!=='ENOENT')throw error;await delay(50);}
    }
    assert(bindings?.length===1,'Exactly one actual native profile root required per process');
    const [native]=bindings;
    assert.equal(native.profile,this.request.profile);
    assert(native.nativeGlobalContext===false&&native.popup===false);
    assert.equal(native.nativeCache,path.join(this.data,'cef-profiles',this.request.profile));
    assert.equal(native.nativeCache,native.requestedCache);
    const targets=(await this.command('Target.getTargets')).targetInfos.filter(target=>target.type==='page');
    assert.equal(targets.length,1,'No extra native page allowed in the profile process');
    const [target]=targets;
    const session=(await this.command('Target.attachToTarget',{targetId:target.targetId,flatten:true})).sessionId;
    const send=(method,params={})=>this.command(method,params,session);
    const evaluate=async(expression,userGesture=false)=>{
      const result=await send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true,userGesture});
      assert(!result.exceptionDetails,JSON.stringify(result.exceptionDetails));return result.result.value;
    };
    await send('Page.enable');await send('Runtime.enable');await send('Network.enable');
    const frame=(await send('Page.getFrameTree')).frameTree.frame;
    assert.equal(frame.id,native.frame.id);assert.equal(target.targetId,native.targetInfo.targetId);
    assert(target.browserContextId&&target.browserContextId===native.targetInfo.browserContextId);
    const origin=new URL(this.request.url).origin;
    assert.equal(new URL(native.frame.url).origin,origin);
    let loaded=false;
    while(Date.now()<deadline&&!loaded) {
      loaded=await evaluate(`location.origin===${JSON.stringify(origin)}&&document.readyState==='complete'`);
      if(!loaded)await delay(100);
    }
    assert(loaded,'Bound actual profile document must finish without forced navigation');
    const context=await evaluate('({origin:location.origin,secure:isSecureContext,tauri:typeof __TAURI_INTERNALS__,ipc:typeof window.ipc,wryHandler:typeof window.webkit?.messageHandlers?.ipc})');
    assert.equal(context.origin,origin);assert.equal(context.secure,true);
    for(const field of ['tauri','ipc','wryHandler'])assert.equal(context[field],'undefined');
    this.proof.binding={native,target,context};
    return {native,target,context,evaluate,send,browserCommand:this.command,nativePid:this.nativePid};
  }
  async stop() {
    if(this.stopped)return this.proof;
    this.stopped=true;
    try {
      if(this.guard) {
        if(this.socket?.readyState===WebSocket.OPEN&&this.guard.exitCode===null)await this.command('Browser.close').catch(()=>{});
        this.socket?.close();
        let ended=await Promise.race([this.ended.then(()=>true),delay(5000).then(()=>false)]);
        if(!ended){this.proof.guardStopRequested=true;this.guard.kill('SIGTERM');ended=await Promise.race([this.ended.then(()=>true),delay(5000).then(()=>false)]);}
        assert(ended,'Owned CEF guard drain did not finish');
        this.proof.guard=JSON.parse(await readFile(path.join(this.data,'guard.json'),'utf8'));
        confirmBrowserCleanup(this.guard.exitCode,this.proof.guard,true);
        assert.equal(this.proof.guard.forcedCleanup,0,'Normal native closure must not force cleanup');
        assert(!this.proof.guardStopRequested,'A controller stop signal cannot qualify normal closure');
        this.proof.closed=true;
      }
      return this.proof;
    }catch(error){this.proof.cleanupError=String(error);throw error;}
    finally {
      this.rejectPending();
      await writeFile(path.join(this.data,'runtime-result.json'),JSON.stringify(this.proof,null,2)+'\n');
    }
  }
}
