// Standard page Notifications -> real Dunst windows -> native input -> page.
// No provider Push subscription, fake OS service or page API replacement.
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export async function probeNotifications({pages,data,nonce,desktop,backend,windows}) {
  const proof={passed:false,pushPassed:false,scope:'Owned Linux notification backend display/click/profile component; network Push remains unqualified'};
  const key=`sumi_notification_${nonce.replaceAll('-','')}`;
  const labels=['Work','Personal'];
  const state=async page=>{
    const value=await page.evaluate(`({origin:location.origin,secure:isSecureContext,state:globalThis[${JSON.stringify(key)}]})`);
    assert.equal(value.origin,page.context.origin);assert.equal(value.secure,true);
    return value.state;
  };
  async function settled(predicate,message) {
    const end=Date.now()+5000;
    while(Date.now()<end) {if(await predicate())return;await delay(50);}
    throw Error(message);
  }
  try {
    proof.tag=nonce;proof.notifications=[];
    for(const [i,page] of pages.entries()) {
      const title=`${labels[i]} ${nonce}`;
      const started=await page.evaluate(`(()=>{if(globalThis[${JSON.stringify(key)}])throw Error('Fresh notification state required');const s=globalThis[${JSON.stringify(key)}]={title:${JSON.stringify(title)},tag:${JSON.stringify(nonce)},shown:0,clicks:0,errors:0,closed:0,trusted:null};const n=new Notification(s.title,{body:${JSON.stringify('Owned '+labels[i]+' notification')},tag:s.tag,requireInteraction:true});n.onshow=()=>s.shown++;n.onerror=()=>s.errors++;n.onclose=()=>s.closed++;n.onclick=e=>{s.clicks++;s.trusted=e.isTrusted;window.focus()};globalThis[${JSON.stringify(key+'_object')}]=n;return {title:s.title,tag:s.tag}})()`);
      assert.equal(started.title,title);assert.equal(started.tag,nonce);
      await settled(async()=>{const s=await state(page);assert.equal(s.errors,0);return s.shown===1;},'No page notification show event');
      proof.notifications.push({profile:page.native.profile,title,initial:await state(page)});
    }
    await settled(async()=>await backend.count('displayed')===1&&await backend.count('waiting')===1,'Both native profile notifications must coexist with the same Web tag');
    proof.countsBefore={displayed:await backend.count('displayed'),waiting:await backend.count('waiting')};
    const trace=await backend.trace();
    for(const item of proof.notifications)assert(trace.includes(`string "${item.title}"`),'Expected browser notification must reach the actual desktop protocol');
    proof.clicks=[];
    for(const [index,page] of pages.entries()) {
      await backend.owner();
      const ids=(await desktop('xdotool',['search','--onlyvisible','--name',`^${backend.title}$`],true)).split(/\s+/).filter(Boolean);
      assert.equal(ids.length,1,'Require a unique native notification window');
      const window=ids[0];
      const owner=JSON.parse(await desktop('python3',['-B',fileURLToPath(new URL('./x11-owner.py',import.meta.url)),window]));
      assert.equal(owner.pid,backend.daemon.pid,'Notification window must belong to the real owned daemon');
      const geometry=await desktop('xdotool',['getwindowgeometry','--shell',window]);
      const sizes=Object.fromEntries(geometry.split('\n').map(line=>line.split('=')));
      assert(Number(sizes.WIDTH)>100&&Number(sizes.HEIGHT)>20);
      const expectedWindow=windows[index].xWindow.id;
      const otherWindow=windows[1-index].xWindow.id;
      assert.notEqual(expectedWindow,otherWindow);
      await desktop('xdotool',['windowfocus','--sync',otherWindow]);
      const focusedBefore=await desktop('xdotool',['getwindowfocus']);
      assert.equal(focusedBefore,otherWindow,'Restoration requires a distinct owned browser window to have focus before the click');
      await desktop('import',['-window','root',path.join(data,`notification-${labels[index].toLowerCase()}.png`)]);
      const before=await Promise.all(pages.map(state));
      assert.deepEqual(before.map(s=>s.clicks),index===0?[0,0]:[1,0]);
      const click={profile:page.native.profile,owner,window,geometry,before,expectedWindow,focusedBefore};
      proof.clicks.push(click);
      assert.equal(before[index].closed,0,'A pending native notification must still belong to its live page object');
      await desktop('xdotool',['mousemove','--window',window,String(Math.floor(Number(sizes.WIDTH)/2)),String(Math.floor(Number(sizes.HEIGHT)/2)),'click','1']);
      await settled(async()=>{
        const values=await Promise.all(pages.map(state));
        click.observed=values;
        assert.equal(values[1-index].clicks,index===0?0:1,'Native click must not reach the other profile');
        assert(values.every(s=>s.errors===0),'Notification errored after native input');
        return values[index].clicks===1;
      },'Native notification action did not reach its original page');
      const after=await Promise.all(pages.map(state));
      assert.equal(after[index].trusted,true,'Page click must be a trusted browser event');
      await settled(async()=>(await desktop('xdotool',['getwindowfocus']))===expectedWindow,'Standard page focus after notification click did not restore the original window');
      Object.assign(click,{after,focused:await desktop('xdotool',['getwindowfocus'])});
      if(index===0)await settled(async()=>await backend.count('displayed')===1&&await backend.count('waiting')===0,'The other profile notification must remain available after first click');
    }
    await settled(async()=>await backend.count('displayed')===0&&await backend.count('waiting')===0,'Native notifications must actually close');
    const finalTrace=await backend.trace();
    assert((finalTrace.match(/member=ActionInvoked/g)??[]).length>=2,'Actual desktop action signals must be recorded');
    proof.passed=true;return proof;
  }catch(error) {
    proof.error=String(error);throw error;
  }finally {
    proof.finalStates=await Promise.all(pages.map(page=>state(page).catch(error=>({error:String(error)}))));
    for(const page of pages)try{await page.evaluate(`globalThis[${JSON.stringify(key+'_object')}]?.close()`);}catch{}
    await writeFile(path.join(data,'notifications-result.json'),JSON.stringify(proof,null,2)+'\n');
  }
}
