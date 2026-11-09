// A real, explicitly supplied Dunst backend in the runner's private session.
// It is a test prerequisite, never an installed App service or a fake daemon.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { open, readFile, writeFile } from 'node:fs/promises';
import { setTimeout as delay } from 'node:timers/promises';
import path from 'node:path';

const name='org.freedesktop.Notifications';
const object='/org/freedesktop/Notifications';
const dbus=(method,...args)=>['call','--session','--dest','org.freedesktop.DBus','--object-path','/org/freedesktop/DBus','--method',`org.freedesktop.DBus.${method}`,...args];

export async function startNotificationBackend({binary,data,nonce,env,desktop}) {
  assert(binary&&path.isAbsolute(binary));
  const title=`SumiNotify-${nonce}`;
  const config=path.join(data,'notification-backend.conf');
  // Keep the daemon's normal tag/replacement handling. One visible item plus
  // FIFO waiting makes both profiles' notifications separately clickable.
  await writeFile(config,`[global]\ntitle = ${title}\nwidth = 420\nheight = (0, 200)\norigin = top-right\noffset = (10, 10)\nnotification_limit = 1\nsort = false\nfont = DejaVu Sans 11\nformat = <b>%s</b>\\n%b\nmouse_left_click = do_action, close_current\n`);
  assert.equal(await desktop('gdbus',dbus('NameHasOwner',name)),'(false,)','Require a fresh notification bus');
  const logfile=await open(path.join(data,'notification-backend.log'),'wx',0o600);
  const daemon=spawn(binary,['-conf',config,'-verbosity','debug','-print'],{env,stdio:['ignore',logfile.fd,logfile.fd]});
  const ended=new Promise((resolve,reject)=>{daemon.once('exit',(code,signal)=>resolve({code,signal}));daemon.once('error',reject)});
  await logfile.close();
  let monitor,monitorEnded;
  const proof={pid:daemon.pid,title,stopped:false,forced:false};
  const service=async(method,...args)=>desktop('gdbus',['call','--session','--dest',name,'--object-path',object,'--method',method,...args]);
  const backend={proof,daemon,title,
    async owner() {
      const pid=await desktop('gdbus',dbus('GetConnectionUnixProcessID',name));
      assert.equal(pid,`(uint32 ${daemon.pid},)`,'Refusing another notification service');
      return pid;
    },
    async clientPid(uniqueName) {
      assert(/^:\d+\.\d+$/.test(uniqueName),'Require an observed unique D-Bus connection');
      const value=await desktop('gdbus',dbus('GetConnectionUnixProcessID',uniqueName));
      const match=/^\(uint32 (\d+),\)$/.exec(value);assert(match,'Missing notification client process');
      return Number(match[1]);
    },
    async count(field) {
      assert(['displayed','waiting','history'].includes(field));
      const text=await service('org.freedesktop.DBus.Properties.Get','org.dunstproject.cmd0',`${field}Length`);
      const match=/^\(<uint32 (\d+)>,\)$/.exec(text);assert(match,`Unexpected Dunst count: ${text}`);
      return Number(match[1]);
    },
    async trace() {return readFile(path.join(data,'notification-bus.log'),'utf8');},
    async stop() {
      proof.prematureExit=daemon.exitCode!==null||daemon.signalCode!==null;
      if(monitor) {
        monitor.kill('SIGTERM');
        let done=await Promise.race([monitorEnded.then(()=>true),delay(4000).then(()=>false)]);
        if(!done){proof.forced=true;monitor.kill('SIGKILL');await monitorEnded;}
      }
      daemon.kill('SIGTERM');
      let done=await Promise.race([ended.then(()=>true),delay(4000).then(()=>false)]);
      if(!done){proof.forced=true;daemon.kill('SIGKILL');}
      proof.exit=await ended;proof.stopped=true;
      await writeFile(path.join(data,'notification-backend-stop.json'),JSON.stringify(proof,null,2)+'\n');
      assert(!proof.prematureExit&&!proof.forced&&proof.exit.code===0,'Notification fixture backend must remain live until its planned, normal stop');
    },
  };
  try {
    const deadline=Date.now()+5000;let ready=false;
    while(Date.now()<deadline&&!ready) {
      assert(daemon.exitCode===null&&daemon.signalCode===null,'Owned notification backend exited during startup');
      ready=(await desktop('gdbus',dbus('NameHasOwner',name)))==='(true,)';
      if(!ready)await delay(50);
    }
    assert(ready,'No private notification service');
    proof.dbusOwner=await backend.owner();
    const serviceName=await desktop('gdbus',dbus('GetNameOwner',name));
    const uniqueName=/^\('(:\d+\.\d+)',\)$/.exec(serviceName);assert(uniqueName,'Missing owned notification service connection');
    proof.uniqueName=uniqueName[1];
    proof.server=await service('org.freedesktop.Notifications.GetServerInformation');
    assert.equal(proof.server,"('dunst', 'knopwob', '1.12.2 (2025-03-05)', '1.2')",'This fixture uses the inspected Dunst 1.12.2 backend');
    proof.capabilities=await service('org.freedesktop.Notifications.GetCapabilities');
    assert(proof.capabilities.includes("'actions'"),'The real backend must support native actions');
    assert.equal(await backend.count('displayed'),0);assert.equal(await backend.count('waiting'),0);
    const buslog=await open(path.join(data,'notification-bus.log'),'wx',0o600);
    monitor=spawn('dbus-monitor',['--session',"interface='org.freedesktop.Notifications'","interface='org.freedesktop.portal.Notification'","interface='org.freedesktop.impl.portal.Notification'",`type='method_return',sender='${proof.uniqueName}'`],{env,stdio:['ignore',buslog.fd,buslog.fd]});
    monitorEnded=new Promise((resolve,reject)=>{monitor.once('exit',resolve);monitor.once('error',reject)});
    await buslog.close();
    const monitorReady=Date.now()+5000;let attached=false;
    while(Date.now()<monitorReady&&!attached) {
      assert(monitor.exitCode===null&&monitor.signalCode===null,'Notification trace exited');
      attached=(await backend.trace()).includes('NameAcquired');
      if(!attached)await delay(50);
    }
    assert(attached,'Private notification trace did not attach');
    return backend;
  }catch(error) {
    try{await backend.stop();}catch(cleanup){error.cleanup=String(cleanup);}
    throw error;
  }
}
