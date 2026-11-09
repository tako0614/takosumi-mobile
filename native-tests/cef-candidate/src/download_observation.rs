//! Fixture-only, bounded download observations. No CEF object leaves the UI
//! thread, and no download callback is invoked or retained. A separate thread
//! writes copied numeric/boolean fields so slow stderr cannot stall the UI.
use cef::*;
use std::{
    io::Write,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{sync_channel, SyncSender},
        OnceLock,
    },
    time::Instant,
};

const LIMIT: usize = 64;
static TRACE: OnceLock<Option<Trace>> = OnceLock::new();
struct Trace {
    sequence: AtomicUsize,
    started: Instant,
    sender: SyncSender<Event>,
}
#[derive(Clone, Copy)]
enum Phase {
    CanDownload,
    Before,
    Updated,
    Truncated,
}
impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::CanDownload => "can-download",
            Self::Before => "before",
            Self::Updated => "updated",
            Self::Truncated => "truncated",
        }
    }
}
struct Event {
    phase: Phase,
    sequence: usize,
    milliseconds: u64,
    browser: Option<i32>,
    valid: Option<bool>,
    item: Option<serde_json::Value>,
}
impl Event {
    fn json(self) -> serde_json::Value {
        serde_json::json!({"event":self.phase.name(),"sequence":self.sequence,"milliseconds":self.milliseconds,
            "pid":std::process::id(),"browserId":self.browser,"valid":self.valid,"item":self.item})
    }
}
// Reserve exactly LIMIT observations and one truncation marker, never wrap.
fn reserve(sequence: &AtomicUsize) -> Option<usize> {
    sequence
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
            if n <= LIMIT {
                Some(n + 1)
            } else {
                None
            }
        })
        .ok()
}
fn trace() -> Option<&'static Trace> {
    TRACE.get_or_init(|| {
        // Even if stderr is blocked, the UI can enqueue the entire bounded
        // trace without waiting. No joining/flushing is added to shutdown:
        // absent/partial diagnostics cannot prove a lifecycle outcome.
        let (sender, receiver) = sync_channel::<Event>(LIMIT+1);
        std::thread::Builder::new().name("download-observer".into()).spawn(move || {
            let ready=serde_json::json!({"event":"observer-ready","pid":std::process::id(),"limit":LIMIT});
            let emit=|value:serde_json::Value| {
                let line=format!("SUMI_DOWNLOAD {value}\n");
                let _=std::io::stderr().lock().write_all(line.as_bytes());
            };
            emit(ready);
            while let Ok(event)=receiver.recv() { emit(event.json()); }
        }).ok()?;
        Some(Trace{sequence:AtomicUsize::new(0),started:Instant::now(),sender})
    }).as_ref()
}
fn record(phase: Phase, browser: Option<&mut Browser>, item: Option<&mut DownloadItem>) {
    let Some(trace) = trace() else {
        return;
    };
    let Some(sequence) = reserve(&trace.sequence) else {
        return;
    };
    let milliseconds = u64::try_from(trace.started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let event = if sequence == LIMIT {
        Event {
            phase: Phase::Truncated,
            sequence,
            milliseconds,
            browser: None,
            valid: None,
            item: None,
        }
    } else {
        let valid = item.as_ref().map(|item| item.is_valid() != 0);
        let item=item.filter(|_|valid==Some(true)).map(|item|serde_json::json!({
            "id":item.id(),"inProgress":item.is_in_progress()!=0,"complete":item.is_complete()!=0,
            "canceled":item.is_canceled()!=0,"interrupted":item.is_interrupted()!=0,
            "paused":item.is_paused()!=0,"interruptReason":item.interrupt_reason().get_raw(),
            "receivedBytes":item.received_bytes(),"totalBytes":item.total_bytes(),"percentComplete":item.percent_complete()
        }));
        Event {
            phase,
            sequence,
            milliseconds,
            browser: browser.map(|b| b.identifier()),
            valid,
            item,
        }
    };
    // No callback/browser/item reference crosses threads; a failed observer
    // must never change the native download's admission or control flow.
    let _ = trace.sender.try_send(event);
}

wrap_download_handler! {
    struct Observer;
    impl DownloadHandler {
        fn can_download(&self, browser: Option<&mut Browser>, _url: Option<&CefString>, _request_method: Option<&CefString>) -> i32 {
            record(Phase::CanDownload,browser,None);
            // Pinned CEF C++ default is true. The generated Rust default is
            // zero, which would silently deny downloads when observing them.
            1
        }
        fn on_before_download(&self, browser: Option<&mut Browser>, item: Option<&mut DownloadItem>, _suggested_name: Option<&CefString>, _callback: Option<&mut BeforeDownloadCallback>) -> i32 {
            record(Phase::Before,browser,item);
            // False preserves Chrome-style handling. Never call Continue.
            0
        }
        fn on_download_updated(&self, browser: Option<&mut Browser>, item: Option<&mut DownloadItem>, _callback: Option<&mut DownloadItemCallback>) {
            record(Phase::Updated,browser,item);
        }
    }
}
pub(super) fn handler() -> DownloadHandler {
    let _ = trace();
    Observer::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_handler_preserves_admission_and_chrome_default_handling() {
        let handler = Observer::new();
        assert_eq!(handler.can_download(None, None, None), 1);
        assert_eq!(handler.on_before_download(None, None, None, None), 0);
        handler.on_download_updated(None, None, None);
    }
    #[test]
    fn trace_has_a_fixed_limit_and_one_explicit_truncation_slot() {
        let sequence = AtomicUsize::new(0);
        for n in 0..=LIMIT {
            assert_eq!(reserve(&sequence), Some(n));
        }
        for _ in 0..100 {
            assert_eq!(reserve(&sequence), None);
        }
        assert_eq!(sequence.load(Ordering::Relaxed), LIMIT + 1);
    }
    #[test]
    fn absent_item_is_explicit_and_carries_no_fabricated_completion() {
        let value = Event {
            phase: Phase::CanDownload,
            sequence: 0,
            milliseconds: 1,
            browser: Some(3),
            valid: None,
            item: None,
        }
        .json();
        assert_eq!(value["event"], "can-download");
        assert!(value["valid"].is_null() && value["item"].is_null());
        let mut keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys,
            [
                "browserId",
                "event",
                "item",
                "milliseconds",
                "pid",
                "sequence",
                "valid"
            ]
        );
    }
}
