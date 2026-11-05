// Replayed false-success controls only; these mocks are not native evidence.
import assert from 'node:assert/strict';
import { mkdtemp, readFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';
import { probeNotifications } from './notifications.mjs';

const nonce='11111111-1111-4111-8111-111111111111';
const origin='https://excalidraw.com';
for(const fault of ['closed-page-object','wrong-profile-callback','foreign-window-owner','already-focused-target']) {
  test(`reject native notification evidence: ${fault}`,async()=>{
    const data=await mkdtemp(path.join(os.tmpdir(),'cef-notification-false-'));
    const instances=[];
    let clicks=0,focus='202';
    const pages=['Work','Personal'].map((label,index)=>{
      const context=vm.createContext({
        location:{origin},isSecureContext:true,window:{focus(){}},
        Notification:class {
          constructor() {
            instances[index]=this;
            queueMicrotask(()=>{
              this.onshow?.();
              if(index===1&&fault==='closed-page-object')instances[0].onclose?.();
            });
          }
          close(){this.onclose?.();}
        },
      });
      return {native:{profile:label},context:{origin},evaluate:async expression=>{
        const value=vm.runInContext(expression,context);
        return value===undefined?undefined:JSON.parse(JSON.stringify(value));
      }};
    });
    const backend={
      daemon:{pid:123},title:'Owned notification fixture',owner:async()=>123,
      count:async()=>1,
      trace:async()=>`string "Work ${nonce}"\nstring "Personal ${nonce}"`,
    };
    const windows=['101','202'].map(id=>({xWindow:{id}}));
    const desktop=async(tool,args)=>{
      if(tool==='python3')return JSON.stringify({pid:fault==='foreign-window-owner'?456:123});
      if(tool==='import')return '';
      assert.equal(tool,'xdotool');
      if(args[0]==='search')return '303';
      if(args[0]==='getwindowgeometry')return 'WIDTH=420\nHEIGHT=120';
      if(args[0]==='windowfocus'){focus=fault==='already-focused-target'?'101':args[2];return '';}
      if(args[0]==='getwindowfocus')return focus;
      if(args[0]==='mousemove') {
        clicks++;assert.equal(fault,'wrong-profile-callback');
        instances[1].onclick({isTrusted:true});return '';
      }
      throw Error(`Unexpected desktop operation ${args}`);
    };
    const messages={
      'closed-page-object':/pending native notification must still belong/,
      'wrong-profile-callback':/Native click must not reach the other profile/,
      'foreign-window-owner':/Notification window must belong/,
      'already-focused-target':/Restoration requires a distinct owned browser/,
    };
    await assert.rejects(probeNotifications({pages,data,nonce,desktop,backend,windows}),messages[fault]);
    assert.equal(clicks,fault==='wrong-profile-callback'?1:0);
    const proof=JSON.parse(await readFile(path.join(data,'notifications-result.json'),'utf8'));
    assert.equal(proof.passed,false);assert.equal(proof.pushPassed,false);
    if(fault==='wrong-profile-callback')assert.deepEqual(proof.clicks[0].observed.map(s=>s.clicks),[0,1]);
  });
}
