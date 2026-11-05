//! Executable actual-engine fixture, deliberately excluded from normal builds.
//! Uses production raw-view construction and navigation actions, with synthetic
//! HTML assigned an https base URI. Never loads a real service or user profile.

use super::*;
use gtk::glib::prelude::{Cast, ObjectExt};
use javascriptcore::ValueExt;
use std::{path::Path, time::Duration};
use webkit2gtk::{ResponsePolicyDecision, ResponsePolicyDecisionExt, WebContextExt, WebViewExt};
use wry::WebViewExtUnix;

static EXIT_STATUS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
static RESET_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PENDING_CLOSE_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PENDING_CLOSE_VALID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const WORK: &str = "11111111-1111-4111-8111-111111111111";
const PERSONAL: &str = "22222222-2222-4222-8222-222222222222";

pub fn run() {
    let directory = std::env::var_os("TAKOSUMI_FIXTURE_DATA")
        .map(PathBuf::from)
        .expect("TAKOSUMI_FIXTURE_DATA must name an isolated, disposable directory");
    assert!(
        directory.is_absolute(),
        "Fixture directory must be absolute"
    );
    let readback = std::env::args().any(|value| value == "--readback");
    let spa = std::env::args().any(|value| value == "--spa-resume");
    assert!(
        !std::env::args().any(|value| value == "--profile-reset") || (!spa && !readback),
        "Profile reset uses a fresh, single-process fixture"
    );
    assert!(
        !std::env::args().any(|value| value == "--missing-document") || readback,
        "Missing-document control requires readback mode"
    );
    if !readback {
        assert!(
            !directory.exists(),
            "First run requires a new fixture directory"
        );
    } else {
        assert!(directory.is_dir(), "Readback requires the first run's data");
    }
    let app = tauri::Builder::default()
        .plugin(init())
        .setup(move |app| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Synthetic Takosumi picker")
            .build()?;
            let certificate =
                std::fs::read_to_string(std::env::var_os("TAKOSUMI_FIXTURE_CERT").unwrap())?;
            let certificate = gtk::gio::TlsCertificate::from_pem(&certificate)?;
            let profiles: &[&str] = if spa {
                &[WORK]
            } else {
                &[WORK, PERSONAL, "shared"]
            };
            for &profile in profiles {
                if spa && readback {
                    let payload = request(profile);
                    let expected = spa_saved_target();
                    let stored = ResumeStore::new(directory.join(profile), payload.key).recall()?;
                    assert_eq!(stored.as_deref(), Some(expected.as_str()),
                        "SPA readback must start with the saved successful route, before constructing a view");
                    println!("PASS: saved SPA route matches before native view construction");
                }
                open_on_ui_thread(app.handle(), request(profile), directory.clone())?;
                let view = SCREENS.with(|screens| {
                    screens
                        .borrow()
                        .get(&label(profile))
                        .unwrap()
                        .view
                        .webview()
                });
                // Only this fixture's ephemeral loopback certificate. Load
                // again after trusting it, then await the real bootstrap
                // document before replacing any factory-started navigation.
                view.context()
                    .unwrap()
                    .allow_tls_certificate_for_host(&certificate, "127.0.0.1");
                // SPA readback must finish the factory's saved-route load.
                // Reloading here can interrupt it before its URI settles.
                // Trust is set in the same setup turn after construction;
                // do not force the expected route or start a second request.
                if !(spa && readback) {
                    view.load_uri(&request(profile).url);
                }
            }
            let handle = app.handle().clone();
            gtk::glib::idle_add_local_once(move || {
                if spa {
                    spa_probe(handle, readback);
                } else {
                    bootstrap(handle, directory, readback, 0);
                }
            });
            Ok(())
        })
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("Fixture application builds");
    let watchdog = app.handle().clone();
    gtk::glib::timeout_add_local_once(Duration::from_secs(45), move || {
        eprintln!("FAIL: fixture watchdog expired");
        watchdog.exit(1);
    });
    let _ = app.run_return(|_, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            SCREENS.with(|screens| screens.borrow_mut().clear());
            CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
        }
    });
    // Wry 2.11's Linux RequestExit path sets ControlFlow::Exit without the
    // requested code. run_return lets us propagate an explicit assertion
    // result instead of accidentally reporting a failed observation as zero.
    std::process::exit(EXIT_STATUS.load(Ordering::SeqCst));
}

fn request(profile: &str) -> OpenRequest {
    let base =
        std::env::var("TAKOSUMI_FIXTURE_HTTPS").expect("Run via native-tests/desktop/run.py");
    let url = tauri::Url::parse(&base).expect("Fixture HTTPS URL");
    assert!(
        url.scheme() == "https"
            && url.host_str() == Some("127.0.0.1")
            && url.port().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "Fixture must use the runner's loopback HTTPS root"
    );
    let origin = url.origin().ascii_serialization();
    let payload: OpenRequest = serde_json::from_value(serde_json::json!({
        "key": if profile == "shared" { format!("web-home:{origin}") }
            else { format!("web-home:{profile}:{origin}") },
        "url": url.as_str(), "title": "Fixture", "profile": profile,
        "webHome": true, "resume": true
    }))
    .unwrap();
    payload
        .validate()
        .expect("Fixture uses a valid production request");
    payload
}

fn recovery_target(path: &str) -> String {
    let method = if std::env::args().any(|arg| arg == "--spa-replace") { "replace" } else { "push" };
    format!("{}{path}?case={method}", request(WORK).url)
}

fn spa_saved_target() -> String {
    if std::env::args().any(|arg| arg == "--cancelled-spa") {
        recovery_target("slow")
    } else if std::env::args().any(|arg| arg == "--error-page-back") {
        format!("{}document/after-error?view=edit", request(WORK).url)
    } else if std::env::args().any(|arg| arg == "--error-page-retry") {
        recovery_target("retry")
    } else {
        format!("{}document/work?view=edit", request(WORK).url)
    }
}

fn spa_probe(app: AppHandle, readback: bool) {
    let view = SCREENS.with(|screens| screens.borrow().get(&label(WORK)).unwrap().view.webview());
    let home = request(WORK).url;
    let target = format!("{home}document/work?view=edit");
    let method = if std::env::args().any(|value| value == "--spa-replace") {
        "replaceState"
    } else {
        "pushState"
    };
    let expected = if readback { spa_saved_target() } else { home };
    let probe = view.clone();
    after_document_load(
        &view,
        expected,
        "bootstrap".into(),
        app.clone(),
        move || {
            if readback {
                if std::env::args().any(|arg| arg == "--cancelled-spa") {
                    println!("PASS: new process restored the legitimate SPA at a previously cancelled URL");
                } else if std::env::args().any(|arg| arg == "--error-page-retry") {
                    println!("PASS: new process restored the successfully retried URL");
                } else if std::env::args().any(|arg| arg == "--error-page-back") {
                    println!("PASS: new process restored the SPA route after native cached Back");
                } else {
                    println!("PASS: new process restored native {method} route without a link click");
                }
                EXIT_STATUS.store(0, Ordering::SeqCst);
                app.exit(0);
                return;
            }
            let script = format!(
                "history.{method}(null, '', '/document/work?view=edit'); location.href === {}",
                serde_json::to_string(&target).unwrap()
            );
            probe.evaluate_javascript(
                &script,
                None,
                None,
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    if !result.is_ok_and(|value| value.to_boolean()) {
                        eprintln!("FAIL: controlled SPA route did not change");
                        app.exit(1);
                        return;
                    }
                    println!("PASS: {method} changed the current native page; closing immediately");
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        if std::env::args().any(|arg| {
                            arg == "--failed-navigation" || arg == "--cancelled-navigation"
                                || arg == "--pending-navigation"
                        }) {
                            fail_after_spa(handle, target);
                        } else {
                            close_spa(handle);
                        }
                    });
                },
            );
        },
    );
}

fn close_spa(app: AppHandle) {
    let screen_label = label(WORK);
    let (window, context) = SCREENS.with(|screens| {
        let screens = screens.borrow();
        let screen = screens.get(&screen_label).unwrap();
        let context = screen.view.webview().context().unwrap().downgrade();
        (screen.window.clone(), context)
    });
    if let Err(error) = window.close() {
        eprintln!("FAIL: native close: {error}");
        app.exit(1);
        return;
    }
    after_spa_close(
        app,
        screen_label,
        std::time::Instant::now() + Duration::from_secs(5),
        context,
    );
}

fn fail_after_spa(app: AppHandle, target: String) {
    let view = SCREENS.with(|screens| screens.borrow().get(&label(WORK)).unwrap().view.webview());
    let native = view.clone();
    after_document_load(&view, target, "bootstrap".into(), app.clone(), move || {
        if std::env::args().any(|arg| arg == "--pending-navigation") {
            let pending_uri = format!("{}slow", request(WORK).url);
            let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let terminal = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let finished_armed = armed.clone();
            let finished_terminal = terminal.clone();
            native.connect_load_changed(move |_, event| {
                if matches!(event, webkit2gtk::LoadEvent::Finished)
                    && finished_armed.load(Ordering::SeqCst)
                {
                    finished_terminal.store(true, Ordering::SeqCst);
                }
            });
            let failed_armed = armed.clone();
            let failed_terminal = terminal.clone();
            let failed_uri = pending_uri.clone();
            native.connect_load_failed(move |_, _, uri, _| {
                if failed_armed.load(Ordering::SeqCst) && uri == failed_uri {
                    failed_terminal.store(true, Ordering::SeqCst);
                }
                false
            });
            let close_label = label(WORK);
            let close_uri = pending_uri.clone();
            let window = SCREENS.with(|screens| screens.borrow().get(&close_label).unwrap().window.clone());
            window.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::CloseRequested { .. })
                    && !PENDING_CLOSE_SEEN.swap(true, Ordering::SeqCst)
                {
                    let valid = !terminal.load(Ordering::SeqCst) && SCREENS.with(|screens| {
                        screens.borrow().get(&close_label).is_some_and(|screen| {
                            let view = screen.view.webview();
                            view.is_loading() && view.uri().as_deref() == Some(close_uri.as_str())
                        })
                    });
                    PENDING_CLOSE_VALID.store(valid, Ordering::SeqCst);
                    if valid {
                        println!("PASS: controlled native load is still pending at CloseRequested");
                    } else {
                        eprintln!("FAIL: pending control settled before native CloseRequested");
                    }
                }
            });
            let expected = pending_uri.clone();
            let id = Rc::new(RefCell::new(None));
            let callback_id = id.clone();
            let handle = app.clone();
            let handler = native.connect_load_changed(move |view, event| {
                if matches!(event, webkit2gtk::LoadEvent::Started)
                    && view.uri().as_deref() == Some(expected.as_str())
                {
                    if let Some(id) = callback_id.borrow_mut().take() {
                        view.disconnect(id);
                    }
                    armed.store(true, Ordering::SeqCst);
                    let app = handle.clone();
                    gtk::glib::idle_add_local_once(move || close_spa(app));
                }
            });
            *id.borrow_mut() = Some(handler);
            native.load_uri(&pending_uri);
            return;
        }
        let cancel = std::env::args().any(|arg| arg == "--cancelled-navigation");
        let failed_uri = if std::env::args().any(|arg| arg == "--cancelled-spa") {
            recovery_target("slow")
        } else if std::env::args().any(|arg| arg == "--error-page-retry") {
            recovery_target("retry")
        } else { format!(
            "{}{}",
            request(WORK).url,
            if cancel { "slow" } else { "failed" }
        ) };
        let expected = failed_uri.clone();
        let id = Rc::new(RefCell::new(None));
        let callback_id = id.clone();
        let handle = app.clone();
        let handler = native.connect_load_failed(move |view, _, uri, error| {
            if uri == expected {
                if let Some(id) = callback_id.borrow_mut().take() {
                    view.disconnect(id);
                }
                if error.matches(webkit2gtk::NetworkError::Cancelled) != cancel {
                    eprintln!("FAIL: wrong controlled failure: {error:?}");
                    handle.exit(1);
                    return false;
                }
                println!(
                    "PASS: controlled native {} observed before Close",
                    if cancel {
                        "cancellation"
                    } else {
                        "network failure"
                    }
                );
                let app = handle.clone();
                if std::env::args().any(|arg| arg == "--cancelled-spa") {
                    let weak = view.downgrade();
                    gtk::glib::idle_add_local_once(move || {
                        if let Some(view) = weak.upgrade() {
                            if arm_synthetic_recovery(&app) {
                                change_spa_and_close(app, &view, spa_saved_target(), "PASS: surviving document changed to the previously cancelled URL");
                            }
                        } else {
                            eprintln!("FAIL: cancelled document disappeared before SPA recovery");
                            app.exit(1);
                        }
                    });
                } else if std::env::args().any(|arg| arg == "--error-page-close") {
                    close_after_error_document(app, view, uri.to_owned());
                } else {
                    gtk::glib::idle_add_local_once(move || close_spa(app));
                }
            }
            false
        });
        *id.borrow_mut() = Some(handler);
        if cancel {
            let id = Rc::new(RefCell::new(None));
            let callback_id = id.clone();
            let expected = failed_uri.clone();
            let handler = native.connect_load_changed(move |view, event| {
                if matches!(event, webkit2gtk::LoadEvent::Started)
                    && view.uri().as_deref() == Some(expected.as_str())
                {
                    if let Some(id) = callback_id.borrow_mut().take() {
                        view.disconnect(id);
                    }
                    let view = view.clone();
                    gtk::glib::idle_add_local_once(move || view.stop_loading());
                }
            });
            *id.borrow_mut() = Some(handler);
        }
        native.load_uri(&failed_uri);
    });
}

fn close_after_error_document(app: AppHandle, view: &webkit2gtk::WebView, failed_uri: String) {
    let committed = Rc::new(std::cell::Cell::new(false));
    let done = Rc::new(std::cell::Cell::new(false));
    let signal = Rc::new(RefCell::new(None));
    let signal_id = signal.clone();
    let completed = done.clone();
    let handle = app.clone();
    let id = view.connect_load_changed(move |view, event| {
        if view.uri().as_deref() != Some(failed_uri.as_str()) {
            return;
        }
        if matches!(event, webkit2gtk::LoadEvent::Committed) {
            committed.set(true);
        }
        if matches!(event, webkit2gtk::LoadEvent::Finished) && committed.get()
            && !completed.replace(true)
        {
            if let Some(id) = signal_id.borrow_mut().take() {
                view.disconnect(id);
            }
            println!("PASS: stock error document committed and finished before native Close");
            let app = handle.clone();
            let target = failed_uri.clone();
            let weak = view.downgrade();
            gtk::glib::idle_add_local_once(move || {
                if let Some(view) = weak.upgrade() {
                    after_error_document(app, &view, target);
                } else {
                    eprintln!("FAIL: error document disappeared before recovery/Close");
                    app.exit(1);
                }
            });
        }
    });
    *signal.borrow_mut() = Some(id);
    gtk::glib::timeout_add_local_once(Duration::from_secs(5), move || {
        if !done.get() {
            eprintln!("FAIL: stock error document did not finish before Close control");
            app.exit(1);
        }
    });
}

fn after_error_document(app: AppHandle, view: &webkit2gtk::WebView, failed_uri: String) {
    if std::env::args().any(|arg| arg == "--error-page-retry") {
        if !arm_synthetic_recovery(&app) {
            return;
        }
        let complete = app.clone();
        after_document_load(view, failed_uri, "bootstrap".into(), app, move || {
            println!("PASS: native Reload completed at the previously failed URL");
            close_spa(complete);
        });
        view.reload();
    } else if std::env::args().any(|arg| arg == "--error-page-back") {
        let responses = Rc::new(std::cell::Cell::new(0u32));
        let observed = responses.clone();
        let policy = view.connect_decide_policy(move |_, decision, _| {
            if decision.downcast_ref::<ResponsePolicyDecision>().is_some_and(|response| response.is_main_frame_main_resource()) {
                observed.set(observed.get() + 1);
            }
            false
        });
        let probe = view.clone();
        let expected = format!("{}document/work?view=edit", request(WORK).url);
        let handle = app.clone();
        after_document_load(view, expected, "bootstrap".into(), handle, move || {
            probe.disconnect(policy);
            if responses.get() != 0 {
                eprintln!("FAIL: Back requested a new main response instead of exercising the cached document");
                app.exit(1);
                return;
            }
            println!("PASS: native Back restored a cached document before a later SPA change");
            change_spa_and_close(app, &probe, spa_saved_target(), "PASS: cached Back accepts the next same-document route");
        });
        view.go_back();
    } else {
        close_spa(app);
    }
}

fn arm_synthetic_recovery(app: &AppHandle) -> bool {
    // Arm only the owned synthetic server; never write resume metadata.
    let gate = PathBuf::from(std::env::var_os("TAKOSUMI_FIXTURE_DATA").unwrap()).join("native-recovery-ready");
    if let Err(error) = std::fs::write(gate, "ready") {
        eprintln!("FAIL: could not arm the synthetic network recovery: {error}");
        app.exit(1);
        false
    } else {
        true
    }
}

fn change_spa_and_close(app: AppHandle, view: &webkit2gtk::WebView, target: String, marker: &'static str) {
    let url = serde_json::to_string(&target).unwrap();
    let script = format!("(() => {{ if (document.querySelector('meta[name=sumi-fixture]')?.content !== 'bootstrap') return false; history.replaceState(null, '', {url}); return location.href === {url}; }})()");
    view.evaluate_javascript(&script, None, None, None::<&gtk::gio::Cancellable>, move |result| {
        if !result.is_ok_and(|value| value.to_boolean()) {
            eprintln!("FAIL: surviving document could not change its same-document route");
            app.exit(1);
            return;
        }
        println!("{marker}");
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || close_spa(handle));
    });
}

fn after_spa_close(
    app: AppHandle,
    label: String,
    deadline: std::time::Instant,
    context: gtk::glib::WeakRef<webkit2gtk::WebContext>,
) {
    let closed = !SCREENS.with(|screens| screens.borrow().contains_key(&label));
    if closed {
        // Only WORK is opened in this case. Drop its production cache after
        // ordinary Close while the UI loop is alive; no helper can hide a
        // view/manager cycle behind process exit.
        CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
    }
    if closed && context.upgrade().is_none() {
        if std::env::args().any(|arg| arg == "--pending-navigation")
            && !(PENDING_CLOSE_SEEN.load(Ordering::SeqCst) && PENDING_CLOSE_VALID.load(Ordering::SeqCst))
        {
            eprintln!("FAIL: pending control did not prove its native Close boundary");
            app.exit(1);
            return;
        }
        println!("PASS: primary window and native context released before process exit");
        EXIT_STATUS.store(0, Ordering::SeqCst);
        app.exit(0);
    } else if std::time::Instant::now() >= deadline {
        eprintln!("FAIL: primary window or native context retained after ordinary Close");
        app.exit(1);
    } else {
        gtk::glib::timeout_add_local_once(Duration::from_millis(10), move || {
            after_spa_close(app, label, deadline, context)
        });
    }
}

fn bootstrap(app: AppHandle, directory: PathBuf, readback: bool, index: usize) {
    let Some(profile) = [WORK, PERSONAL, "shared"].get(index).copied() else {
        step(app, directory, readback, 0);
        return;
    };
    let view = SCREENS.with(|screens| {
        screens
            .borrow()
            .get(&label(profile))
            .unwrap()
            .view
            .webview()
    });
    after_document_load(
        &view,
        request(profile).url,
        "bootstrap".into(),
        app.clone(),
        move || {
            println!("PASS: factory bootstrap finished for profile={profile}");
            bootstrap(app, directory, readback, index + 1);
        },
    );
}

fn label(profile: &str) -> String {
    SCREENS.with(|screens| {
        screens
            .borrow()
            .iter()
            .find(|(_, screen)| screen.profile == profile && !screen.key.is_empty())
            .map(|(label, _)| label.clone())
            .expect("Fixture root screen exists")
    })
}

fn step(app: AppHandle, directory: PathBuf, readback: bool, index: usize) {
    let reset = std::env::args().any(|value| value == "--profile-reset");
    let mut operations = if RESET_DONE.load(Ordering::SeqCst) {
        vec![
            (WORK, "sumi", "", ""),
            (PERSONAL, "sumi", "personal", ""),
            (WORK, "other", "", ""),
            ("shared", "sumi", "legacy", ""),
        ]
    } else if readback {
        vec![
            (WORK, "sumi", "work", ""),
            (PERSONAL, "sumi", "personal", ""),
            (WORK, "other", "other", ""),
            ("shared", "sumi", "legacy", ""),
        ]
    } else {
        vec![
            (WORK, "sumi", "", "work"),
            (PERSONAL, "sumi", "", "personal"),
            (WORK, "other", "", "other"),
            (WORK, "sumi", "work", ""),
            ("shared", "sumi", "", "legacy"),
            (PERSONAL, "sumi", "personal", ""),
        ]
    };
    if reset && !RESET_DONE.load(Ordering::SeqCst) {
        // The write snapshot alone cannot establish that OTHER-site data
        // existed before deletion. Read it once before starting the clear.
        operations.push((WORK, "other", "other", ""));
    }
    if index == operations.len() {
        if reset {
            if RESET_DONE.load(Ordering::SeqCst) {
                println!("PASS: completed native profile reset removes both sites and preserves personal/shared profiles");
                EXIT_STATUS.store(0, Ordering::SeqCst);
                app.exit(0);
            } else {
                reset_profile_probe(app, directory);
            }
            return;
        }
        match lifecycle(&app, &directory) {
            Ok(()) => {
                println!("Cookie/storage observations passed: {index}, readback={readback}");
                title_and_back_probe(app);
            }
            Err(error) => {
                eprintln!("FAIL: {error}");
                app.exit(1);
            }
        }
        return;
    }
    let (profile, site, expected, write) = operations[index];
    let screen_label = label(profile);
    let origin = format!("https://{site}.invalid");
    let nonce = format!("{}-{index}", if readback { "read" } else { "write" });
    let target_uri = format!("{origin}/__fixture/{nonce}");
    let target_html = format!("<!doctype html><meta name='sumi-fixture' content='{nonce}'><title>Synthetic {nonce}</title><p>Takosumi engine fixture</p>");
    let native_view =
        SCREENS.with(|screens| screens.borrow().get(&screen_label).unwrap().view.webview());
    // All factory bootstrap loads finished before the first observation.
    // Do not stop an in-flight initial load and race its cancellation/error
    // document against this synthetic target.
    let expected_uri = target_uri.clone();
    let diagnostic_view = native_view.clone();
    after_document_load(
        &native_view,
        target_uri.clone(),
        nonce.clone(),
        app.clone(),
        move || {
            let script = format!("(() => {{ try {{ if (location.href !== '{expected_uri}' || location.origin !== '{origin}' || document.querySelector('meta[name=sumi-fixture]')?.content !== '{nonce}') return {{wrongDocument: true, uri: location.href, origin: location.origin, nonce: document.querySelector('meta[name=sumi-fixture]')?.content}}; const before = {{nonce: '{nonce}', uri: location.href, origin: location.origin, cookie: document.cookie, storage: localStorage.getItem('identity') || '', tauri: typeof window.__TAURI_INTERNALS__, ipc: typeof window.ipc}}; {} return before; }} catch (error) {{ return {{failure: String(error), uri: location.href, title: document.title}}; }} }})()",
            if write.is_empty() { String::new() } else {format!("document.cookie = 'identity={write}; Path=/; Max-Age=3600; Secure; SameSite=Lax'; localStorage.setItem('identity', '{write}');")});
            let next_app = app.clone();
            diagnostic_view.evaluate_javascript(&script, None, None, None::<&gtk::gio::Cancellable>, move |native_result| {
            let result = match native_result {
                Ok(value) => match value.to_json(0) {
                    Some(json) => json.to_string(),
                    None => { eprintln!("FAIL: native evaluation returned no JSON at observation {index}"); next_app.exit(1); return; }
                },
                Err(error) => { eprintln!("FAIL: native evaluation failed at observation {index}, target={expected_uri}: {error:?}"); next_app.exit(1); return; }
            };
            let valid = serde_json::from_str::<serde_json::Value>(&result).ok().is_some_and(|value| {
                value["uri"] == expected_uri && value["origin"] == origin && value["nonce"] == nonce
                    && value["cookie"] == if expected.is_empty() { String::new() } else { format!("identity={expected}") }
                    && value["storage"] == expected && value["tauri"] == "undefined" && value["ipc"] == "undefined"
            });
            println!("observation {index}: profile={profile}, site={site}, expected={expected:?}, result={result}, valid={valid}");
            if !valid { next_app.exit(1); return; }
            let handle = next_app.clone();
            let data = directory.clone();
            // Callback may be dispatched by an engine thread. Schedule all
            // view/GTK access on Tauri's UI thread again.
            let _ = next_app.run_on_main_thread(move || step(handle, data, readback, index + 1));
        });
        },
    );
    if index == 0 {
        // Controlled old-document completion before the requested document.
        // The readiness helper must not observe the previous document.
        load_after_stale_document(&native_view, target_html, target_uri);
    } else {
        native_view.load_html(&target_html, Some(&target_uri));
    }
}

fn reset_profile_probe(app: AppHandle, directory: PathBuf) {
    let (send, _) = std::sync::mpsc::sync_channel(1);
    let live = profile_reset::begin(
        &app,
        crate::ClearProfileRequest {
            profile: WORK.into(),
            key: request(WORK).key,
        },
        directory.clone(),
        send,
    );
    if !matches!(live, Err(Error::InvalidRequest(message)) if message.contains("すべて閉じて"))
    {
        eprintln!("FAIL: profile clear did not reject a live native screen");
        app.exit(1);
        return;
    }
    let (send, _) = std::sync::mpsc::sync_channel(1);
    if profile_reset::begin(
        &app,
        crate::ClearProfileRequest {
            profile: "shared".into(),
            key: request("shared").key,
        },
        directory.clone(),
        send,
    )
    .is_ok()
    {
        eprintln!("FAIL: profile clear accepted the legacy shared store");
        app.exit(1);
        return;
    }
    let window = SCREENS.with(|screens| screens.borrow().get(&label(WORK)).unwrap().window.clone());
    if let Err(error) = window.close() {
        eprintln!("FAIL: profile fixture close: {error}");
        app.exit(1);
        return;
    }
    reset_after_close(
        app,
        directory,
        std::time::Instant::now() + Duration::from_secs(5),
    );
}

fn reset_after_close(app: AppHandle, directory: PathBuf, deadline: std::time::Instant) {
    if SCREENS.with(|screens| {
        screens
            .borrow()
            .values()
            .any(|screen| screen.profile == WORK)
    }) {
        if std::time::Instant::now() >= deadline {
            eprintln!("FAIL: profile screen did not close before reset");
            app.exit(1);
        } else {
            gtk::glib::timeout_add_local_once(Duration::from_millis(10), move || {
                reset_after_close(app, directory, deadline)
            });
        }
        return;
    }
    let pending = (|| -> Result<_> {
        let home = request(WORK);
        let preference = ResumeStore::new(directory.join(WORK), home.key.clone());
        let prior = format!("{}document/work", home.url);
        preference.record(&prior).map_err(failure)?;
        if preference.recall().map_err(failure)?.as_deref() != Some(prior.as_str()) {
            return Err(Error::InvalidRequest(
                "The pre-clear page preference was not recorded.",
            ));
        }
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        profile_reset::begin(
            &app,
            crate::ClearProfileRequest {
                profile: WORK.into(),
                key: home.key,
            },
            directory.clone(),
            send,
        )?;
        if !matches!(
            receive.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ) {
            return Err(Error::InvalidRequest(
                "Clear acknowledged before native completion.",
            ));
        }
        if open_on_ui_thread(&app, request(WORK), directory.clone()).is_ok() {
            return Err(Error::InvalidRequest("A clearing profile reopened."));
        }
        let (send, _) = std::sync::mpsc::sync_channel(1);
        if profile_reset::begin(
            &app,
            crate::ClearProfileRequest {
                profile: WORK.into(),
                key: request(WORK).key,
            },
            directory.clone(),
            send,
        )
        .is_ok()
        {
            return Err(Error::InvalidRequest(
                "A duplicate profile clear was accepted.",
            ));
        }
        Ok(receive)
    })();
    match pending {
        Ok(receive) => reset_wait(
            app,
            directory,
            receive,
            std::time::Instant::now() + Duration::from_secs(10),
        ),
        Err(error) => {
            eprintln!("FAIL: profile reset setup: {error}");
            app.exit(1);
        }
    }
}

fn reset_wait(
    app: AppHandle,
    directory: PathBuf,
    receive: std::sync::mpsc::Receiver<Result<crate::ClearProfileResponse>>,
    deadline: std::time::Instant,
) {
    match receive.try_recv() {
        Ok(Ok(receipt)) if receipt.cleared => {
            let preference = ResumeStore::new(directory.join(WORK), request(WORK).key).recall();
            if !matches!(preference, Ok(None)) {
                eprintln!("FAIL: completed clear left a prior-page preference");
                app.exit(1);
                return;
            }
            if let Err(error) = open_on_ui_thread(&app, request(WORK), directory.clone()) {
                eprintln!("FAIL: cleared profile could not reopen: {error}");
                app.exit(1);
                return;
            }
            RESET_DONE.store(true, Ordering::SeqCst);
            let view =
                SCREENS.with(|screens| screens.borrow().get(&label(WORK)).unwrap().view.webview());
            after_document_load(
                &view,
                request(WORK).url,
                "bootstrap".into(),
                app.clone(),
                move || step(app, directory, false, 0),
            );
        }
        Err(std::sync::mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => {
            gtk::glib::timeout_add_local_once(Duration::from_millis(10), move || {
                reset_wait(app, directory, receive, deadline)
            });
        }
        result => {
            eprintln!("FAIL: native profile clear did not complete: {result:?}");
            app.exit(1);
        }
    }
}

fn load_after_stale_document(view: &webkit2gtk::WebView, html: String, uri: String) {
    use gtk::glib::prelude::ObjectExt;
    let id = Rc::new(RefCell::new(None));
    let callback_id = id.clone();
    let stale_uri = format!("{uri}?previous=1");
    let expected_stale = stale_uri.clone();
    let target = Rc::new(RefCell::new(Some((html, uri))));
    let handler = view.connect_load_changed(move |view, event| {
        if matches!(event, webkit2gtk::LoadEvent::Finished)
            && view.uri().as_deref() == Some(expected_stale.as_str())
        {
            if let Some(id) = callback_id.borrow_mut().take() {
                view.disconnect(id);
            }
            let Some((html, uri)) = target.borrow_mut().take() else {
                return;
            };
            let view = view.clone();
            if !std::env::args().any(|value| value == "--missing-document") {
                gtk::glib::idle_add_local_once(move || view.load_html(&html, Some(&uri)));
            }
        }
    });
    *id.borrow_mut() = Some(handler);
    view.load_html(
        "<!doctype html><title>Previous document</title>",
        Some(&stale_uri),
    );
}

struct PendingDocument {
    uri: String,
    nonce: String,
    app: AppHandle,
    deadline: std::time::Instant,
    callback: RefCell<Option<Box<dyn FnOnce()>>>,
}

fn after_document_load(
    view: &webkit2gtk::WebView,
    uri: String,
    nonce: String,
    app: AppHandle,
    next: impl FnOnce() + 'static,
) {
    let pending = Rc::new(PendingDocument {
        uri,
        nonce,
        app,
        deadline: std::time::Instant::now() + Duration::from_secs(5),
        callback: RefCell::new(Some(Box::new(next))),
    });
    poll_document(view.clone(), pending);
}

fn poll_document(view: webkit2gtk::WebView, pending: Rc<PendingDocument>) {
    // The caller registers readiness before starting its target navigation.
    // Do not launch an async evaluation into the document it is about to
    // replace. Check the native document first, including across process swaps.
    if view.is_loading() || view.uri().as_deref() != Some(pending.uri.as_str()) {
        if std::time::Instant::now() >= pending.deadline {
            eprintln!("FAIL: expected fixture document not ready: target={}, nonce={}, native_uri={:?}, loading={}", pending.uri, pending.nonce, view.uri(), view.is_loading());
            pending.app.exit(1);
        } else {
            gtk::glib::timeout_add_local_once(Duration::from_millis(25), move || {
                poll_document(view, pending)
            });
        }
        return;
    }
    let probe = "({uri: location.href, nonce: document.querySelector('meta[name=sumi-fixture]')?.content, ready: document.readyState})";
    let native = view.clone();
    view.evaluate_javascript(probe, None, None, None::<&gtk::gio::Cancellable>, move |result| {
        let diagnostic = match result {
            Ok(value) => value.to_json(0).map(|value| value.to_string()).unwrap_or_else(|| "No JSON result".into()),
            Err(error) => format!("Native WebKit error: {error:?}"),
        };
        let ready = !native.is_loading() && native.uri().as_deref() == Some(pending.uri.as_str())
            && serde_json::from_str::<serde_json::Value>(&diagnostic).ok().is_some_and(|value|
            value["uri"] == pending.uri && value["nonce"] == pending.nonce && value["ready"] == "complete");
        if ready {
            let callback = pending.callback.borrow_mut().take();
            if let Some(next) = callback { next(); }
        } else if std::time::Instant::now() >= pending.deadline {
            eprintln!("FAIL: expected fixture document not ready: target={}, nonce={}, native_uri={:?}, loading={}, last={diagnostic}", pending.uri, pending.nonce, native.uri(), native.is_loading());
            pending.app.exit(1);
        } else {
            // Readiness only. Cookies/storage are read and asserted exactly
            // once after identity matches; failed data is never retried.
            gtk::glib::timeout_add_local_once(Duration::from_millis(10), move || poll_document(native, pending));
        }
    });
}

fn title_and_back_probe(app: AppHandle) {
    let original = label(WORK);
    SCREENS.with(|screens| {
        screens
            .borrow()
            .get(&original)
            .unwrap()
            .view
            .webview()
            .load_html(
                "<p>Current document</p><iframe src='https://frame.invalid/'></iframe>",
                Some("https://sumi.invalid/current"),
            );
    });
    gtk::glib::timeout_add_local_once(Duration::from_millis(400), move || {
        let (view, window) = SCREENS.with(|screens| {
            let screens = screens.borrow();
            let screen = screens.get(&original).unwrap();
            (screen.view.webview(), screen.window.clone())
        });
        if window.title().unwrap() != site_title("https://sumi.invalid/") {
            eprintln!("FAIL: iframe changed the top-level origin title");
            app.exit(1);
            return;
        }
        view.load_uri("https://pending.invalid/");
        view.stop_loading();
        if window.title().unwrap() != site_title("https://sumi.invalid/") {
            eprintln!("FAIL: cancelled navigation changed the displayed origin");
            app.exit(1);
            return;
        }
        println!("PASS: iframe/cancelled navigation preserves committed origin title");
        native_back_probe(app);
    });
}

fn native_back_probe(app: AppHandle) {
    let base =
        std::env::var("TAKOSUMI_FIXTURE_HTTPS").expect("Run via native-tests/desktop/run.py");
    let certificate =
        std::fs::read_to_string(std::env::var_os("TAKOSUMI_FIXTURE_CERT").unwrap()).unwrap();
    let certificate = gtk::gio::TlsCertificate::from_pem(&certificate).unwrap();
    let original = label(WORK);
    let view = SCREENS.with(|screens| screens.borrow().get(&original).unwrap().view.webview());
    // Fixture-only exact local certificate/host trust. Production does not
    // install a TLS exception or disable certificate validation.
    view.context()
        .unwrap()
        .allow_tls_certificate_for_host(&certificate, "127.0.0.1");
    view.load_uri(&format!("{base}/first"));
    gtk::glib::timeout_add_local_once(Duration::from_millis(500), move || {
        view.load_uri(&format!("{base}/second"));
        gtk::glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let expected = format!("{base}/first");
            let handle = app.clone();
            SCREENS.with(|screens| screens.borrow().get(&original).unwrap().view.evaluate_script_with_callback(
                "history.back = () => { window.backHijacked = true; }; document.body.textContent.trim() === 'second'", move |result| {
                    if result != "true" { eprintln!("FAIL: HTTPS history fixture did not load second page: {result}"); handle.exit(1); return; }
                    let app = handle.clone();
                    let expected = expected.clone();
                    let original = label(WORK);
                    handle_action(&app, &original, "web-back");
                    gtk::glib::timeout_add_local_once(Duration::from_millis(500), move || {
                        let current = SCREENS.with(|screens| screens.borrow().get(&original).unwrap().view.url()).unwrap();
                        if current != expected { eprintln!("FAIL: native Back: {current} vs {expected}"); app.exit(1); return; }
                        println!("PASS: native Back traverses real local HTTPS history despite page override");
                        popup_probe(app);
                    });
                }
            ).unwrap());
        });
    });
}

fn popup_probe(app: AppHandle) {
    let original = label(WORK);
    SCREENS.with(|screens| screens.borrow().get(&original).unwrap().view.webview().load_html(
        "<button id='popup' autofocus onclick=\"window.open('about:blank','sumi-fixture-popup')\">Open popup</button>",
        Some("https://sumi.invalid/")));
    gtk::glib::timeout_add_local_once(Duration::from_millis(400), move || {
        use gtk::prelude::WidgetExt;
        let window = SCREENS.with(|screens| {
            let screens = screens.borrow();
            let screen = screens.get(&original).unwrap();
            screen.view.webview().grab_focus();
            screen.window.clone()
        });
        window.set_title("SumiFixtureOpener").unwrap();
        gtk::glib::timeout_add_local_once(Duration::from_millis(250), move || {
            let output = std::process::Command::new("xdotool")
                .args(["search", "--onlyvisible", "--name", "^SumiFixtureOpener$"])
                .output()
                .unwrap();
            let id = String::from_utf8(output.stdout).unwrap();
            let Some(id) = id.lines().next() else {
                eprintln!("FAIL: fixture opener has no X11 window");
                app.exit(1);
                return;
            };
            let status = std::process::Command::new("xdotool")
                .args(["windowfocus", "--sync", id, "key", "--window", id, "Return"])
                .status()
                .unwrap();
            assert!(status.success(), "Fixture native key injection failed");
            gtk::glib::timeout_add_local_once(Duration::from_millis(500), move || {
                let popup = SCREENS.with(|screens| {
                    screens
                        .borrow()
                        .iter()
                        .find(|(_, screen)| screen.key.is_empty())
                        .map(|(label, _)| label.clone())
                });
                let Some(popup) = popup else {
                    eprintln!("FAIL: actual window.open produced no popup");
                    app.exit(1);
                    return;
                };
                let handle = app.clone();
                SCREENS.with(|screens| screens.borrow().get(&popup).unwrap().view.evaluate_script_with_callback(
                "({cookie: document.cookie, storage: localStorage.getItem('identity'), opener: window.opener !== null, tauri: typeof __TAURI_INTERNALS__, ipc: typeof ipc})", move |result| {
                    let value: serde_json::Value = serde_json::from_str(&result).unwrap_or_default();
                    if value["cookie"] != "identity=work" || value["storage"] != "work" || value["opener"] != true || value["tauri"] != "undefined" || value["ipc"] != "undefined" {
                        eprintln!("FAIL: actual popup profile/bridge/opener: {result}"); handle.exit(1); return;
                    }
                    println!("Popup profile/opener observation: {result}");
                    let app = handle.clone();
                    let popup = popup.clone();
                    handle.run_on_main_thread(move || {
                        SCREENS.with(|screens| screens.borrow().get(&popup).unwrap().view.evaluate_script_with_callback("window.close(); ({closed: window.closed, opener: window.opener !== null, length: history.length})", |result| eprintln!("fixture JS close evaluation: {result}")).unwrap());
                        gtk::glib::timeout_add_local_once(Duration::from_millis(400), move || {
                            if SCREENS.with(|screens| screens.borrow().contains_key(&popup)) {
                                eprintln!("FAIL: window.close left a native popup/registry entry"); app.exit(1); return;
                            }
                            println!("PASS: actual native-input window.open/profile/opener/window.close; no content messaging");
                            // Settled-state process restart, not crash recovery.
                            gtk::glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                                EXIT_STATUS.store(0, Ordering::SeqCst); app.exit(0);
                            });
                        });
                    }).unwrap();
                }
            ).unwrap());
            });
        });
    });
}

fn lifecycle(app: &AppHandle, directory: &Path) -> Result<()> {
    let original = label(WORK);
    open_on_ui_thread(app, request(WORK), directory.to_path_buf())?;
    assert_eq!(label(WORK), original, "Resume reuses the existing screen");
    let mut retarget = request(PERSONAL);
    retarget.key = request(WORK).key;
    assert!(
        open_on_ui_thread(app, retarget, directory.to_path_buf()).is_err(),
        "Retarget must fail"
    );
    handle_action(app, &original, "web-picker");
    assert!(
        app.get_webview_window("main")
            .unwrap()
            .is_visible()
            .map_err(failure)?,
        "Picker is shown"
    );
    assert!(
        SCREENS
            .with(|screens| screens.borrow().get(&original).unwrap().view.url())
            .map_err(failure)?
            .starts_with("https://other.invalid/")
            || SCREENS
                .with(|screens| screens.borrow().get(&original).unwrap().view.url())
                .map_err(failure)?
                .starts_with("https://sumi.invalid/")
    );
    Ok(())
}
