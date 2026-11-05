//! Standalone Linux browser-foundation experiment; not the product runtime.
//! Lifecycle follows cef-rs cefsimple (see UPSTREAM.md). Remote pages receive
//! no Tauri webview, V8 extension, process-message router or native IPC.

use cef::*;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::Write,
    path::PathBuf,
    rc::Rc as LocalRc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc,
    },
};
use tauri_plugin_app_screen::{is_isolated_profile, OpenRequest};

struct Configuration {
    case: PathBuf,
    request: OpenRequest,
    events: mpsc::Sender<serde_json::Value>,
}

type Observers = LocalRc<RefCell<BTreeMap<i32, (Registration, bool)>>>;

#[derive(Clone)]
struct Launch {
    client: Client,
    config: Arc<Configuration>,
    pending: LocalRc<RefCell<Option<RequestContext>>>,
}

wrap_dev_tools_message_observer! {
    struct CandidateTarget { config: Arc<Configuration>, browser_id: i32 }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(&self, browser: Option<&mut Browser>, message_id: i32, success: i32, result: Option<&[u8]>) {
            assert_ne!(currently_on(ThreadId::UI), 0);
            assert!([1, 2].contains(&message_id));
            assert_eq!(browser.expect("Native result browser").identifier(), self.browser_id);
            let raw = result.expect("Target lookup result");
            assert!(raw.len() <= 131072, "Unexpected native metadata size");
            let result: serde_json::Value = serde_json::from_slice(raw).unwrap();
            // CEF's private inspection agent may identify itself as `other`.
            // Preserve it as a diagnostic; identify the actual view by frame ID.
            self.config.events.send(serde_json::json!({"kind":"devtools","browserId":self.browser_id,"messageId":message_id,"success":success,"result":result})).unwrap();
            if success != 1 { return; }
            if message_id == 1 {
                if let Some(target) = result.get("targetInfo") {
                    self.config.events.send(serde_json::json!({"kind":"target","browserId":self.browser_id,"targetInfo":target})).unwrap();
                }
            } else if let Some(frame) = result.get("frameTree").and_then(|t| t.get("frame")) {
                assert!(frame["id"].as_str().is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())));
                self.config.events.send(serde_json::json!({"kind":"frame","browserId":self.browser_id,"frame":frame})).unwrap();
            }
        }
    }
}

wrap_life_span_handler! {
    struct CandidateLifetime { count: Arc<AtomicUsize>, queued_roots: Arc<AtomicUsize>, config: Arc<Configuration>, observers: Observers }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let browser = browser.expect("CEF created a browser");
            assert_ne!(currently_on(ThreadId::UI), 0);
            assert_eq!(browser.host().expect("Native host").runtime_style(), RuntimeStyle::CHROME);
            let context = browser.host().unwrap().request_context().expect("Actual browser context");
            let actual = PathBuf::from(CefString::from(&context.cache_path()).to_string());
            let requested = self.config.case.join("cef-profiles").join(&self.config.request.profile);
            assert_eq!(context.is_global(), 0, "Requested UUID must not use the global context");
            assert_eq!(actual, requested, "Native context must select the requested canonical profile");
            self.config.events.send(serde_json::json!({
                "kind":"profile",
                "profile":self.config.request.profile,"requestedCache":requested,"nativeCache":actual,
                "nativeGlobalContext":false,"browserId":browser.identifier(),"popup":browser.is_popup() != 0
            })).unwrap();
            self.count.fetch_add(1, Ordering::SeqCst);
            if browser.is_popup() == 0 {
                assert!(self.queued_roots.fetch_sub(1, Ordering::SeqCst) > 0);
            }
            let host = browser.host().unwrap();
            let mut observer = CandidateTarget::new(self.config.clone(), browser.identifier());
            let registration = host.add_dev_tools_message_observer(Some(&mut observer)).expect("Native target observer registration");
            assert!(self.observers.borrow_mut().insert(browser.identifier(), (registration, false)).is_none());
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            assert_ne!(currently_on(ThreadId::UI), 0);
            let id = browser.expect("Closing native browser").identifier();
            self.config.events.send(serde_json::json!({"kind":"closed","browserId":id,"profile":self.config.request.profile})).unwrap();
            let registration = self.observers.borrow_mut().remove(&id);
            drop(registration);
            if self.count.fetch_sub(1, Ordering::SeqCst) == 1 && self.queued_roots.load(Ordering::SeqCst) == 0 { quit_message_loop(); }
        }
    }
}

wrap_life_span_handler! {
    struct UnboundLifetime { count: Arc<AtomicUsize>, queued_roots: Arc<AtomicUsize>, events: mpsc::Sender<serde_json::Value> }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let browser = browser.expect("Unbound Chrome UI browser");
            self.count.fetch_add(1, Ordering::SeqCst);
            self.events.send(serde_json::json!({"kind":"unbound-ui-window","browserId":browser.identifier()})).unwrap();
            browser.host().unwrap().close_browser(1);
        }
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if self.count.fetch_sub(1, Ordering::SeqCst) == 1 && self.queued_roots.load(Ordering::SeqCst) == 0 { quit_message_loop(); }
        }
    }
}

wrap_client! {
    struct UnboundClient { count: Arc<AtomicUsize>, queued_roots: Arc<AtomicUsize>, events: mpsc::Sender<serde_json::Value> }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(UnboundLifetime::new(self.count.clone(), self.queued_roots.clone(), self.events.clone()))
        }
    }
}

wrap_load_handler! {
    struct CandidateLoads { config: Arc<Configuration>, observers: Observers }
    impl LoadHandler {
        fn on_load_start(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition: TransitionType) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() == 1) {
                let browser = browser.expect("Main-frame native browser");
                self.config.events.send(serde_json::json!({"kind":"load-start","browserId":browser.identifier(),"frameId":CefString::from(&frame.identifier()).to_string(),"url":CefString::from(&frame.url()).to_string()})).unwrap();
                let query = {
                    let mut observers = self.observers.borrow_mut();
                    let entry = observers.get_mut(&browser.identifier()).expect("Created browser observer");
                    let query = !entry.1;
                    entry.1 = true;
                    query
                };
                if query {
                    let host = browser.host().unwrap();
                    assert_eq!(host.execute_dev_tools_method(1, Some(&CefString::from("Target.getTargetInfo")), None), 1);
                    assert_eq!(host.execute_dev_tools_method(2, Some(&CefString::from("Page.getFrameTree")), None), 2);
                }
            }
        }
        fn on_load_end(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, status: i32) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() == 1) {
                self.config.events.send(serde_json::json!({"kind":"load-end","browserId":browser.map(|b| b.identifier()),"frameId":CefString::from(&frame.identifier()).to_string(),"url":CefString::from(&frame.url()).to_string(),"status":status})).unwrap();
            }
        }
        fn on_load_error(&self, browser: Option<&mut Browser>, frame: Option<&mut Frame>, code: Errorcode, error: Option<&CefString>, url: Option<&CefString>) {
            if frame.is_some_and(|frame| frame.is_main() == 1) {
                self.config.events.send(serde_json::json!({"kind":"load-error","browserId":browser.map(|b| b.identifier()),"code":sys::cef_errorcode_t::from(code) as i32,"error":error.map(ToString::to_string),"url":url.map(ToString::to_string)})).unwrap();
            }
        }
    }
}

wrap_client! {
    struct CandidateClient { count: Arc<AtomicUsize>, queued_roots: Arc<AtomicUsize>, config: Arc<Configuration>, observers: Observers }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(CandidateLifetime::new(self.count.clone(), self.queued_roots.clone(), self.config.clone(), self.observers.clone()))
        }
        fn load_handler(&self) -> Option<LoadHandler> { Some(CandidateLoads::new(self.config.clone(), self.observers.clone())) }
    }
}

wrap_request_context_handler! {
    struct CandidateContext { client: Client, config: Arc<Configuration>, pending: LocalRc<RefCell<Option<RequestContext>>> }
    impl RequestContextHandler {
        fn on_request_context_initialized(&self, request_context: Option<&mut RequestContext>) {
            assert_ne!(currently_on(ThreadId::UI), 0);
            let context = request_context.expect("Initialized custom request context");
            let mut destination = value_create().expect("CEF preference value");
            assert_eq!(destination.set_string(Some(&CefString::from(self.config.case.join("Downloads").to_str().unwrap()))), 1);
            // CEF requires a writable out-parameter; default() is a null string.
            let mut error = CefString::from("");
            assert_eq!(context.set_preference(Some(&CefString::from("download.default_directory")), Some(&mut destination), Some(&mut error)), 1, "{}", error);
            let preference = context.preference(Some(&CefString::from("download.default_directory"))).expect("Native download preference");
            assert_eq!(PathBuf::from(CefString::from(&preference.string()).to_string()), self.config.case.join("Downloads"));
            let info = WindowInfo { runtime_style: RuntimeStyle::CHROME, ..Default::default() };
            let mut client = self.client.clone();
            assert_eq!(browser_host_create_browser(Some(&info), Some(&mut client), Some(&CefString::from(self.config.request.url.as_str())), Some(&BrowserSettings::default()), None, Some(context)), 1);
            self.pending.borrow_mut().take();
        }
    }
}

wrap_browser_process_handler! {
    struct CandidateBrowserProcess { launches: Vec<Launch>, unbound: Client }
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            for launch in &self.launches {
                let mut handler = CandidateContext::new(launch.client.clone(), launch.config.clone(), launch.pending.clone());
                let profile = launch.config.case.join("cef-profiles").join(&launch.config.request.profile);
                let settings = RequestContextSettings {
                    cache_path: CefString::from(profile.to_str().unwrap()),
                    ..Default::default()
                };
                let context = request_context_create_context(Some(&settings), Some(&mut handler)).expect("Explicit UUID request context");
                launch.pending.replace(Some(context));
            }
        }
        fn default_client(&self) -> Option<Client> {
            // Null would create an unmanaged Chrome UI browser that blocks
            // shutdown. An unknown profile instead gets a close-only client.
            Some(if self.launches.len() == 1 { self.launches[0].client.clone() } else { self.unbound.clone() })
        }
    }
}

wrap_app! {
    struct CandidateApp { launches: Vec<Launch>, unbound: Client }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(CandidateBrowserProcess::new(self.launches.clone(), self.unbound.clone()))
        }
    }
}

fn main() {
    const { assert!(cfg!(target_os = "linux"), "This candidate is Linux only") };
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let status = execute_process(Some(args.as_main_args()), None, std::ptr::null_mut());
    if status >= 0 {
        std::process::exit(status);
    }
    assert_eq!(status, -1);

    let case = PathBuf::from(
        std::env::var_os("TAKOSUMI_CEF_CASE").expect("Use the owned candidate runner"),
    );
    assert!(case.is_absolute() && case.canonicalize().unwrap() == case);
    let nonce = std::fs::read_to_string(case.join(".owned-browser-reference")).unwrap();
    assert!(
        is_isolated_profile(nonce.trim()),
        "Owned case marker must be a UUID"
    );
    let input: serde_json::Value =
        serde_json::from_slice(&std::fs::read(case.join("request.json")).unwrap()).unwrap();
    let requests: Vec<OpenRequest> = if input.is_array() {
        serde_json::from_value(input).unwrap()
    } else {
        vec![serde_json::from_value(input).unwrap()]
    };
    assert!((1..=2).contains(&requests.len()));
    for request in &requests {
        request
            .validate()
            .expect("Production App request validation");
        assert!(is_isolated_profile(&request.profile));
        assert!(matches!(
            request.url.as_str(),
            "https://excalidraw.com/" | "https://www.photopea.com/"
        ));
    }
    if requests.len() == 2 {
        assert_ne!(requests[0].profile, requests[1].profile);
        assert_eq!(
            requests[0].url, requests[1].url,
            "Same-origin native profile control"
        );
    }
    let port: i32 = std::env::var("TAKOSUMI_CEF_PORT").unwrap().parse().unwrap();
    assert!((1024..65536).contains(&port));
    let root = case.join("cef-profiles");
    assert!(!root.exists(), "A fresh candidate profile is required");
    for request in &requests {
        std::fs::create_dir_all(root.join(&request.profile)).unwrap();
    }
    assert!(
        case.join("Downloads").is_dir(),
        "Owned Downloads must exist before CEF starts"
    );
    let settings = Settings {
        no_sandbox: 1, // Root-only isolated test; no sandbox/release qualification.
        root_cache_path: CefString::from(root.to_str().unwrap()),
        remote_debugging_port: port,
        log_file: CefString::from(case.join("cef-native.log").to_str().unwrap()),
        accept_language_list: CefString::from("en-US"),
        ..Default::default()
    };
    let (events, receiver) = mpsc::channel::<serde_json::Value>();
    let evidence_case = case.clone();
    let expected_roots = requests.len();
    let evidence = std::thread::spawn(move || {
        let mut log = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(evidence_case.join("native-events.jsonl"))
            .unwrap();
        let mut browsers = BTreeMap::<i64, serde_json::Value>::new();
        let mut roots_ready = false;
        for event in receiver {
            if event["kind"] == "profile" {
                let id = event["browserId"].as_i64().unwrap();
                assert!(browsers.insert(id, event.clone()).is_none());
                if expected_roots == 1 && event["popup"] == false {
                    write_snapshot(&evidence_case, "native-profile.json", &event);
                }
            } else if event["kind"] == "closed" {
                let id = event["browserId"].as_i64().unwrap();
                write_snapshot(
                    &evidence_case,
                    &format!("native-closed-{id}.json"),
                    &serde_json::json!({"event":event,"binding":browsers.get(&id)}),
                );
            } else if event["kind"] == "devtools" {
                let id = event["browserId"].as_i64().unwrap();
                let method = event["messageId"].as_i64().unwrap();
                write_snapshot(
                    &evidence_case,
                    &format!("native-devtools-{id}-{method}.json"),
                    &event,
                );
            } else if event["kind"] == "target" || event["kind"] == "frame" {
                let id = event["browserId"].as_i64().unwrap();
                let binding = browsers
                    .get_mut(&id)
                    .expect("Profile precedes native target");
                let field = if event["kind"] == "target" {
                    "targetInfo"
                } else {
                    "frame"
                };
                binding[field] = event[field].clone();
                if let Some(frame) = binding.get("frame") {
                    let frame = frame["id"].as_str().unwrap();
                    write_snapshot(
                        &evidence_case,
                        &format!("native-frame-{frame}.json"),
                        binding,
                    );
                }
                let roots: Vec<_> = browsers
                    .values()
                    .filter(|b| b["popup"] == false && b.get("frame").is_some())
                    .collect();
                if !roots_ready && roots.len() == expected_roots {
                    write_snapshot(
                        &evidence_case,
                        "native-roots-ready.json",
                        &serde_json::to_value(roots).unwrap(),
                    );
                    roots_ready = true;
                }
            }
            writeln!(log, "{}", event).unwrap();
            if event["kind"] == "runtime" && event["stage"] == "clients-dropped" {
                // SDK shutdown has returned and all managed browsers closed.
                // Retained Sender handles must not keep the recorder alive.
                write_snapshot(&evidence_case, "native-runtime-complete.json", &event);
                break;
            }
        }
    });
    let count = Arc::new(AtomicUsize::new(0));
    let queued_roots = Arc::new(AtomicUsize::new(requests.len()));
    let observers = LocalRc::new(RefCell::new(BTreeMap::new()));
    let lifecycle = events.clone();
    std::fs::write(
        case.join("native-startup.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "requestValidated":true,"requests":requests,
            "rootCache":root,"ipcInstalled":false,"runtime":"CEF Chrome style candidate"
        }))
        .unwrap(),
    )
    .unwrap();
    let launches: Vec<_> = requests
        .into_iter()
        .map(|request| {
            let config = Arc::new(Configuration {
                case: case.clone(),
                request,
                events: events.clone(),
            });
            let client = CandidateClient::new(
                count.clone(),
                queued_roots.clone(),
                config.clone(),
                observers.clone(),
            );
            Launch {
                client,
                config,
                pending: LocalRc::new(RefCell::new(None)),
            }
        })
        .collect();
    let unbound = UnboundClient::new(count, queued_roots, events);
    let mut app = CandidateApp::new(launches.clone(), unbound);
    assert_eq!(
        initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut()
        ),
        1
    );
    run_message_loop();
    lifecycle
        .send(serde_json::json!({"kind":"runtime","stage":"loop-returned"}))
        .unwrap();
    for launch in &launches {
        launch.pending.borrow_mut().take();
    }
    assert!(
        observers.borrow().is_empty(),
        "All native observers must close before shutdown"
    );
    lifecycle
        .send(serde_json::json!({"kind":"runtime","stage":"shutdown-start"}))
        .unwrap();
    shutdown();
    lifecycle
        .send(serde_json::json!({"kind":"runtime","stage":"shutdown-finished"}))
        .unwrap();
    drop(app);
    drop(launches);
    lifecycle
        .send(serde_json::json!({"kind":"runtime","stage":"clients-dropped"}))
        .unwrap();
    drop(lifecycle);
    evidence.join().unwrap();
}

fn write_snapshot(case: &std::path::Path, name: &str, value: &serde_json::Value) {
    let pending = case.join(format!("{name}.pending"));
    std::fs::write(&pending, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    std::fs::rename(pending, case.join(name)).unwrap();
}
