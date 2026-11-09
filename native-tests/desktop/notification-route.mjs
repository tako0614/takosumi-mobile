// Correlate the real notification protocol's Notify reply and native action.
// Parse only this private dbus-monitor trace and controlled literal title.
import assert from 'node:assert/strict';

const blocks=trace=>trace.split(/(?=^(?:method call|method return|signal|error) time=)/m)
  .map(text=>text.trim().split('\n')).filter(lines=>lines[0]);
const field=(header,name)=>new RegExp(`(?:^| )${name}=([^ ;]+)`).exec(header)?.[1];
const uint=body=>{
  const match=/^uint32 (\d+)$/.exec(body.trim());
  assert(match,'Expected one native uint32');
  const value=Number(match[1]);assert(Number.isSafeInteger(value)&&value>0&&value<=0xffffffff);
  return value;
};
export function notificationRoute(trace,title,service) {
  assert(/^:\d+\.\d+$/.test(service));
  assert(/^[A-Za-z0-9 _.\-]+$/.test(title),'Require a controlled unescaped fixture title');
  const messages=blocks(trace);
  const calls=messages.filter(lines=>lines[0].startsWith('method call ')&&
    lines[0].endsWith('interface=org.freedesktop.Notifications; member=Notify')&&
    lines[4]?.trim()===`string "${title}"`);
  assert(calls.length<=1,'Ambiguous matching Notify calls');
  if(!calls.length)return null;
  assert.equal(calls[0][2]?.trim(),'uint32 0','A new fixture notification must not replace an existing ID');
  const call=calls[0][0],sender=field(call,'sender'),serial=field(call,'serial');
  assert(/^:\d+\.\d+$/.test(sender??'')&&/^\d+$/.test(serial??''));
  assert.equal(field(call,'destination'),'org.freedesktop.Notifications');
  const replies=messages.filter(lines=>lines[0].startsWith('method return ')&&field(lines[0],'sender')===service&&
    field(lines[0],'destination')===sender&&field(lines[0],'reply_serial')===serial);
  assert(replies.length<=1,'Ambiguous Notify replies');
  if(!replies.length)return null;
  assert.equal(replies[0].length,2,'Unexpected Notify reply payload');
  return {service,sender,serial,id:uint(replies[0][1])};
}
export function notificationActions(trace) {
  return blocks(trace).filter(lines=>lines[0].startsWith('signal ')&&
    lines[0].endsWith('interface=org.freedesktop.Notifications; member=ActionInvoked')).map(lines=>{
    assert.equal(lines.length,3,'Unexpected native action payload');
    assert.equal(lines[2].trim(),'string "default"','Expected the native default action');
    return {service:field(lines[0],'sender'),sender:field(lines[0],'destination'),id:uint(lines[1])};
  });
}
export function requireNotificationAction(action,route) {
  assert.deepEqual(action,{service:route.service,sender:route.sender,id:route.id},'Native action must belong to the exact Personal Notify reply');
}
