//! Standalone Linux browser-foundation experiment; not the product runtime.
//! Lifecycle follows cef-rs cefsimple (see UPSTREAM.md). Remote pages receive
//! no Tauri webview, V8 extension, process-message router or native IPC.

use cef::*;
use std::{
    cell::RefCell,
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

wrap_life_span_handler! {
    struct CandidateLifetime { count: Arc<AtomicUsize>, config: Arc<Configuration> }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let browser = browser.expect("CEF created a browser");
            assert_eq!(browser.host().expect("Native host").runtime_style(), RuntimeStyle::CHROME);
            let context = browser.host().unwrap().request_context().expect("Actual browser context");
            let actual = PathBuf::from(CefString::from(&context.cache_path()).to_string());
            let requested = self.config.case.join("cef-profiles").join(&self.config.request.profile);
            assert_eq!(context.is_global(), 0, "Requested UUID must not use the global context");
            assert_eq!(actual, requested, "Native context must select the requested canonical profile");
            self.config.events.send(serde_json::json!({
                "kind":"profile",
                "profile":self.config.request.profile,"requestedCache":requested,"nativeCache":actual,
                "nativeGlobalContext":false,"browserId":browser.identifier()
            })).unwrap();
            self.count.fetch_add(1, Ordering::SeqCst);
        }
        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if self.count.fetch_sub(1, Ordering::SeqCst) == 1 { quit_message_loop(); }
        }
    }
}

wrap_load_handler! {
    struct CandidateLoads { config: Arc<Configuration> }
    impl LoadHandler {
        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition: TransitionType) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() == 1) {
                self.config.events.send(serde_json::json!({"kind":"load-start","url":CefString::from(&frame.url()).to_string()})).unwrap();
            }
        }
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, status: i32) {
            if let Some(frame) = frame.filter(|frame| frame.is_main() == 1) {
                self.config.events.send(serde_json::json!({"kind":"load-end","url":CefString::from(&frame.url()).to_string(),"status":status})).unwrap();
            }
        }
        fn on_load_error(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, code: Errorcode, error: Option<&CefString>, url: Option<&CefString>) {
            if frame.is_some_and(|frame| frame.is_main() == 1) {
                self.config.events.send(serde_json::json!({"kind":"load-error","code":sys::cef_errorcode_t::from(code) as i32,"error":error.map(ToString::to_string),"url":url.map(ToString::to_string)})).unwrap();
            }
        }
    }
}

wrap_client! {
    struct CandidateClient { count: Arc<AtomicUsize>, config: Arc<Configuration> }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(CandidateLifetime::new(self.count.clone(), self.config.clone()))
        }
        fn load_handler(&self) -> Option<LoadHandler> { Some(CandidateLoads::new(self.config.clone())) }
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
    struct CandidateBrowserProcess { client: Client, config: Arc<Configuration>, pending: LocalRc<RefCell<Option<RequestContext>>> }
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) {
            let mut handler = CandidateContext::new(self.client.clone(), self.config.clone(), self.pending.clone());
            let profile = self.config.case.join("cef-profiles").join(&self.config.request.profile);
            let settings = RequestContextSettings {
                cache_path: CefString::from(profile.to_str().unwrap()),
                ..Default::default()
            };
            let context = request_context_create_context(Some(&settings), Some(&mut handler)).expect("Explicit UUID request context");
            self.pending.replace(Some(context));
        }
        fn default_client(&self) -> Option<Client> { Some(self.client.clone()) }
    }
}

wrap_app! {
    struct CandidateApp { client: Client, config: Arc<Configuration>, pending: LocalRc<RefCell<Option<RequestContext>>> }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(CandidateBrowserProcess::new(self.client.clone(), self.config.clone(), self.pending.clone()))
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
    let request: OpenRequest =
        serde_json::from_slice(&std::fs::read(case.join("request.json")).unwrap()).unwrap();
    request
        .validate()
        .expect("Production App request validation");
    assert!(is_isolated_profile(&request.profile));
    assert!(matches!(
        request.url.as_str(),
        "https://excalidraw.com/" | "https://www.photopea.com/"
    ));
    let port: i32 = std::env::var("TAKOSUMI_CEF_PORT").unwrap().parse().unwrap();
    assert!((1024..65536).contains(&port));
    let root = case.join("cef-profiles");
    assert!(!root.exists(), "A fresh candidate profile is required");
    let profile = root.join(&request.profile);
    std::fs::create_dir_all(&profile).unwrap();
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
    let evidence = std::thread::spawn(move || {
        let mut log = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(evidence_case.join("native-events.jsonl"))
            .unwrap();
        for event in receiver {
            if event["kind"] == "profile" {
                std::fs::write(
                    evidence_case.join("native-profile.json"),
                    serde_json::to_vec_pretty(&event).unwrap(),
                )
                .unwrap();
            }
            writeln!(log, "{}", event).unwrap();
        }
    });
    let config = Arc::new(Configuration {
        case: case.clone(),
        request,
        events,
    });
    let client = CandidateClient::new(Arc::new(AtomicUsize::new(0)), config.clone());
    let pending = LocalRc::new(RefCell::new(None));
    let mut app = CandidateApp::new(client, config.clone(), pending.clone());
    std::fs::write(case.join("native-startup.json"), serde_json::to_vec_pretty(&serde_json::json!({
        "requestValidated":true,"profile":config.request.profile,"url":config.request.url,
        "requestedCache":profile,"rootCache":root,"ipcInstalled":false,"runtime":"CEF Chrome style candidate"
    })).unwrap()).unwrap();
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
    pending.borrow_mut().take();
    shutdown();
    drop(app);
    drop(config);
    evidence.join().unwrap();
}
