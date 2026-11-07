//! macOS (M1): the Cocoa parts of the browser process.
//!
//! CEF requires the browser process's `NSApplication` to implement its
//! `CefAppProtocol` (`isHandlingSendEvent` / `setHandlingSendEvent:`, which
//! Chromium's event handling reads) and to be the shared application before
//! CefInitialize: `install_application` makes `TakosumiCefApplication` the
//! shared application, as CEF's cefsimple and cef-rs's example do. The
//! raise of a reused window brings its `NSWindow` forward, as the X11 and
//! Win32 raises do on Linux and Windows; the window number reported to the
//! App is that window's `windowNumber`.
use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::{
    define_class, extern_methods, msg_send,
    rc::Retained,
    runtime::{AnyObject, Bool, NSObjectProtocol},
    sel, ClassType, DefinedClass, MainThreadMarker,
};
use objc2_app_kit::{NSApp, NSApplication, NSEvent};
use std::{cell::Cell, ffi::c_void};

/// Instance variables of `TakosumiCefApplication`.
#[derive(Default)]
pub struct ApplicationIvars {
    handling_send_event: Cell<Bool>,
}

define_class!(
    /// The browser process's `NSApplication`, implementing CEF's
    /// `CefAppProtocol`.
    #[unsafe(super(NSApplication))]
    #[ivars = ApplicationIvars]
    #[name = "TakosumiCefApplication"]
    pub struct TakosumiCefApplication;

    impl TakosumiCefApplication {
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            let was_sending = self.ivars().handling_send_event.get();
            if !was_sending.as_bool() {
                self.ivars().handling_send_event.set(Bool::YES);
            }
            let _: () = msg_send![super(self), sendEvent: event];
            if !was_sending.as_bool() {
                self.ivars().handling_send_event.set(Bool::NO);
            }
        }

        /// Cocoa's quit (the Dock menu, logout) would end the process with
        /// `exit()` and skip CEF's shutdown. The App owns this browser's
        /// lifetime (its Shutdown request, or the end of its input), so a
        /// quit from the system is only reported.
        #[unsafe(method(terminate:))]
        unsafe fn terminate(&self, _sender: Option<&AnyObject>) {
            eprintln!("The system asked the browser to quit; the App ends it.");
        }
    }

    unsafe impl CrAppControlProtocol for TakosumiCefApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn set_handling_send_event(&self, handling_send_event: Bool) {
            self.ivars().handling_send_event.set(handling_send_event);
        }
    }

    unsafe impl CrAppProtocol for TakosumiCefApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn is_handling_send_event(&self) -> Bool {
            self.ivars().handling_send_event.get()
        }
    }

    unsafe impl CefAppProtocol for TakosumiCefApplication {}
);

impl TakosumiCefApplication {
    extern_methods! {
        #[unsafe(method(sharedApplication))]
        fn shared_application() -> Retained<Self>;
    }
}

/// Make `TakosumiCefApplication` the shared application (before anything
/// else touches `NSApp`, and before CefInitialize), with the regular
/// activation policy of an app with windows. Panics off the main thread or
/// when another class already is the shared application.
pub fn install_application() {
    let mtm = MainThreadMarker::new().expect("The browser starts on the main thread");
    let _ = TakosumiCefApplication::shared_application();
    let app = NSApp(mtm);
    assert!(
        app.isKindOfClass(TakosumiCefApplication::class()),
        "NSApp was created before the browser's application class"
    );
    // NSApplicationActivationPolicyRegular.
    let _: Bool = unsafe { msg_send![&app, setActivationPolicy: 0isize] };
}

/// Whether an Objective-C object answers `selector`.
fn responds(object: &AnyObject, selector: objc2::runtime::Sel) -> bool {
    let answer: Bool = unsafe { msg_send![object, respondsToSelector: selector] };
    answer.as_bool()
}

/// The `NSWindow` of a browser's native window handle, if it has one. CEF's
/// handle on macOS is an `NSView` (for a Chrome-style window the content view
/// of its `NSWindow`), null before CEF sets it; an `NSView` not yet in a
/// window answers nil. A handle that is itself a window is taken as it is.
fn window_of<'a>(handle: *mut c_void) -> Option<&'a AnyObject> {
    // SAFETY: the handle is null or a live AppKit object CEF owns while the
    // browser lives; it is only asked selectors it answers.
    let object = unsafe { handle.cast::<AnyObject>().as_ref() }?;
    if responds(object, sel!(window)) {
        let window: *mut AnyObject = unsafe { msg_send![object, window] };
        unsafe { window.as_ref() }
    } else if responds(object, sel!(windowNumber)) {
        Some(object)
    } else {
        None
    }
}

/// The window number of a browser's `NSWindow` (0 when it has none yet).
pub fn window_number(handle: *mut c_void) -> u64 {
    window_of(handle)
        .map(|window| {
            let number: isize = unsafe { msg_send![window, windowNumber] };
            u64::try_from(number).unwrap_or(0)
        })
        .unwrap_or(0)
}

/// What a browser's native window handle shows, for the browser's standard
/// error: null or not, its class, whether it is in an `NSWindow`, and that
/// window's number (0 when none).
pub fn describe(handle: *mut c_void) -> String {
    // SAFETY: as in `window_of`.
    let Some(object) = (unsafe { handle.cast::<AnyObject>().as_ref() }) else {
        return "window handle null, no NSWindow, windowNumber 0".into();
    };
    let class = object.class().name().to_string_lossy().into_owned();
    let window = window_of(handle).is_some();
    format!(
        "window handle set ({class}), NSWindow {}, windowNumber {}",
        if window { "present" } else { "nil" },
        window_number(handle)
    )
}

/// Bring a reused browser window forward: restore it when minimized,
/// activate the application (`activate` on macOS 14 and later, otherwise
/// `activateIgnoringOtherApps:`), make it the key window and order it to
/// the front, also when the application is not active. macOS decides
/// activation cooperatively, so whether the window becomes the active one
/// is not checked here.
pub fn raise(handle: *mut c_void) -> Result<(), &'static str> {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    // Resolved now: the window holding the browser can change.
    let window = window_of(handle).ok_or("the browser view has no window")?;
    let nil: Option<&AnyObject> = None;
    unsafe {
        let miniaturized: Bool = msg_send![window, isMiniaturized];
        if miniaturized.as_bool() {
            let _: () = msg_send![window, deminiaturize: nil];
        }
        let app = NSApp(mtm);
        let modern: Bool = msg_send![&app, respondsToSelector: sel!(activate)];
        if modern.as_bool() {
            let _: () = msg_send![&app, activate];
        } else {
            let _: () = msg_send![&app, activateIgnoringOtherApps: Bool::YES];
        }
        let _: () = msg_send![window, makeKeyAndOrderFront: nil];
        let _: () = msg_send![window, orderFrontRegardless];
    }
    Ok(())
}
