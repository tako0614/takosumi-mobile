//! Opt-in Linux browser service. Its only control input is the parent's pipe.
//! No remote-page IPC/V8 extension or permission handler. Explicit fixture
//! inspection uses a loopback DevTools port, never the parent's control pipe.
use cef::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io::{self, BufReader, Read, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri_plugin_app_screen::{
    browser_process::{decode, read_packet, Reply, Request},
    OpenRequest,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseObservation {
    Idle,
    Pending,
    Ready,
    Failed,
}
struct Screen {
    browser: Browser,
    key: String,
    popup: bool,
    _observer: Registration,
    observation: CloseObservation,
    load_noted: bool,
}
struct State {
    directory: PathBuf,
    profile: String,
    context: Option<RequestContext>,
    boot: Option<Vec<u8>>,
    screens: BTreeMap<i32, Screen>,
    pending_close: BTreeMap<i32, u64>,
    pending: usize,
    live: usize,
    last_id: u64,
    shutdown: Option<u64>,
    failed: bool,
    unload_dialogs: BTreeSet<i32>,
}
thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }

fn with_state<T>(run: impl FnOnce(&mut State) -> T) -> T {
    assert_ne!(currently_on(ThreadId::UI), 0);
    STATE.with(|state| run(state.borrow_mut().as_mut().expect("Owned browser state")))
}
fn reply(value: Reply) {
    let mut out = io::stdout().lock();
    // A broken parent pipe cannot authorize another operation. EOF also posts
    // the owned shutdown task; never print protocol data to the site's console.
    let _ = serde_json::to_writer(&mut out, &value);
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}
fn error(id: u64, message: &str) {
    reply(Reply::Error {
        id,
        message: message.into(),
    });
}
fn maybe_quit() {
    let done = with_state(|state| {
        if state.live == 0 && state.pending == 0 {
            if let Some(id) = state.shutdown {
                reply(Reply::Stopped { id });
            }
            true
        } else {
            false
        }
    });
    if done {
        quit_message_loop();
    }
}
fn close_all(id: Option<u64>, failed: bool) {
    let browsers = with_state(|state| {
        state.failed |= failed;
        state.shutdown = Some(id.unwrap_or(0));
        state
            .screens
            .values()
            .map(|screen| screen.browser.clone())
            .collect::<Vec<_>>()
    });
    // Do not hold a RefCell borrow across native callbacks. Ordinary shutdown
    // honors beforeunload; a lost/malformed parent pipe is an abnormal stop.
    for browser in browsers {
        if failed {
            browser.host().unwrap().close_browser(1);
        } else {
            request_close(browser);
        }
    }
    maybe_quit();
}
fn request_close(browser: Browser) {
    let observation = with_state(|state| {
        state.screens.get_mut(&browser.identifier()).map(|screen| {
            let previous = screen.observation;
            if previous == CloseObservation::Idle {
                screen.observation = CloseObservation::Pending;
            }
            previous
        })
    });
    match observation {
        Some(CloseObservation::Idle) => {
            // Start the private lifecycle agent only for a close request. Starting
            // a Page agent during site initialization is unnecessary and can
            // interfere with a simultaneously attached observation client.
            let queued = browser.host().unwrap().execute_dev_tools_method(
                1,
                Some(&CefString::from("Page.enable")),
                None,
            );
            if queued != 1 {
                observation_result(&browser, false);
            }
        }
        Some(CloseObservation::Ready) => {
            browser.host().unwrap().close_browser(0);
        }
        Some(CloseObservation::Failed) => {
            close_unavailable(browser.identifier());
        }
        _ => {} // Pending setup or an already closed browser never authorizes close.
    }
}
fn close_unavailable(browser_id: i32) {
    let id = with_state(|state| {
        state
            .shutdown
            .take()
            .or_else(|| state.pending_close.remove(&browser_id))
    });
    if let Some(id) = id {
        error(
            id,
            "Native close observation is unavailable; browser retained",
        );
    }
}
fn observation_result(browser: &Browser, success: bool) {
    let pending = with_state(|state| {
        state
            .screens
            .get_mut(&browser.identifier())
            .is_some_and(|screen| {
                if screen.observation != CloseObservation::Pending {
                    return false;
                }
                screen.observation = if success {
                    CloseObservation::Ready
                } else {
                    CloseObservation::Failed
                };
                true
            })
    });
    if !pending {
        return;
    }
    if success {
        browser.host().unwrap().close_browser(0);
    } else {
        close_unavailable(browser.identifier());
    }
}
/// Bring a reused X11 browser window to the front when the home returns to it.
/// Raises it directly (works without a window manager) and asks an EWMH window
/// manager to activate it as a task switcher would. Uses XCB checked requests,
/// so a stale window yields an error value instead of reaching a process-wide
/// Xlib error handler that could exit the browser. libxcb is loaded at run
/// time (libcef already links it); the candidate gains no crate dependency.
mod x11_front {
    use std::ffi::{c_char, c_int, c_uint, c_void};
    extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn free(pointer: *mut c_void);
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Cookie {
        sequence: c_uint,
    }
    #[repr(C)]
    struct ScreenIterator {
        data: *const u32, // xcb_screen_t starts with its root window id.
        rem: c_int,
        index: c_int,
    }
    #[repr(C)]
    struct AtomReply {
        response_type: u8,
        pad: u8,
        sequence: u16,
        length: u32,
        atom: u32,
    }
    #[repr(C)]
    struct ClientMessage {
        response_type: u8,
        format: u8,
        sequence: u16,
        window: u32,
        message_type: u32,
        data: [u32; 5],
    }
    const RTLD_NOW: c_int = 2;
    const CLIENT_MESSAGE: u8 = 33;
    const SUBSTRUCTURE_NOTIFY_AND_REDIRECT: u32 = (1 << 19) | (1 << 20);
    const CONFIG_STACK_MODE: u16 = 64;
    const STACK_ABOVE: u32 = 0;
    const SOURCE_PAGER: u32 = 2;

    pub fn raise(window: u64) -> Result<(), &'static str> {
        let window = u32::try_from(window).map_err(|_| "invalid X window id")?;
        if window == 0 {
            return Err("no native window");
        }
        // SAFETY: symbols come from the system libxcb with their documented C
        // signatures. The connection is private to this call; replies and
        // errors it returns are freed here.
        unsafe {
            let lib = dlopen(c"libxcb.so.1".as_ptr(), RTLD_NOW);
            if lib.is_null() {
                return Err("libxcb unavailable");
            }
            macro_rules! symbol {
                ($name:expr) => {{
                    let found = dlsym(lib, $name.as_ptr());
                    if found.is_null() {
                        return Err("libxcb symbol unavailable");
                    }
                    std::mem::transmute(found)
                }};
            }
            let connect: unsafe extern "C" fn(*const c_char, *mut c_int) -> *mut c_void =
                symbol!(c"xcb_connect");
            let has_error: unsafe extern "C" fn(*mut c_void) -> c_int =
                symbol!(c"xcb_connection_has_error");
            let disconnect: unsafe extern "C" fn(*mut c_void) = symbol!(c"xcb_disconnect");
            let setup: unsafe extern "C" fn(*mut c_void) -> *const c_void =
                symbol!(c"xcb_get_setup");
            let roots: unsafe extern "C" fn(*const c_void) -> ScreenIterator =
                symbol!(c"xcb_setup_roots_iterator");
            let intern: unsafe extern "C" fn(*mut c_void, u8, u16, *const c_char) -> Cookie =
                symbol!(c"xcb_intern_atom");
            let intern_reply: unsafe extern "C" fn(
                *mut c_void,
                Cookie,
                *mut *mut c_void,
            ) -> *mut AtomReply = symbol!(c"xcb_intern_atom_reply");
            let send: unsafe extern "C" fn(*mut c_void, u8, u32, u32, *const c_char) -> Cookie =
                symbol!(c"xcb_send_event_checked");
            let configure: unsafe extern "C" fn(*mut c_void, u32, u16, *const u32) -> Cookie =
                symbol!(c"xcb_configure_window_checked");
            let check: unsafe extern "C" fn(*mut c_void, Cookie) -> *mut c_void =
                symbol!(c"xcb_request_check");
            let connection = connect(std::ptr::null(), std::ptr::null_mut());
            if connection.is_null() || has_error(connection) != 0 {
                if !connection.is_null() {
                    disconnect(connection);
                }
                return Err("X display unavailable");
            }
            let finish = |result| {
                disconnect(connection);
                result
            };
            let screen = roots(setup(connection));
            if screen.data.is_null() || screen.rem < 1 {
                return finish(Err("no X screen"));
            }
            let root = *screen.data;
            let name = c"_NET_ACTIVE_WINDOW";
            let mut error = std::ptr::null_mut();
            let reply = intern_reply(
                connection,
                intern(connection, 0, name.to_bytes().len() as u16, name.as_ptr()),
                &mut error,
            );
            if !error.is_null() {
                free(error);
            }
            if reply.is_null() {
                return finish(Err("EWMH atom unavailable"));
            }
            let active = (*reply).atom;
            free(reply.cast());
            let event = ClientMessage {
                response_type: CLIENT_MESSAGE,
                format: 32,
                sequence: 0,
                window,
                message_type: active,
                data: [SOURCE_PAGER, 0, 0, 0, 0],
            };
            let sent = check(
                connection,
                send(
                    connection,
                    0,
                    root,
                    SUBSTRUCTURE_NOTIFY_AND_REDIRECT,
                    (&event as *const ClientMessage).cast(),
                ),
            );
            let raised = check(
                connection,
                configure(connection, window, CONFIG_STACK_MODE, &STACK_ABOVE),
            );
            let failed = !sent.is_null() || !raised.is_null();
            for problem in [sent, raised] {
                if !problem.is_null() {
                    free(problem);
                }
            }
            finish(if failed {
                Err("X window activation or raise was rejected")
            } else {
                Ok(())
            })
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        #[ignore = "needs an X display; run under xvfb-run with --ignored"]
        fn a_stale_window_is_an_error_value_not_a_process_exit() {
            assert!(
                std::env::var_os("DISPLAY").is_some(),
                "run under an X display"
            );
            assert_eq!(super::raise(0), Err("no native window"));
            // An id no client owns: the checked raise must report, not exit.
            assert_eq!(
                super::raise(0x3ff_fff0),
                Err("X window activation or raise was rejected")
            );
        }
    }
}

fn opened(id: u64, key: String, browser: &Browser) {
    let context = browser.host().unwrap().request_context().unwrap();
    let actual = PathBuf::from(CefString::from(&context.cache_path()).to_string());
    let (profile, expected) = with_state(|state| {
        (
            state.profile.clone(),
            state.directory.join("cef-profiles").join(&state.profile),
        )
    });
    assert_eq!(context.is_global(), 0);
    assert_eq!(actual, expected);
    reply(Reply::Opened {
        id,
        key,
        profile,
        browser_id: browser.identifier(),
        cache_path: actual.to_str().unwrap().into(),
        window_id: browser.host().unwrap().window_handle(),
        native_pid: std::process::id(),
    });
}

wrap_life_span_handler! {
    struct Lifetime { id: u64, key: String, bound: bool }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let browser=browser.expect("Native browser");
            let popup=browser.is_popup()!=0;
            let host=browser.host().unwrap();
            let mut observer=DialogEvents::new(browser.identifier());
            let registration=host.add_dev_tools_message_observer(Some(&mut observer)).expect("Owned browser lifecycle observer");
            with_state(|state| {
                state.live+=1;
                if self.bound {
                    if !popup { state.pending-=1; }
                    assert!(state.screens.insert(browser.identifier(),Screen{browser:browser.clone(),key:self.key.clone(),popup,_observer:registration.clone(),observation:CloseObservation::Idle,load_noted:false}).is_none());
                }
            });
            if !self.bound { browser.host().unwrap().close_browser(1);return; }
            let (stopping,failed,over_limit)=with_state(|state|(state.shutdown.is_some(),state.failed,state.screens.len()>48));
            if stopping||over_limit { browser.host().unwrap().close_browser(i32::from(failed||over_limit));return; }
            if !popup { opened(self.id,self.key.clone(),browser); }
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            let browser=browser.expect("Closing native browser");
            let id=browser.identifier();
            with_state(|state| {
                let screen=state.screens.remove(&id);
                if let Some(screen)=screen.filter(|screen|!screen.popup) {
                    reply(Reply::Closed{id:state.pending_close.remove(&id).unwrap_or(0),key:screen.key,
                        profile:state.profile.clone(),browser_id:id});
                }
                state.live-=1;
            });
            maybe_quit();
        }
    }
}
wrap_dev_tools_message_observer! {
    struct DialogEvents { browser_id: i32 }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, browser: Option<&mut Browser>, message_id:i32, success:i32, _result:Option<&[u8]>) {
            if message_id!=1{return;}
            let browser=browser.expect("Native observation result");
            assert_eq!(browser.identifier(),self.browser_id);
            observation_result(browser,success==1);
        }
        fn on_dev_tools_event(&self, browser: Option<&mut Browser>, method: Option<&CefString>, params: Option<&[u8]>) {
            let Some(method)=method else {return;};
            let method=method.to_string();
            if method!="Page.javascriptDialogOpening" && method!="Page.javascriptDialogClosed" {return;}
            assert_eq!(browser.expect("Native lifecycle browser").identifier(),self.browser_id);
            // Dialog text is supplied by the remote page. Never panic or
            // allocate unbounded JSON from an oversized alert/confirm event.
            let Some(bytes)=params.filter(|bytes|bytes.len()<=8192) else {return;};
            let Ok(params)=serde_json::from_slice::<serde_json::Value>(bytes) else {return;};
            with_state(|state| {
                if method=="Page.javascriptDialogOpening" && params["type"]=="beforeunload" {
                    state.unload_dialogs.insert(self.browser_id);
                } else if method=="Page.javascriptDialogClosed" && state.unload_dialogs.remove(&self.browser_id) && params["result"]==false {
                    let id=state.shutdown.take().or_else(||state.pending_close.remove(&self.browser_id));
                    if let Some(id)=id { reply(Reply::Cancelled{id,profile:state.profile.clone()}); }
                }
            });
        }
    }
}
wrap_client! {
    struct ClientHandler { id: u64, key: String, bound: bool }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(Lifetime::new(self.id,self.key.clone(),self.bound)) }
        fn load_handler(&self) -> Option<LoadHandler> {Some(Loads::new())}
    }
}
wrap_load_handler! {
    struct Loads;
    impl LoadHandler {
        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _http_status_code:i32) {
            let (Some(browser),Some(frame))=(browser,frame) else {return;};
            if frame.is_main()==0{return;}
            let note=with_state(|state|state.screens.get_mut(&browser.identifier()).is_some_and(|screen|{
                if screen.load_noted {false} else {screen.load_noted=true;true}
            }));
            if note && std::env::var_os("TAKOSUMI_BROWSER_PROCESS_INSPECT").as_deref()==Some(std::ffi::OsStr::new("1")) {
                let directory=with_state(|state|state.directory.clone());
                std::fs::write(directory.join(format!("load-ready-{}.json",browser.identifier())),serde_json::to_vec(
                    &serde_json::json!({"pid":std::process::id(),"browserId":browser.identifier(),"mainFrame":true})).unwrap()).unwrap();
            }
        }
    }
}

fn open(id: u64, payload: OpenRequest) {
    let existing = with_state(|state| {
        state
            .screens
            .values()
            .find(|screen| !screen.popup && screen.key == payload.key)
            .map(|screen| screen.browser.clone())
    });
    if let Some(browser) = existing {
        if !payload.resume {
            browser
                .main_frame()
                .unwrap()
                .load_url(Some(&CefString::from(payload.url.as_str())));
        }
        let host = browser.host().unwrap();
        host.set_focus(1);
        // set_focus only moves keyboard focus inside the browser. A window
        // covered by another profile stays hidden unless it is raised.
        if let Err(problem) = x11_front::raise(host.window_handle() as u64) {
            eprintln!("Reused browser window was not raised: {problem}");
        }
        opened(id, payload.key, &browser);
        return;
    }
    let context = with_state(|state| {
        if state.screens.len() + state.pending >= 48 {
            return None;
        }
        state.pending += 1;
        state.context.clone()
    });
    let Some(mut context) = context else {
        error(id, "Browser screen limit or context unavailable");
        return;
    };
    let mut client = ClientHandler::new(id, payload.key, true);
    let info = WindowInfo {
        runtime_style: RuntimeStyle::CHROME,
        ..Default::default()
    };
    if browser_host_create_browser(
        Some(&info),
        Some(&mut client),
        Some(&CefString::from(payload.url.as_str())),
        Some(&BrowserSettings::default()),
        None,
        Some(&mut context),
    ) != 1
    {
        with_state(|state| state.pending -= 1);
        error(id, "Native browser creation rejected");
    }
}
fn dispatch(bytes: Option<&[u8]>) {
    let Some(bytes) = bytes else {
        if with_state(|state| state.shutdown.is_none()) {
            close_all(None, true);
        }
        return;
    };
    let profile = with_state(|state| state.profile.clone());
    let request = match decode(bytes, &profile) {
        Ok(request) => request,
        Err(_) => {
            error(0, "Invalid browser command");
            close_all(None, true);
            return;
        }
    };
    let id = request.id();
    let allowed = with_state(|state| {
        if id <= state.last_id || state.shutdown.is_some() {
            return false;
        }
        state.last_id = id;
        true
    });
    if !allowed {
        error(id, "Stale command or browser stopping");
        return;
    }
    match request {
        Request::Open { id, payload } => open(id, payload),
        Request::Close { id, key } => {
            let browser = with_state(|state| {
                let found = state
                    .screens
                    .values()
                    .find(|screen| !screen.popup && screen.key == key)
                    .map(|screen| screen.browser.clone());
                if let Some(browser) = &found {
                    state.pending_close.insert(browser.identifier(), id);
                }
                found
            });
            if let Some(browser) = browser {
                request_close(browser);
            } else {
                error(id, "No live browser screen");
            }
        }
        Request::Shutdown { id } => {
            // Chrome-style CEF exposes no reversible close preparation. A
            // later window's cancellation must not destroy an earlier one.
            if with_state(|state| state.screens.len() + state.pending > 1) {
                reply(Reply::CloseBlocked {
                    id,
                    profile: with_state(|state| state.profile.clone()),
                });
            } else {
                close_all(Some(id), false);
            }
        }
    }
}
wrap_task! {
    struct PipeTask { bytes: Option<Vec<u8>> }
    impl Task { fn execute(&self) { dispatch(self.bytes.as_deref()); } }
}
wrap_request_context_handler! {
    struct ContextHandler;
    impl RequestContextHandler {
        fn on_request_context_initialized(&self, context: Option<&mut RequestContext>) {
            let context=context.expect("Profile context");
            let mut path=value_create().unwrap();
            let downloads=with_state(|state|state.directory.join("Downloads"));
            path.set_string(Some(&CefString::from(downloads.to_str().unwrap())));
            let mut diagnostic=CefString::from("");
            assert_eq!(context.set_preference(Some(&CefString::from("download.default_directory")),Some(&mut path),Some(&mut diagnostic)),1);
            let (boot,stopping)=with_state(|state|{state.context=Some(context.clone());state.pending-=1;(state.boot.take().unwrap(),state.shutdown.is_some())});
            if stopping { maybe_quit(); } else { dispatch(Some(&boot)); }
        }
    }
}
wrap_browser_process_handler! {
    struct ProcessHandler;
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            let path=with_state(|state|state.directory.join("cef-profiles").join(&state.profile));
            let settings=RequestContextSettings{cache_path:CefString::from(path.to_str().unwrap()),..Default::default()};
            let mut handler=ContextHandler::new();
            let context=request_context_create_context(Some(&settings),Some(&mut handler)).unwrap();
            with_state(|state|state.context=Some(context));
        }
        fn default_client(&self) -> Option<Client> { Some(ClientHandler::new(0,String::new(),false)) }
    }
}
wrap_app! {
    struct Application;
    impl App { fn browser_process_handler(&self) -> Option<BrowserProcessHandler> { Some(ProcessHandler::new()) } }
}

fn main() {
    const {
        assert!(cfg!(target_os = "linux"));
    }
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let status = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if status >= 0 {
        std::process::exit(status);
    }
    let directory = PathBuf::from(
        std::env::var_os("TAKOSUMI_BROWSER_PROCESS_DIRECTORY").expect("Owned parent directory"),
    );
    assert!(directory.is_absolute() && directory.canonicalize().unwrap() == directory);
    let mut boot = Vec::new();
    std::fs::File::open(directory.join("boot.json"))
        .unwrap()
        .take(tauri_plugin_app_screen::browser_process::MAX_PACKET_BYTES as u64 + 1)
        .read_to_end(&mut boot)
        .unwrap();
    let input: serde_json::Value = serde_json::from_slice(&boot).unwrap();
    let profile = input["payload"]["profile"].as_str().unwrap().to_string();
    assert!(matches!(
        decode(&boot, &profile).unwrap(),
        Request::Open { .. }
    ));
    let cache = directory.join("cef-profiles");
    std::fs::create_dir_all(cache.join(&profile)).unwrap();
    assert!(directory.join("Downloads").is_dir());
    STATE.with(|state| {
        state.replace(Some(State {
            directory: directory.clone(),
            profile,
            context: None,
            boot: Some(boot),
            screens: BTreeMap::new(),
            pending_close: BTreeMap::new(),
            pending: 1,
            live: 0,
            last_id: 0,
            shutdown: None,
            failed: false,
            unload_dialogs: BTreeSet::new(),
        }))
    });
    let inspection = std::env::var_os("TAKOSUMI_BROWSER_PROCESS_INSPECT").as_deref()
        == Some(std::ffi::OsStr::new("1"));
    let port = if inspection {
        // The small bind-to-start race can only fail this isolated probe.
        // The runner additionally binds targets to the attested native PID.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port() as i32
    } else {
        0
    };
    if inspection {
        std::fs::write(
            directory.join("inspection.json"),
            serde_json::to_vec(&serde_json::json!({"pid":std::process::id(),"port":port})).unwrap(),
        )
        .unwrap();
    }
    let settings = Settings {
        no_sandbox: 1,
        root_cache_path: CefString::from(cache.to_str().unwrap()),
        log_file: CefString::from(directory.join("cef-native.log").to_str().unwrap()),
        remote_debugging_port: port,
        ..Default::default()
    };
    let mut app = Application::new();
    assert_eq!(
        initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut()
        ),
        1
    );
    let active = Arc::new((Mutex::new(()), AtomicBool::new(true)));
    let gate = active.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(io::stdin());
        loop {
            let bytes = read_packet(&mut reader).ok().flatten();
            let stop = bytes.is_none();
            let _guard = gate.0.lock().unwrap();
            if !gate.1.load(Ordering::Acquire) {
                break;
            }
            let mut task = PipeTask::new(bytes);
            if post_task(ThreadId::UI, Some(&mut task)) != 1 || stop {
                break;
            }
        }
    });
    run_message_loop();
    {
        let _guard = active.0.lock().unwrap();
        active.1.store(false, Ordering::Release);
    }
    let failed = STATE.with(|state| {
        let mut state = state.borrow_mut().take().unwrap();
        assert!(state.screens.is_empty() && state.live == 0 && state.pending == 0);
        state.context.take();
        state.failed
    });
    drop(app);
    shutdown();
    if failed {
        std::process::exit(1);
    }
}
