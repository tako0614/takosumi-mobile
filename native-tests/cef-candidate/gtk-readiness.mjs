// Experimental English GTK chooser readiness, scoped to the inspected theme.
// This observes the rendered confirm button; it never changes a Web API/file.
import assert from 'node:assert/strict';
import { setTimeout as delay } from 'node:timers/promises';

export async function waitForGtkConfirm(capture, point) {
  const crop=`70x18+${point.x-35}+${point.y-9}`;
  const deadline=Date.now()+8000;
  let dark=0;
  while(Date.now()<deadline) {
    const pixels=await capture(crop);
    assert(Buffer.isBuffer(pixels)&&pixels.length===70*18*3,'Unexpected GTK button sample');
    dark=0;
    for(let i=0;i<pixels.length;i+=3) {
      if(Math.max(pixels[i],pixels[i+1],pixels[i+2])<110)dark++;
    }
    // Disabled text in the observed GTK theme has no pixels below 110;
    // the enabled Select label has 79. Exclude the button border from the crop.
    if(dark>=40&&dark<=250)return {crop,dark};
    await delay(50);
  }
  throw Error(`GTK confirm button did not become visibly enabled: dark=${dark}`);
}
