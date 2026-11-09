// Protocol-corruption controls; these do not qualify native notifications.
import assert from 'node:assert/strict';
import test from 'node:test';
import { notificationRoute,notificationActions,requireNotificationAction } from './notification-route.mjs';

const title='Closing Personal owned-case',service=':1.1';
const call=`method call time=1 sender=:1.22 -> destination=org.freedesktop.Notifications serial=33 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify
   string "Chromium"
   uint32 0
   string "owned icon"
   string "${title}"
   string "owned body"
   array [
   ]
   array [
   ]
   int32 0
`;
const reply=`method return time=2 sender=:1.1 -> destination=:1.22 serial=54 reply_serial=33
   uint32 4
`;
const action=`signal time=3 sender=:1.1 -> destination=:1.22 serial=55 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=ActionInvoked
   uint32 4
   string "default"
`;
test('bind Notify call, exact reply and native action',()=>{
  const trace=call+reply+action,route=notificationRoute(trace,title,service);
  assert.deepEqual(route,{service,sender:':1.22',serial:'33',id:4});
  requireNotificationAction(notificationActions(trace)[0],route);
});
test('missing, wrong peer and wrong call serial cannot supply the ID',()=>{
  for(const trace of [call,reply,call+reply.replace('sender=:1.1 ','sender=:1.2 '),
    call+reply.replace('destination=:1.22','destination=:1.19'),call+reply.replace('reply_serial=33','reply_serial=34')]) {
    assert.equal(notificationRoute(trace,title,service),null);
  }
});
test('same title cannot hide ambiguous or malformed notification replies',()=>{
  for(const trace of [call+call+reply,call+reply+reply,call+reply.replace('uint32 4','uint32 0'),
    call+reply.replace('uint32 4','uint32 4294967296'),call.replace('uint32 0','uint32 2')+reply]) {
    assert.throws(()=>notificationRoute(trace,title,service));
  }
});
test('an action for a replacement alert or another browser cannot pass',()=>{
  const route=notificationRoute(call+reply,title,service);
  for(const changed of [action.replace('uint32 4','uint32 5'),action.replace('destination=:1.22','destination=:1.19'),
    action.replace('sender=:1.1 ','sender=:1.2 ')]) {
    assert.throws(()=>requireNotificationAction(notificationActions(changed)[0],route),/exact Personal Notify reply/);
  }
});
test('unexpected action payload or a nondefault action is rejected',()=>{
  for(const changed of [action.replace('"default"','"settings"'),action+'   uint32 9\n']) {
    assert.throws(()=>notificationActions(changed));
  }
});
