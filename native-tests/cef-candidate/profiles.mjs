// Native UUID-to-page bindings, then synthetic same-origin storage/cookie tests.
// No real login, account, permission grant or site Web API replacement.
import assert from 'node:assert/strict';
import { access, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

export const PROFILE_IDS=['11111111-1111-4111-8111-111111111111','22222222-2222-4222-8222-222222222222'];

export async function probeProfiles({command,data,url,nonce,popup=false}) {
  const proof={passed:false,stage:'native bindings'};
  try {
  const marker=path.join(data,'native-roots-ready.json');
  // One existing 25s page-readiness budget covers both native bindings and
  // both documents; it does not add a separate per-page extension.
  const deadline=Date.now()+25000;let ready=false;
  while(Date.now()<deadline&&!ready) {
    try {await access(marker);ready=true;}catch(e){if(e.code!=='ENOENT')throw e;}
    if(!ready)await delay(50);
  }
  assert(ready,'Both native root bindings must finish');
  const bindings=JSON.parse(await readFile(marker,'utf8'));
  proof.bindings=bindings;
  assert(bindings.length===2,'Exactly two actual native roots required');
  assert.deepEqual(bindings.map(b=>b.profile).sort(),[...PROFILE_IDS].sort());
  assert.equal(new Set(bindings.map(b=>b.browserId)).size,2);
  assert.equal(new Set(bindings.map(b=>b.frame.id)).size,2);
  const allTargets=(await command('Target.getTargets')).targetInfos;
  proof.targets=allTargets;
  const targets=allTargets.filter(t=>t.type==='page');
  assert.equal(targets.length,2,'Unexpected native root page');
  assert.equal(new Set(targets.map(t=>t.browserContextId)).size,2);
  assert(targets.every(t=>typeof t.browserContextId==='string'&&t.browserContextId.length>0));
  const candidates=[];
  for(const target of targets) {
    const session=(await command('Target.attachToTarget',{targetId:target.targetId,flatten:true})).sessionId;
    const send=(method,params={})=>command(method,params,session);
    const evaluate=async (expression,userGesture=false)=>{
      const value=await send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true,userGesture});
      assert(!value.exceptionDetails,JSON.stringify(value.exceptionDetails));return value.result.value;
    };
    await send('Page.enable');await send('Runtime.enable');await send('Network.enable');
    const frame=(await send('Page.getFrameTree')).frameTree.frame;
    candidates.push({target,frame,send,evaluate});
  }
  proof.candidateFrames=candidates.map(p=>({target:p.target,frame:p.frame}));
  const pages=[];
  for(const profile of PROFILE_IDS) {
    const native=bindings.find(b=>b.profile===profile);
    assert(native.nativeGlobalContext===false&&native.popup===false);
    assert.equal(native.nativeCache,path.join(data,'cef-profiles',profile));
    assert.equal(native.nativeCache,native.requestedCache);
    assert.equal(new URL(native.frame.url).origin,new URL(url).origin);
    const candidate=candidates.find(p=>p.frame.id===native.frame.id);
    assert(candidate,'Native browser frame must match exactly one actual page');
    const {target,send,evaluate}=candidate;
    let loaded=false;
    while(Date.now()<deadline&&!loaded) {
      loaded=await evaluate(`location.origin===${JSON.stringify(new URL(url).origin)}&&document.readyState==='complete'`);
      if(!loaded)await delay(100);
    }
    assert(loaded,'Actual profile page must load without forced navigation');
    assert.equal((await send('Page.getFrameTree')).frameTree.frame.id,native.frame.id);
    const context=await evaluate("({origin:location.origin,secure:isSecureContext,title:document.title,tauri:typeof __TAURI_INTERNALS__,ipc:typeof window.ipc,wryHandler:typeof window.webkit?.messageHandlers?.ipc})");
    assert.equal(context.origin,new URL(url).origin);assert.equal(context.secure,true);
    for(const field of ['tauri','ipc','wryHandler'])assert.equal(context[field],'undefined');
    pages.push({native,target,context,send,evaluate});
  }
  const [work,personal]=pages;
  const key=`sumi_candidate_${nonce}`, cookieName=`sumi_candidate_${nonce.replaceAll('-','')}`;
  const get=page=>page.evaluate(`localStorage.getItem(${JSON.stringify(key)})`);
  const set=(page,value)=>page.evaluate(`(()=>{localStorage.setItem(${JSON.stringify(key)},${JSON.stringify(value)});return true})()`);
  const cookies=async page=>(await page.send('Network.getCookies',{urls:[url]})).cookies.filter(c=>c.name===cookieName);
  const setCookie=async(page,value)=>{
    const result=await page.send('Network.setCookie',{name:cookieName,value,url,secure:true,httpOnly:true,sameSite:'Lax'});
    assert.equal(result.success,true);
  };
  Object.assign(proof,{bindings,contexts:pages.map(p=>({profile:p.native.profile,target:p.target,context:p.context})),cookieSetup:'Synthetic CDP HttpOnly cookie, not server login',storage:{},cookies:{},stage:'same-origin state'});
  proof.storage.initial=[await get(work),await get(personal)];assert.deepEqual(proof.storage.initial,[null,null]);
  await set(work,`work:${nonce}`);await setCookie(work,`work:${nonce}`);
  proof.storage.personalAfterWork=await get(personal);assert.equal(proof.storage.personalAfterWork,null);
  proof.cookies.personalAfterWork=await cookies(personal);assert.deepEqual(proof.cookies.personalAfterWork,[]);
  await set(personal,`personal:${nonce}`);await setCookie(personal,`personal:${nonce}`);
  proof.storage.workAfterPersonal=await get(work);assert.equal(proof.storage.workAfterPersonal,`work:${nonce}`);
  proof.storage.personal=await get(personal);assert.equal(proof.storage.personal,`personal:${nonce}`);
  for(const [index,page] of pages.entries()) {
    const value=`${index===0?'work':'personal'}:${nonce}`;
    const actual=await cookies(page);assert.equal(actual.length,1);
    assert(actual[0].httpOnly===true&&actual[0].secure===true&&actual[0].value===value);
    proof.cookies[index===0?'work':'personal']=actual;
    assert.equal(await page.evaluate(`document.cookie.includes(${JSON.stringify(cookieName)})`),false,'HttpOnly test cookie must remain hidden from JavaScript');
  }
  const observeClose=async(page,survivors)=>{
  const closePath=path.join(data,`native-closed-${page.native.browserId}.json`);
  const closeDeadline=Date.now()+5000;let closedNative=false,remaining=[];
  while(Date.now()<closeDeadline) {
    try {await access(closePath);closedNative=true;}catch(e){if(e.code!=='ENOENT')throw e;}
    remaining=(await command('Target.getTargets')).targetInfos;
    if(closedNative&&!remaining.some(t=>t.targetId===page.target.targetId))break;
    await delay(50);
  }
  assert(closedNative&&!remaining.some(t=>t.targetId===page.target.targetId),'Native browser must actually close before retained-state reads');
  for(const survivor of survivors)assert(remaining.some(t=>t.targetId===survivor.target.targetId),'Expected browser must remain open');
  const result=JSON.parse(await readFile(closePath,'utf8'));
  assert.equal(result.event.browserId,page.native.browserId);
  assert.equal(result.event.profile,page.native.profile);
  assert.equal(result.binding.frame.id,page.native.frame.id);
  return result;
  };
  if(popup) {
    proof.stage='native same-origin popup';
    const state=proof.popup={passed:false,method:'Standard window.open/window.close with synthetic state, not provider login'};
    const reference=`sumi_popup_${nonce.replaceAll('-','')}`;
    state.accepted=await work.evaluate(`(()=>{globalThis[${JSON.stringify(reference)}]=window.open(${JSON.stringify(url)},${JSON.stringify(reference)},'popup,width=600,height=500');return Boolean(globalThis[${JSON.stringify(reference)}])})()`,true);
    assert.equal(state.accepted,true,'Popup request must be accepted');
    const appeared=Date.now()+10000;let target;
    while(Date.now()<appeared&&!target) {
      const actual=(await command('Target.getTargets')).targetInfos.filter(t=>t.type==='page');
      const found=actual.filter(t=>t.openerId===work.target.targetId&&!targets.some(old=>old.targetId===t.targetId));
      assert(found.length<=1,'Unexpected popup family');target=found[0];
      if(!target)await delay(50);
    }
    assert(target,'Actual popup target must appear');state.target=target;
    assert.equal(target.browserContextId,work.target.browserContextId);
    const session=(await command('Target.attachToTarget',{targetId:target.targetId,flatten:true})).sessionId;
    const send=(method,params={})=>command(method,params,session);
    const evaluate=async(expression,userGesture=false)=>{
      const value=await send('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true,userGesture});
      assert(!value.exceptionDetails,JSON.stringify(value.exceptionDetails));return value.result.value;
    };
    await send('Page.enable');await send('Runtime.enable');await send('Network.enable');
    const end=Date.now()+25000;let loaded=false;
    while(Date.now()<end&&!loaded) {
      loaded=await evaluate(`location.origin===${JSON.stringify(new URL(url).origin)}&&document.readyState==='complete'`);
      if(!loaded)await delay(100);
    }
    assert(loaded,'Actual popup document must load');
    const frame=(await send('Page.getFrameTree')).frameTree.frame;
    assert(/^[0-9a-f]{32}$/i.test(frame.id));
    const marker=path.join(data,`native-frame-${frame.id}.json`);let bound=false;
    while(Date.now()<end&&!bound) {
      try{await access(marker);bound=true;}catch(e){if(e.code!=='ENOENT')throw e;}
      if(!bound)await delay(50);
    }
    assert(bound,'Native popup frame binding must finish');
    const native=state.native=JSON.parse(await readFile(marker,'utf8'));
    assert(native.popup===true&&native.nativeGlobalContext===false);
    assert.equal(native.profile,work.native.profile);
    assert.equal(native.nativeCache,work.native.nativeCache);assert.equal(native.frame.id,frame.id);
    const child={native,target,send,evaluate};
    state.context=await evaluate("({origin:location.origin,secure:isSecureContext,opener:window.opener!==null,openerOrigin:window.opener?.location.origin,tauri:typeof __TAURI_INTERNALS__,ipc:typeof window.ipc,wryHandler:typeof window.webkit?.messageHandlers?.ipc})");
    assert.equal(state.context.origin,new URL(url).origin);assert.equal(state.context.secure,true);
    assert.equal(state.context.opener,true);assert.equal(state.context.openerOrigin,new URL(url).origin);
    for(const field of ['tauri','ipc','wryHandler'])assert.equal(state.context[field],'undefined');
    state.storageBefore=await get(child);assert.equal(state.storageBefore,`work:${nonce}`);
    state.cookiesBefore=await cookies(child);assert.equal(state.cookiesBefore.length,1);assert.equal(state.cookiesBefore[0].value,`work:${nonce}`);
    await set(child,`popup:${nonce}`);await setCookie(child,`popup:${nonce}`);
    await evaluate('(()=>{setTimeout(()=>window.close(),0);return true})()',true);
    state.close=await observeClose(child,[work,personal]);
    assert.equal(await work.evaluate(`globalThis[${JSON.stringify(reference)}].closed`),true);
    state.parentStorage=await get(work);assert.equal(state.parentStorage,`popup:${nonce}`);
    state.parentCookies=await cookies(work);assert.equal(state.parentCookies.length,1);assert.equal(state.parentCookies[0].value,`popup:${nonce}`);
    state.personalStorage=await get(personal);assert.equal(state.personalStorage,`personal:${nonce}`);
    state.personalCookies=await cookies(personal);assert.equal(state.personalCookies.length,1);assert.equal(state.personalCookies[0].value,`personal:${nonce}`);
    state.passed=true;
  }
  const closed=await command('Target.closeTarget',{targetId:work.target.targetId});assert.equal(closed.success,true);
  proof.workClose=await observeClose(work,[personal]);
  proof.storage.personalAfterWorkClose=await get(personal);assert.equal(proof.storage.personalAfterWorkClose,`personal:${nonce}`);
  proof.cookies.personalAfterWorkClose=await cookies(personal);
  assert.equal(proof.cookies.personalAfterWorkClose.length,1);
  assert.equal(proof.cookies.personalAfterWorkClose[0].value,`personal:${nonce}`);
  assert(proof.cookies.personalAfterWorkClose[0].httpOnly===true&&proof.cookies.personalAfterWorkClose[0].secure===true);
  proof.passed=true;
  return proof;
  }finally {
    await writeFile(path.join(data,'profiles-result.json'),JSON.stringify(proof,null,2)+'\n');
  }
}
