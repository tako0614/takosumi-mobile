//! Fixture-only scalar load records for the existing private browser stderr.
//! The UI-owned State reserves the first 64 main-frame events. A bounded
//! nonwaiting channel carries copied scalars to one opt-in writer. No CEF
//! object, URL or clock crosses it; failed/partial output is only diagnostic.
use std::{
    io::Write,
    sync::mpsc::{sync_channel, SyncSender},
    thread::JoinHandle,
};

pub(crate) const LIMIT: u8 = 64;
const MAX_LINE_BYTES: usize = 128;

#[derive(Clone, Copy)]
pub(crate) enum Event {
    Start,
    End(i32),
    Error(i32),
}

pub(crate) struct Observation {
    sender: Option<SyncSender<Record>>,
    used: u8,
}

struct Record {
    browser_id: i32,
    event: Event,
}

pub(crate) struct Pending {
    sender: SyncSender<Record>,
    record: Record,
}

fn spawn_writer(
    mut emit: impl FnMut(Record) + Send + 'static,
) -> Option<(SyncSender<Record>, JoinHandle<()>)> {
    let (sender, receiver) = sync_channel::<Record>(usize::from(LIMIT));
    let worker = std::thread::Builder::new()
        .name("cef-load-observer".into())
        .spawn(move || {
            while let Ok(record) = receiver.recv() {
                emit(record);
            }
        })
        .ok()?;
    Some((sender, worker))
}

impl Observation {
    pub(crate) fn new(enabled: bool) -> Self {
        Self::with_writer(enabled, || {
            let (sender, worker) =
                spawn_writer(|record| record.write_to(&mut std::io::stderr().lock()))?;
            // State owns the sender. Dropping it closes the receiver after
            // pending records; no joining/flushing is added to CEF shutdown.
            drop(worker);
            Some(sender)
        })
    }

    fn with_writer(enabled: bool, create: impl FnOnce() -> Option<SyncSender<Record>>) -> Self {
        Self {
            sender: if enabled { create() } else { None },
            used: 0,
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.sender.is_some() && self.used < LIMIT
    }

    pub(crate) fn take(
        &mut self,
        browser_id: i32,
        main_frame: bool,
        event: Event,
    ) -> Option<Pending> {
        if !self.active() || !main_frame || browser_id <= 0 {
            return None;
        }
        let sender = self.sender.as_ref()?.clone();
        // Reserve an attempt before send: a failed writer cannot grow it.
        self.used += 1;
        Some(Pending {
            sender,
            record: Record { browser_id, event },
        })
    }
}

impl Pending {
    pub(crate) fn send(self) {
        let _ = self.sender.try_send(self.record);
    }
}

impl Record {
    fn write_to(self, sink: &mut impl Write) {
        let (kind, status, error) = match self.event {
            Event::Start => ("start", None, None),
            Event::End(status) => ("end", Some(status), None),
            Event::Error(code) => ("error", None, Some(code)),
        };
        let number =
            |value: Option<i32>| value.map_or_else(|| "null".to_owned(), |n| n.to_string());
        let line = format!(
            "SUMI_CEF_LOAD {{\"kind\":\"{kind}\",\"browserId\":{},\"mainFrame\":true,\"httpStatus\":{},\"errorCode\":{}}}\n",
            self.browser_id, number(status), number(error),
        );
        if line.len() <= MAX_LINE_BYTES {
            let _ = sink.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    fn channel(capacity: usize) -> (Observation, std::sync::mpsc::Receiver<Record>) {
        let (sender, receiver) = sync_channel(capacity);
        (Observation::with_writer(true, || Some(sender)), receiver)
    }

    #[test]
    fn normal_and_non_main_inputs_keep_channel_silent() {
        let mut normal = Observation::new(false);
        for _ in 0..1000 {
            assert!(normal.take(1, true, Event::Start).is_none());
        }
        assert!(!normal.active());
        assert_eq!(normal.used, 0);

        let mut calls = 0;
        let disabled = Observation::with_writer(false, || {
            calls += 1;
            None
        });
        assert!(!disabled.active());
        assert_eq!(calls, 0);
        assert!(!Observation::with_writer(true, || None).active());
        let (mut inspected, _receiver) = channel(usize::from(LIMIT));
        for _ in 0..1000 {
            assert!(inspected.take(1, false, Event::End(200)).is_none());
            assert!(inspected.take(0, true, Event::Error(-1)).is_none());
            assert!(inspected.take(-1, true, Event::Error(-1)).is_none());
        }
        assert_eq!(inspected.used, 0);
        assert!(inspected.take(1, true, Event::Start).is_some());
    }

    #[test]
    fn scalar_events_keep_signed_codes_and_closed_fields() {
        let (mut observation, receiver) = channel(usize::from(LIMIT));
        let mut sink = Vec::new();
        for event in [Event::Start, Event::End(200), Event::Error(-202)] {
            observation.take(7, true, event).unwrap().send();
        }
        for record in receiver.try_iter() {
            record.write_to(&mut sink);
        }
        let wire = String::from_utf8(sink).unwrap();
        assert_eq!(wire, concat!(
            "SUMI_CEF_LOAD {\"kind\":\"start\",\"browserId\":7,\"mainFrame\":true,\"httpStatus\":null,\"errorCode\":null}\n",
            "SUMI_CEF_LOAD {\"kind\":\"end\",\"browserId\":7,\"mainFrame\":true,\"httpStatus\":200,\"errorCode\":null}\n",
            "SUMI_CEF_LOAD {\"kind\":\"error\",\"browserId\":7,\"mainFrame\":true,\"httpStatus\":null,\"errorCode\":-202}\n",
        ));
        println!("LOAD_CONTROL_BEGIN\n{wire}LOAD_CONTROL_END");
    }

    #[test]
    fn first_64_events_and_bytes_are_bounded() {
        let (mut observation, receiver) = channel(usize::from(LIMIT));
        let mut sink = Vec::new();
        for n in 0..1000 {
            let event = if n % 2 == 0 {
                Event::End(i32::MIN)
            } else {
                Event::Error(i32::MAX)
            };
            if let Some(record) = observation.take(i32::MAX, true, event) {
                record.send();
            }
        }
        for record in receiver.try_iter() {
            record.write_to(&mut sink);
        }
        assert_eq!(observation.used, LIMIT);
        assert!(!observation.active());
        assert_eq!(
            sink.iter().filter(|b| **b == b'\n').count(),
            usize::from(LIMIT)
        );
        assert!(sink.len() <= usize::from(LIMIT) * MAX_LINE_BYTES);
        assert!(sink
            .split(|b| *b == b'\n')
            .all(|line| line.len() < MAX_LINE_BYTES));
    }

    struct Refused {
        calls: Arc<AtomicUsize>,
        first_partial: bool,
    }
    impl Write for Refused {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if self.first_partial && call == 1 {
                return Ok(bytes.len().min(3));
            }
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "closed test sink",
            ))
        }
        fn flush(&mut self) -> io::Result<()> {
            panic!("observation must not add a flush")
        }
    }

    #[test]
    fn full_and_disconnected_channels_do_not_wait_or_refund_budget() {
        let (mut observation, receiver) = channel(0);
        for _ in 0..1000 {
            if let Some(record) = observation.take(1, true, Event::Error(-1)) {
                record.send();
            }
        }
        assert_eq!(observation.used, LIMIT);
        assert!(receiver.try_recv().is_err());
        let (mut observation, receiver) = channel(1);
        drop(receiver);
        for _ in 0..1000 {
            if let Some(record) = observation.take(1, true, Event::Start) {
                record.send();
            }
        }
        assert_eq!(observation.used, LIMIT);
    }

    #[test]
    fn actual_writer_failures_and_partial_writes_do_not_escape() {
        for partial in [false, true] {
            let calls = Arc::new(AtomicUsize::new(0));
            let mut sink = Refused {
                calls: calls.clone(),
                first_partial: partial,
            };
            let (sender, worker) = spawn_writer(move |record| record.write_to(&mut sink)).unwrap();
            let mut observation = Observation::with_writer(true, || Some(sender));
            observation.take(1, true, Event::Start).unwrap().send();
            drop(observation);
            worker.join().unwrap(); // Only this owned test thread, never production shutdown.
            assert_eq!(calls.load(Ordering::SeqCst), if partial { 2 } else { 1 });
        }
    }

    #[test]
    fn a_panicking_writer_only_loses_diagnostics() {
        let (sender, worker) = spawn_writer(|_| panic!("closed test observer")).unwrap();
        let mut observation = Observation::with_writer(true, || Some(sender));
        observation.take(1, true, Event::Start).unwrap().send();
        assert!(worker.join().is_err());
        observation.take(1, true, Event::End(200)).unwrap().send();
        assert_eq!(observation.used, 2);
    }
}
