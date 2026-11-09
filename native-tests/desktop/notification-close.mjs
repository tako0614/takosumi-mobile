// Keep the real OS daemon alive across a profile's normal native close.
// No synthetic notification action or disappearance deadline substitutes for it.
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { notificationRoute,notificationActions,requireNotificationAction } from './notification-route.mjs';

export async function probePendingNotificationClose({pages,data,nonce,desktop,backend,retirePersonal,workLive}) {
  const [work,personal]=pages;
  const key=`sumi_pending_notification_${nonce.replaceAll('-','')}`;
  const baselineKey=`sumi_notification_${nonce.replaceAll('-','')}`;
  const title=`Closing Personal ${nonce}`;
  const proof={passed:false,scope:'Actual Linux App pending nonpersistent notification and normal Personal close; bounded Work isolation observation, no Web Push/reset qualification'};
  const counts=async()=>({displayed:await backend.count('displayed'),waiting:await backend.count('waiting')});
  const actions=trace=>notificationActions(trace).length;
  const workState=()=>work.evaluate(`(()=>{const n=globalThis[${JSON.stringify(baselineKey)}];return {href:location.href,origin:location.origin,top:window===top,witness:globalThis[${JSON.stringify(key)}],notification:n?{clicks:n.clicks,errors:n.errors,trusted:n.trusted}:null}})()`);
  const targets=async()=>((await work.c.command('Target.getTargets')).targetInfos.filter(t=>t.type==='page')
    .map(t=>({id:t.targetId,url:t.url,context:t.browserContextId??null,opener:t.openerId??null})).sort((a,b)=>a.id.localeCompare(b.id)));
  async function waitFor(predicate,message) {
    const end=Date.now()+5000;
    while(Date.now()<end){if(await predicate())return;await delay(50);}
    throw Error(message);
  }
  try {
    await backend.owner();
    assert.deepEqual(await counts(),{displayed:0,waiting:0},'The preceding baseline must have closed its alerts');
    // Observe the real Work Notification object left by the preceding native
    // click test, without creating another alert or replacing any page API.
    await work.evaluate(`(()=>{if(globalThis[${JSON.stringify(key)}])throw Error('Fresh Work witness required');const n=globalThis[${JSON.stringify(baselineKey+'_object')}];if(!n)throw Error('Missing real Work Notification');const s=globalThis[${JSON.stringify(key)}]={nonce:${JSON.stringify(nonce)},clicks:0};n.addEventListener('click',()=>s.clicks++);return true})()`);
    proof.workBefore=await workState();
    assert.equal(proof.workBefore.origin,work.context.origin);assert.equal(proof.workBefore.top,true);
    proof.workTargetsBefore=await targets();
    assert(proof.workTargetsBefore.some(t=>t.id===work.target.targetId),'The attested Work page must be live');
    await personal.evaluate(`(()=>{if(globalThis[${JSON.stringify(key)}])throw Error('Fresh Personal state required');const s=globalThis[${JSON.stringify(key)}]={title:${JSON.stringify(title)},shown:0,errors:0,clicks:0};const n=new Notification(s.title,{body:'Owned pending Personal notification',tag:${JSON.stringify(key)},requireInteraction:true});n.onshow=()=>s.shown++;n.onerror=()=>s.errors++;n.onclick=()=>s.clicks++;globalThis[${JSON.stringify(key+'_object')}]=n;return true})()`);
    await waitFor(async()=>{
      const state=await personal.evaluate(`globalThis[${JSON.stringify(key)}]`);
      assert.equal(state.errors,0);return state.shown===1;
    },'The actual pending Personal notification was not shown');
    await waitFor(async()=>{const c=await counts();return c.displayed===1&&c.waiting===0;},'The sole pending Personal alert must reach the OS');
    proof.pending=await personal.evaluate(`globalThis[${JSON.stringify(key)}]`);
    const trace=await backend.trace();
    assert(trace.includes(`string "${title}"`),'The unique Personal title must reach the real desktop protocol');
    await waitFor(async()=>{
      proof.route=notificationRoute(await backend.trace(),title,backend.proof.uniqueName);
      return proof.route!==null;
    },'No exact native Notify reply for Personal');
    proof.clientPid=await backend.clientPid(proof.route.sender);
    assert.equal(proof.clientPid,personal.nativePid,'The Notify client must be the attested Personal browser');
    proof.actionCountBefore=actions(trace);
    proof.workGuardBefore=await workLive();
    proof.personalClose=await retirePersonal();
    proof.workGuardAfterClose=await workLive();
    proof.afterCloseCounts=await counts();
    assert.equal(proof.afterCloseCounts.waiting,0);
    assert([0,1].includes(proof.afterCloseCounts.displayed),'Unexpected alert after the only pending profile closed');
    if(proof.afterCloseCounts.displayed===0) {
      proof.outcome='OS alert absent after normal profile close; removal cause not inferred';
    } else {
      await backend.owner();
      const ids=(await desktop('xdotool',['search','--onlyvisible','--name',`^${backend.title}$`],true)).split(/\s+/).filter(Boolean);
      // A real asynchronous removal between the count and window read is an
      // observed removal, not permission to click an unrelated window.
      if(ids.length===0) {
        assert.deepEqual(await counts(),{displayed:0,waiting:0},'A missing alert window must have an actual empty backend');
        proof.outcome='OS alert retired before native input';
      } else {
        assert.equal(ids.length,1,'Require the sole owned native notification window');
        const window=ids[0];
        const owner=JSON.parse(await desktop('python3',['-B',fileURLToPath(new URL('../cef-candidate/x11-owner.py',import.meta.url)),window]));
        assert.equal(owner.pid,backend.daemon.pid);
        const geometry=await desktop('xdotool',['getwindowgeometry','--shell',window]);
        const sizes=Object.fromEntries(geometry.split('\n').map(line=>line.split('=')));
        assert(Number(sizes.WIDTH)>100&&Number(sizes.HEIGHT)>20);
        await desktop('import',['-window','root',path.join(data,'notification-pending-personal.png')]);
        await desktop('xdotool',['mousemove','--window',window,String(Number(sizes.WIDTH)-12),'10']);
        const pointer=await desktop('xdotool',['getmouselocation','--shell']);
        assert.equal(pointer.split('\n').find(line=>line.startsWith('WINDOW='))?.slice(7),window,'Native input must target the owned alert');
        proof.nativeAction={owner,window,geometry,pointer};
        await desktop('xdotool',['click','1']);
        await waitFor(async()=>actions(await backend.trace())>proof.actionCountBefore,'No real desktop action for the retained alert');
        assert.equal(actions(await backend.trace()),proof.actionCountBefore+1,'Exactly one native action must follow the baseline');
        proof.action=notificationActions(await backend.trace()).at(-1);
        requireNotificationAction(proof.action,proof.route);
        await waitFor(async()=>{const c=await counts();return c.displayed===0&&c.waiting===0;},'The real Dunst native click must close its alert');
        proof.outcome='Native action on retired Personal alert';
      }
    }
    // Work stays live for a bounded observation after the real retirement or
    // action. This is not a promise about arbitrarily delayed future delivery.
    const started=Date.now();
    proof.workSamples=0;
    do {
      await workLive();
      assert.deepEqual(await workState(),proof.workBefore,'Personal notification must not change Work or deliver a Work click');
      assert.deepEqual(await targets(),proof.workTargetsBefore,'Personal notification must not open or navigate a Work target');
      proof.workSamples++;
      await delay(100);
    } while(Date.now()-started<1000);
    proof.observedMilliseconds=Date.now()-started;
    proof.workAfter=await workState();proof.workTargetsAfter=await targets();
    proof.finalCounts=await counts();
    proof.actionCountAfter=actions(await backend.trace());
    assert.deepEqual(proof.finalCounts,{displayed:0,waiting:0},'No unrelated alert may replace the retired one');
    assert.equal(proof.actionCountAfter,proof.actionCountBefore+(proof.nativeAction?1:0),'Only the observed native input may invoke an action');
    proof.workGuardFinal=await workLive();
    proof.passed=true;return proof;
  }catch(error){proof.error=String(error);throw error;}
  finally {
    // Do not call Notification.close() on the retired page to manufacture OS
    // cleanup. The caller stops only its real backend after saving this proof.
    try {
      await writeFile(path.join(data,'notification-pending-close-result.json'),JSON.stringify(proof,null,2)+'\n');
    } catch(error) {
      if(!proof.error)throw error;
      console.error('Pending notification diagnostic write failed:',String(error));
    }
  }
}
