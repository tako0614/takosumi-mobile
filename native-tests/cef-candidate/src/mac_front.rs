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

/// The `NSWindow` of a browser's native view (CEF's window handle on
/// macOS), if it has one.
fn window_of(view: *mut c_void) -> Option<&'static AnyObject> {
    // SAFETY: CEF's window handle on macOS is the browser's NSView (or
    // null); `window` returns its NSWindow or nil, owned by AppKit while the
    // browser lives.
    let view = unsafe { view.cast::<AnyObject>().as_ref() }?;
    let window: *mut AnyObject = unsafe { msg_send![view, window] };
    unsafe { window.as_ref() }
}

/// The window number of a browser's `NSWindow` (0 when it has none).
pub fn window_number(view: *mut c_void) -> u64 {
    window_of(view)
        .map(|window| {
            let number: isize = unsafe { msg_send![window, windowNumber] };
            u64::try_from(number).unwrap_or(0)
        })
        .unwrap_or(0)
}

/// Bring a reused browser window forward: restore it when minimized,
/// activate the application (`activate` on macOS 14 and later, otherwise
/// `activateIgnoringOtherApps:`), make it the key window and order it to
/// the front, also when the application is not active. macOS decides
/// activation cooperatively, so whether the window becomes the active one
/// is not checked here.
pub fn raise(view: *mut c_void) -> Result<(), &'static str> {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let window = window_of(view).ok_or("the browser view has no window")?;
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
