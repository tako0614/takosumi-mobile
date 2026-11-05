// Native OS file chooser steps for the actual App path, adapted from the
// standalone CEF candidate driver (native-tests/cef-candidate/operate.mjs),
// which stays unchanged as its own qualified source. The experimental English
// GTK dialog driver must fail when the expected unique dialog, focus or
// closure is missing; it never substitutes another input route.
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';
import { waitForGtkConfirm } from '../cef-candidate/gtk-readiness.mjs';

export function nativeFiles({desktop,data}) {
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
    await desktop('xdotool',['windowfocus','--sync',id]);
    assert(await desktop('xdotool',['getwindowfocus'])===id,'Native dialog must own focus');
    await desktop('xdotool',['key','--clearmodifiers','ctrl+l']);
    // Save As initially selects only the stem; replace the whole value so the
    // requested output stays exact.
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
  return {visibleWindows,nativeDialog,selectNativePath};
}

// Keyboard input through the page's own CDP session, as the standalone driver.
export async function key(send,key,modifiers=0) {
  const special={Enter:['Enter',13],ArrowRight:['ArrowRight',39]};
  const [code,value]=special[key]??['Key'+key.toUpperCase(),key.toUpperCase().charCodeAt(0)];
  const selected=[{bit:2,key:'Control',code:'ControlLeft',value:17},{bit:1,key:'Alt',code:'AltLeft',value:18},{bit:8,key:'Shift',code:'ShiftLeft',value:16}].filter(m=>modifiers&m.bit);
  let state=0;
  for(const m of selected){state|=m.bit;await send('Input.dispatchKeyEvent',{type:'rawKeyDown',key:m.key,code:m.code,windowsVirtualKeyCode:m.value,nativeVirtualKeyCode:m.value,modifiers:state});}
  const actualKey=(modifiers&8)&&key.length===1?key.toUpperCase():key;
  await send('Input.dispatchKeyEvent',{type:'rawKeyDown',key:actualKey,code,windowsVirtualKeyCode:value,nativeVirtualKeyCode:value,modifiers:state});
  await send('Input.dispatchKeyEvent',{type:'keyUp',key:actualKey,code,windowsVirtualKeyCode:value,nativeVirtualKeyCode:value,modifiers:state});
  for(const m of selected.reverse()){state&=~m.bit;await send('Input.dispatchKeyEvent',{type:'keyUp',key:m.key,code:m.code,windowsVirtualKeyCode:m.value,nativeVirtualKeyCode:m.value,modifiers:state});}
}

// The standalone driver's readiness wait with its own per-step bound.
export async function ready(evaluate,expression,seconds=25) {
  const end=Date.now()+seconds*1000;
  while(Date.now()<end) {
    if(await evaluate(`Boolean(${expression})`))return;
    await delay(100);
  }
  throw Error(`Readiness expired: ${expression}; state=${JSON.stringify(await evaluate('({url:location.href,ready:document.readyState,body:document.body?.innerText.slice(0,500)})'))}`);
}
