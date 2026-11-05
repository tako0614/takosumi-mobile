// Replay false-qualification controls at the probe's CDP/desktop boundary.
// These are oracle tests, not native permission or notification evidence.
import assert from 'node:assert/strict';
import { mkdtemp, readFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { inspectPermissions } from './permissions.mjs';

const origin='https://excalidraw.com';
const bounds={left:10,top:10,width:1050,height:1004,windowState:'normal'};
for(const fault of ['state-origin','state-insecure','request-origin','resolved-before-click','preclick-origin','foreign-native-pid','process-local-command']) {
  test(`reject ${fault} before native click`,async()=>{
    const data=await mkdtemp(path.join(os.tmpdir(),'cef-permission-false-'));
    let clicks=0;
    const snapshot={started:true,result:null,error:null,origin,secure:true};
    const pages=['work','personal'].map(label=>({
      native:{profile:label},context:{origin},target:{targetId:label},
      nativePid:fault==='foreign-native-pid'?456:undefined,
      browserCommand:fault==='process-local-command'?async()=>{throw Error('Bound browser namespace reached');}:undefined,
      send:async method=>{assert.equal(method,'Page.bringToFront');return {};},
      evaluate:async expression=>{
        if(expression==='document.title')return 'Fixture';
        if(expression.startsWith('document.title='))return true;
        if(expression.includes('navigator.permissions.query'))return {
          origin:fault==='state-origin'?'https://other.invalid':origin,
          secure:fault!=='state-insecure',notification:'default',query:'prompt',
        };
        if(expression.includes('Notification.requestPermission'))return {
          ...snapshot,origin:fault==='request-origin'?'https://other.invalid':origin,
        };
        if(expression.startsWith('globalThis['))return {
          ...snapshot,
          result:fault==='resolved-before-click'?'granted':null,
          origin:fault==='preclick-origin'?'https://other.invalid':origin,
        };
        throw Error(`Unexpected evaluation: ${expression}`);
      },
    }));
    const command=async method=>{
      if(method==='Browser.getWindowForTarget')return {windowId:1,bounds};
      assert.equal(method,'Browser.setWindowBounds');return {};
    };
    const desktop=async(tool,args)=>{
      if(tool==='import')return '';
      assert.equal(tool,'xdotool');
      if(args[0]==='mousemove'){clicks++;return '';}
      const values={search:'42',getwindowname:'Owned fixture',getwindowpid:'123',
        windowfocus:'',getwindowfocus:'42',getwindowgeometry:'X=10\nY=10\nWIDTH=1050\nHEIGHT=1004'};
      assert(Object.hasOwn(values,args[0]));return values[args[0]];
    };
    const message=fault==='foreign-native-pid'?/Refusing a window outside the owned native browser/:
      fault==='process-local-command'?/Bound browser namespace reached/:
      fault.startsWith('state-')?/Permission site must/:
      fault==='request-origin'?/Permission request must use the bound origin/:/Permission request must still be pending/;
    await assert.rejects(inspectPermissions({pages,command,data,nonce:'11111111-1111-4111-8111-111111111111',desktop,nativePid:123,decide:true}),message);
    assert.equal(clicks,0,'Invalid permission evidence must not reach a native choice');
    const proof=JSON.parse(await readFile(path.join(data,'permissions-result.json'),'utf8'));
    assert.equal(proof.permissionPassed,false);
    assert.equal(proof.notificationPassed,false);
  });
}
