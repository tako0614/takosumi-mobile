//! Opt-in browser service for Linux and (first candidate) Windows. Its only
//! control input is the parent's pipe.
//! No remote-page IPC/V8 extension or permission handler. Explicit fixture
//! inspection (`fixture-inspection` builds only) uses a loopback DevTools
//! port, never the parent's control pipe.
use cef::*;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io::{self, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};
use tauri_plugin_app_screen::{
    browser_process::{decode, external_link, link_host, read_packet, Reply, Request},
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
    /// The window an ordinary shutdown is closing now; the chain continues only after it.
    chain: Option<i32>,
    failed: bool,
    unload_dialogs: BTreeSet<i32>,
}
thread_local! { static STATE: RefCell<Option<State>> = const { RefCell::new(None) }; }
/// Whether the control pipe's reader may still post a request to the UI
/// thread. Closed before the message loop is asked to quit, under the same
/// lock the reader holds while it posts, so nothing is posted afterwards.
static CONTROL_OPEN: Mutex<bool> = Mutex::new(true);
fn close_control() {
    *CONTROL_OPEN.lock().unwrap_or_else(PoisonError::into_inner) = false;
}

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
        // The parent closes its end of the pipe once it reads `stopped`; the
        // end of input must not reach the UI thread after the loop ends.
        close_control();
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
    // honors beforeunload one window at a time (oldest first; on_before_close
    // continues with the next); a lost/malformed parent pipe is an abnormal stop.
    if failed {
        for browser in browsers {
            browser.host().unwrap().close_browser(1);
        }
    } else if let Some(browser) = browsers.into_iter().next() {
        with_state(|state| state.chain = Some(browser.identifier()));
        request_close(browser);
    }
    maybe_quit();
}
/// During an ordinary shutdown, after the window the chain targeted has
/// closed, the next (oldest) window still open. Windows the user or a popup
/// closes meanwhile do not advance the chain or re-ask the target.
fn next_to_close(closed: i32) -> Option<Browser> {
    with_state(|state| {
        if state.shutdown.is_none() || state.failed || state.chain != Some(closed) {
            return None;
        }
        let next = state
            .screens
            .values()
            .next()
            .map(|screen| screen.browser.clone());
        state.chain = next.as_ref().map(|browser| browser.identifier());
        next
    })
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
#[cfg(target_os = "linux")]
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

/// Windows browser windows. The browser's native handle may be a child of
/// its frame; both the raise and the `window_id` reported to the App use the
/// frame, the top-level window (`GA_ROOT`) the person sees and switches to.
/// Bringing a reused window to the front restores it when minimized, shows it
/// and asks for the foreground. Windows hands the foreground only to a
/// process allowed to take it (for example one the foreground App passed that
/// right with `AllowSetForegroundWindow`); otherwise it flashes the taskbar
/// button, and the refusal is reported like an X11 one. A stale handle makes
/// the calls fail, never crash. user32 is already linked by the CEF wrapper;
/// the candidate gains no crate dependency.
#[cfg(windows)]
mod win32_front {
    use std::ffi::{c_int, c_uint, c_void};
    type Hwnd = *mut c_void;
    #[link(name = "user32")]
    extern "system" {
        fn IsWindow(window: Hwnd) -> c_int;
        fn GetAncestor(window: Hwnd, flags: c_uint) -> Hwnd;
        fn IsIconic(window: Hwnd) -> c_int;
        fn ShowWindow(window: Hwnd, command: c_int) -> c_int;
        fn SetForegroundWindow(window: Hwnd) -> c_int;
    }
    const GA_ROOT: c_uint = 2;
    const SW_SHOW: c_int = 5;
    const SW_RESTORE: c_int = 9;

    /// The top-level window of a browser handle (itself when it has none).
    pub fn top_level(window: Hwnd) -> Hwnd {
        if window.is_null() {
            return window;
        }
        // SAFETY: documented user32 signature; a stale handle yields null.
        let root = unsafe { GetAncestor(window, GA_ROOT) };
        if root.is_null() {
            window
        } else {
            root
        }
    }

    pub fn raise(top: Hwnd) -> Result<(), &'static str> {
        if top.is_null() {
            return Err("no native window");
        }
        // SAFETY: documented user32 signatures; every call accepts any
        // handle value and reports a stale one through its result.
        unsafe {
            if IsWindow(top) == 0 {
                return Err("stale native window");
            }
            // The return value is the previous visibility, not a failure.
            ShowWindow(
                top,
                if IsIconic(top) != 0 {
                    SW_RESTORE
                } else {
                    SW_SHOW
                },
            );
            if SetForegroundWindow(top) == 0 {
                return Err("Windows refused the foreground change");
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn raise(host: &BrowserHost) -> Result<(), &'static str> {
    x11_front::raise(host.window_handle() as u64)
}
#[cfg(windows)]
fn raise(host: &BrowserHost) -> Result<(), &'static str> {
    win32_front::raise(win32_front::top_level(host.window_handle().0.cast()))
}

/// The native window id reported to the App: the X11 window CEF reports on
/// Linux; on Windows the value of the top-level HWND the raise brings forward.
#[cfg(target_os = "linux")]
fn window_id(host: &BrowserHost) -> u64 {
    host.window_handle()
}
#[cfg(windows)]
fn window_id(host: &BrowserHost) -> u64 {
    win32_front::top_level(host.window_handle().0.cast()) as usize as u64
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
    #[cfg(target_os = "linux")]
    assert_eq!(actual, expected);
    // Exactly the requested spelling, not merely the same components.
    #[cfg(windows)]
    assert_eq!(actual.as_os_str(), expected.as_os_str());
    reply(Reply::Opened {
        id,
        key,
        profile,
        browser_id: browser.identifier(),
        cache_path: actual.to_str().unwrap().into(),
        window_id: window_id(&browser.host().unwrap()),
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
            // A cancel clears the shutdown, so the remaining windows stay open.
            if let Some(next)=next_to_close(id) { request_close(next); }
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
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {Some(LinkMenu::new())}
    }
}
/// `MENU_ID_USER_FIRST` in cef_types.h.
const OPEN_EXTERNALLY: i32 = 26500;
// The person may choose to open a web link outside the App (policy: external
// links stay in the profile unless the person chooses otherwise). The browser
// runs with its profile's HOME and XDG directories, so it only hands the link
// to the App, which opens it with the person's own desktop.
wrap_context_menu_handler! {
    struct LinkMenu;
    impl ContextMenuHandler {
        fn on_before_context_menu(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, params: Option<&mut ContextMenuParams>, model: Option<&mut MenuModel>) {
            let (Some(params),Some(model))=(params,model) else {return;};
            // Name the destination host (ASCII/punycode): a page can cover the
            // screen with a link or change it at the last moment.
            let url=CefString::from(&params.link_url()).to_string();
            if let Some(host)=link_host(&url) {
                model.insert_item_at(0,OPEN_EXTERNALLY,Some(&CefString::from(format!("既定のブラウザで開く: {host}").as_str())));
                model.insert_separator_at(1);
            }
        }
        fn on_context_menu_command(&self, _browser: Option<&mut Browser>, _frame: Option<&mut Frame>, params: Option<&mut ContextMenuParams>, command_id: ::std::os::raw::c_int, _event_flags: EventFlags) -> ::std::os::raw::c_int {
            if command_id!=OPEN_EXTERNALLY {return 0;}
            let Some(params)=params else {return 1;};
            let url=CefString::from(&params.link_url()).to_string();
            if external_link(&url) {
                reply(Reply::OpenExternal{profile:with_state(|state|state.profile.clone()),url});
            }
            1
        }
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
            if note && inspection() {
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
        if let Err(problem) = raise(&host) {
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
    // A request the reader posted just before the control pipe closed can
    // still run while CEF shuts down, after the browser state is gone (on
    // Windows the parent's end of input arrived while the closing window
    // handed over the foreground). Nothing is left to do then. A panic here
    // would abort the browser: CEF task callbacks cannot unwind, and the
    // process ends with 0xC0000409 instead of exit 0.
    if STATE.with(|state| state.borrow().is_none()) {
        eprintln!("A control request arrived after the browser stopped; ignored.");
        return;
    }
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
            // Chrome-style CEF exposes no reversible close preparation, so
            // windows close one at a time and a beforeunload cancel stops the
            // rest (the owner's sequential-quit decision). A window still being
            // created cannot take part yet.
            if with_state(|state| state.pending > 0) {
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

/// The native fixture's observation seam: only a browser built with this
/// crate's `fixture-inspection` feature honours
/// `TAKOSUMI_BROWSER_PROCESS_INSPECT=1`. A product browser has no DevTools
/// port and writes no observation files, whatever its environment says.
#[cfg(feature = "fixture-inspection")]
fn inspection() -> bool {
    std::env::var_os("TAKOSUMI_BROWSER_PROCESS_INSPECT").as_deref()
        == Some(std::ffi::OsStr::new("1"))
}
#[cfg(not(feature = "fixture-inspection"))]
fn inspection() -> bool {
    false
}

/// A loopback DevTools port for the fixture's observer, recorded with this
/// process's pid; 0 (no port) otherwise.
#[cfg(feature = "fixture-inspection")]
fn inspection_port(directory: &std::path::Path) -> i32 {
    if !inspection() {
        return 0;
    }
    // The small bind-to-start race can only fail this isolated probe.
    // The runner additionally binds targets to the attested native PID.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port() as i32;
    std::fs::write(
        directory.join("inspection.json"),
        serde_json::to_vec(&serde_json::json!({"pid":std::process::id(),"port":port})).unwrap(),
    )
    .unwrap();
    port
}
#[cfg(not(feature = "fixture-inspection"))]
fn inspection_port(_directory: &std::path::Path) -> i32 {
    0
}

/// Chromium's sandbox runs unless the App started this browser with
/// `--no-sandbox` (the App is root, or the fixture opted out). Chromium also
/// refuses its sandbox to root, so root never asks for it. CEF's own setting
/// follows the same decision as the switch.
#[cfg(target_os = "linux")]
fn without_sandbox(mut args: impl Iterator<Item = std::ffi::OsString>) -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let root = unsafe { libc::geteuid() } == 0;
    root || args.any(|arg| arg == "--no-sandbox")
}
/// Windows (first candidate): the browser always runs without Chromium's
/// sandbox, whatever its switches. CEF's Windows sandbox needs the
/// `cef_sandbox` library (cef-dll-sys `sandbox`) and its bootstrap layout: the
/// browser built as a DLL that CEF's `bootstrap.exe`, renamed to the browser,
/// loads, with sandbox information passed to `execute_process` and
/// `initialize`. This build links none of that and passes no sandbox
/// information, so CEF is told so explicitly. Such a build must be asked for
/// by name (the feature `windows-unsandboxed-candidate`), so an unsandboxed
/// browser is never built, and staged, by accident.
/// TODO(TASK-0119 Windows): qualify the bootstrap/cef_sandbox layout and
/// enable the sandbox before any Windows package.
#[cfg(windows)]
fn without_sandbox(_args: impl Iterator<Item = std::ffi::OsString>) -> bool {
    true
}
#[cfg(all(windows, not(feature = "windows-unsandboxed-candidate")))]
compile_error!(
    "The Windows browser runs without Chromium's sandbox. Build it only as a test candidate, \
     with the feature windows-unsandboxed-candidate; it is not for packages."
);

/// The parent passes the directory in its final form, so the cache path CEF
/// reports back compares exactly with the one requested.
#[cfg(target_os = "linux")]
fn is_final(directory: &Path) -> bool {
    directory.canonicalize().unwrap() == directory
}
/// On Windows `canonicalize` spells the final path in its verbatim form
/// (`\\?\C:\…` or `\\?\UNC\server\share\…`); the parent and CEF use the
/// ordinary spelling of the same path (`C:\…`, `\\server\share\…`). After
/// that prefix the spelling must match exactly: `Path` equality would accept
/// another drive-letter case, `/` separators, repeated or trailing
/// separators. Short (8.3) names, links and letter case are resolved by
/// `canonicalize`, so the parent passes the long, final spelling.
#[cfg(windows)]
fn is_final(directory: &Path) -> bool {
    let verbatim = directory.canonicalize().unwrap();
    let text = verbatim.to_str().expect("UTF-8 browser directory");
    let ordinary = if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else if let Some(disk) = text
        .strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
    {
        disk.to_string()
    } else {
        text.to_string()
    };
    std::ffi::OsStr::new(&ordinary) == directory.as_os_str()
}

fn main() {
    const {
        assert!(cfg!(any(target_os = "linux", windows)));
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
    assert!(directory.is_absolute() && is_final(&directory));
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
            chain: None,
            failed: false,
            unload_dialogs: BTreeSet::new(),
        }))
    });
    let port = inspection_port(&directory);
    let settings = Settings {
        no_sandbox: i32::from(without_sandbox(std::env::args_os().skip(1))),
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
    std::thread::spawn(move || {
        let mut reader = BufReader::new(io::stdin());
        loop {
            let bytes = read_packet(&mut reader).ok().flatten();
            let stop = bytes.is_none();
            let open = CONTROL_OPEN.lock().unwrap_or_else(PoisonError::into_inner);
            if !*open {
                break;
            }
            let mut task = PipeTask::new(bytes);
            if post_task(ThreadId::UI, Some(&mut task)) != 1 || stop {
                break;
            }
        }
    });
    run_message_loop();
    close_control();
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
