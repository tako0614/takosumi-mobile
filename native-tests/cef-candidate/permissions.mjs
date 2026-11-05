// Inspect native Chrome permission UI; never grant permissions through CDP.
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

export async function inspectPermissions({pages,command,data,nonce,desktop,nativePid,decide=false,notificationProbe}) {
  const proof={permissionPassed:false,notificationPassed:false,method:decide?'Inspected English Chrome default UI; actual allow/deny, no CDP grant':'Default Chrome UI observation only; no grant or notification delivery qualified'};
  const [work,personal]=pages;
  const state=async page=>{
    const value=await page.evaluate("(async()=>({origin:location.origin,secure:isSecureContext,notification:Notification.permission,query:(await navigator.permissions.query({name:'notifications'})).state}))()");
    assert.equal(value.origin,page.context.origin,'Permission site must stay at the bound origin');
    assert.equal(value.secure,true,'Permission site must remain secure');
    return value;
  };
  const property=`sumi_permission_${nonce.replaceAll('-','')}`;
  const originalTitles=await Promise.all(pages.map(page=>page.evaluate('document.title')));
  async function request(page,label,choice) {
    const step={profile:page.native.profile};
    // Window IDs are scoped to a browser process, not the desktop session.
    const browserCommand=page.browserCommand??command;
    const expectedPid=page.nativePid??nativePid;
    assert(Number.isInteger(expectedPid)&&expectedPid>0,'An attested native browser PID is required');
    proof[label]=step;
    const title=`Sumi Permission ${label} ${nonce}`;
    await page.evaluate(`document.title=${JSON.stringify(title)}`);
    await page.send('Page.bringToFront');
    step.initialWindow=await browserCommand('Browser.getWindowForTarget',{targetId:page.target.targetId});
    // Native roots cascade by 10px. Place each owned test window in the
    // inspected geometry before requesting permission, then verify readback.
    await browserCommand('Browser.setWindowBounds',{windowId:step.initialWindow.windowId,bounds:{left:10,top:10,width:1050,height:1004,windowState:'normal'}});
    step.window=await browserCommand('Browser.getWindowForTarget',{targetId:page.target.targetId});
    const end=Date.now()+5000;let ids=[];
    while(Date.now()<end&&!ids.length) {
      ids=(await desktop('xdotool',['search','--onlyvisible','--name',title],true)).split(/\s+/).filter(Boolean);
      if(!ids.length)await delay(50);
    }
    assert.equal(ids.length,1,'Require a unique owned visible browser title');
    const id=ids[0];
    step.xWindow={id,title:await desktop('xdotool',['getwindowname',id]),pid:Number(await desktop('xdotool',['getwindowpid',id]))};
    assert.equal(step.xWindow.pid,expectedPid,'Refusing a window outside the owned native browser');
    step.xWindow.geometry=await desktop('xdotool',['getwindowgeometry','--shell',id]);
    for(const expected of ['X=10','Y=10','WIDTH=1050','HEIGHT=1004'])assert(step.xWindow.geometry.split('\n').includes(expected),'Native window geometry must match the inspected layout');
    await desktop('xdotool',['windowfocus','--sync',id]);
    assert.equal(await desktop('xdotool',['getwindowfocus']),id);
    step.request=await page.evaluate(`(()=>{globalThis[${JSON.stringify(property)}]={started:true,result:null,error:null,origin:location.origin,secure:isSecureContext};globalThis[${JSON.stringify(property+'_promise')}]=Notification.requestPermission().then(result=>{globalThis[${JSON.stringify(property)}].result=result;return result},error=>{globalThis[${JSON.stringify(property)}].error=String(error);throw error});return globalThis[${JSON.stringify(property)}]})()`,true);
    assert.equal(step.request.origin,page.context.origin,'Permission request must use the bound origin');
    assert.equal(step.request.secure,true);
    await delay(1000);
    await desktop('import',['-window','root',path.join(data,`permission-${label}-ui.png`)]);
    if(choice) {
      // Coordinates are from the retained, visually inspected native prompt
      // of this English Linux CEF build and geometry, not a portable UI API.
      assert.deepEqual(step.window.bounds,{left:10,top:10,width:1050,height:1004,windowState:'normal'},'Uninspected permission UI geometry');
      const layouts={'https://www.photopea.com':{name:'wrapped origin',y:216},'https://excalidraw.com':{name:'single-line origin',y:192}};
      const layout=layouts[page.context.origin];assert(layout,'Uninspected permission origin layout');
      step.choice={kind:choice,layout:layout.name,x:choice==='allow'?390:310,y:layout.y};
      step.preClick=await page.evaluate(`globalThis[${JSON.stringify(property)}]`);
      assert.deepEqual(step.preClick,{started:true,result:null,error:null,origin:page.context.origin,secure:true},'Permission request must still be pending at the bound origin immediately before the single native choice');
      assert.equal(await desktop('xdotool',['getwindowfocus']),id,'Permission choice must retain the owned window focus');
      await desktop('xdotool',['mousemove','--window',id,String(step.choice.x),String(step.choice.y),'click','1']);
      step.resolved=await page.evaluate(`globalThis[${JSON.stringify(property+'_promise')}]`);
      assert.equal(step.resolved,choice==='allow'?'granted':'denied','The native choice must resolve the actual permission request');
      await desktop('import',['-window','root',path.join(data,`permission-${label}-resolved.png`)]);
    }
    step.result=await page.evaluate(`globalThis[${JSON.stringify(property)}]`);
    return step;
  }
  try {
    proof.before={work:await state(work),personal:await state(personal)};
    for(const s of Object.values(proof.before))assert(s.secure&&s.notification==='default'&&s.query==='prompt');
    await request(work,'work',decide?'allow':null);
    proof.afterWork={work:await state(work),personal:await state(personal)};
    if(decide) {
      assert.equal(proof.afterWork.work.notification,'granted');assert.equal(proof.afterWork.work.query,'granted');
      assert.equal(proof.afterWork.personal.notification,'default');assert.equal(proof.afterWork.personal.query,'prompt');
      const personalChoice=notificationProbe?'allow':'block';
      proof.personalChoice=personalChoice;
      await request(personal,'personal',personalChoice);
      proof.afterChoices={work:await state(work),personal:await state(personal)};
      assert.equal(proof.afterChoices.work.notification,'granted');assert.equal(proof.afterChoices.work.query,'granted');
      const expectedPersonal=notificationProbe?'granted':'denied';
      assert.equal(proof.afterChoices.personal.notification,expectedPersonal);assert.equal(proof.afterChoices.personal.query,expectedPersonal);
      if(notificationProbe) {
        proof.nativeNotifications=await notificationProbe({pages,windows:[proof.work,proof.personal]});
      }else {
      const notification=page=>page.evaluate(`new Promise(resolve=>{let n;const result={event:null,error:null};const finish=event=>{result.event=event;clearTimeout(timer);n?.close();resolve(result)};const timer=setTimeout(()=>finish('timeout'),3000);try{n=new Notification(${JSON.stringify('Sumi probe '+nonce)},{body:'Owned browser notification component probe',tag:${JSON.stringify(nonce)}});n.onshow=()=>finish('show');n.onerror=()=>finish('error')}catch(e){result.error=String(e);finish('constructor-error')}})`);
      proof.notification={work:await notification(work),personal:await notification(personal),scope:'Observed API events only; OS display, delivery, click and Push remain unqualified'};
      assert.notEqual(proof.notification.personal.event,'show','Denied profile must not show a notification');
      }
    }
    proof.after={work:await state(work),personal:await state(personal)};
    if(decide) {
      assert.equal(proof.after.work.notification,'granted');assert.equal(proof.after.work.query,'granted');
      const expectedPersonal=notificationProbe?'granted':'denied';
      assert.equal(proof.after.personal.notification,expectedPersonal);assert.equal(proof.after.personal.query,expectedPersonal);
      proof.permissionPassed=true;
    }
    return proof;
  }finally {
    for(const [i,page] of pages.entries())await page.evaluate(`document.title=${JSON.stringify(originalTitles[i])}`).catch(()=>{});
    await writeFile(path.join(data,'permissions-result.json'),JSON.stringify(proof,null,2)+'\n');
  }
}
