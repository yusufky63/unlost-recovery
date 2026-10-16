//! The UI's view of the engine: one call API, two backends. Image files are scanned in this
//! process; raw volumes and shadow copies go to the elevated broker. Results come back as a
//! stream of events per call, with backpressure so a fast reader cannot flood memory.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use parking_lot::Mutex;
use serde_json::Value;

use crate::broker::BrokerClient;
use crate::error::ErrorPayload;
use crate::job::ProgressSnapshot;
use crate::protocol::Request;
use crate::session::{Session, Sink};
use crate::{Error, Result, system};

/// Scan ids at or above this belong to the broker's session.
pub const BROKER_SCAN_BASE: u32 = 1_000_000;
/// Data chunks buffered per call before the producer waits.
const CHANNEL_DEPTH: usize = 16;

pub enum Event {
    Progress(ProgressSnapshot),
    Data(Vec<u8>),
    Done(std::result::Result<Value, ErrorPayload>),
}

#[derive(Default)]
pub struct Router {
    map: Mutex<HashMap<u64, SyncSender<Event>>>,
}

impl Router {
    fn register(&self, id: u64) -> Receiver<Event> {
        let (tx, rx) = sync_channel(CHANNEL_DEPTH);
        self.map.lock().insert(id, tx);
        rx
    }

    fn sender(&self, id: u64) -> Option<SyncSender<Event>> {
        self.map.lock().get(&id).cloned()
    }

    pub fn progress(&self, id: u64, p: ProgressSnapshot) {
        if let Some(tx) = self.sender(id) {
            // Progress is advisory: drop it rather than block the stream.
            if let Err(TrySendError::Disconnected(_)) = tx.try_send(Event::Progress(p)) {
                self.map.lock().remove(&id);
            }
        }
    }

    pub fn data(&self, id: u64, bytes: Vec<u8>) -> Result<()> {
        let tx = self.sender(id).ok_or(Error::Cancelled)?;
        tx.send(Event::Data(bytes)).map_err(|_| {
            self.map.lock().remove(&id);
            Error::Cancelled
        })
    }

    pub fn done(&self, id: u64, r: std::result::Result<Value, ErrorPayload>) {
        if let Some(tx) = self.map.lock().remove(&id) {
            let _ = tx.send(Event::Done(r));
        }
    }

    pub fn fail_all(&self, e: &Error) {
        let all: Vec<(u64, SyncSender<Event>)> = self.map.lock().drain().collect();
        for (_, tx) in all {
            let _ = tx.send(Event::Done(Err(ErrorPayload::from(e))));
        }
    }
}

struct RouterSink(Arc<Router>);

impl Sink for RouterSink {
    fn progress(&self, id: u64, p: ProgressSnapshot) {
        self.0.progress(id, p);
    }
    fn data(&self, id: u64, bytes: &[u8]) -> Result<()> {
        self.0.data(id, bytes.to_vec())
    }
}

pub struct Engine {
    session: Arc<Session>,
    router: Arc<Router>,
    broker: Mutex<Option<Arc<BrokerClient>>>,
    next: AtomicU64,
}

pub struct Call {
    pub id: u64,
    rx: Receiver<Event>,
    cancel: Canceller,
}

pub type Canceller = Arc<dyn Fn() + Send + Sync>;

impl Call {
    /// Collects the call's events until it finishes. A failing `on_data` cancels the call.
    pub fn wait(
        self,
        mut on_progress: impl FnMut(ProgressSnapshot),
        mut on_data: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<Value> {
        let mut failed: Option<Error> = None;
        for ev in self.rx.iter() {
            match ev {
                Event::Progress(p) => on_progress(p),
                Event::Data(b) => {
                    if failed.is_none()
                        && let Err(e) = on_data(&b)
                    {
                        (self.cancel)();
                        failed = Some(e);
                    }
                }
                Event::Done(r) => {
                    if let Some(e) = failed {
                        return Err(e);
                    }
                    return r.map_err(Error::from);
                }
            }
        }
        Err(failed.unwrap_or_else(|| Error::Broker("the engine stopped before answering".into())))
    }

    pub fn cancel(&self) {
        (self.cancel)();
    }

    /// A handle that cancels this call from another thread.
    pub fn canceller(&self) -> Canceller {
        self.cancel.clone()
    }
}

impl Engine {
    pub fn new() -> Arc<Self> {
        Arc::new(Engine {
            session: Arc::new(Session::new()),
            router: Arc::new(Router::default()),
            broker: Mutex::new(None),
            next: AtomicU64::new(1),
        })
    }

    pub fn broker_ready(&self) -> bool {
        self.broker.lock().as_ref().is_some_and(|b| b.alive())
    }

    /// True when admin work can run now: this process is elevated or the broker is up.
    pub fn admin_ready(&self) -> bool {
        system::is_elevated() || self.broker_ready()
    }

    /// Starts the broker through UAC. `Ok(false)` when the user declined.
    pub fn start_broker(&self) -> Result<bool> {
        if self.broker_ready() {
            return Ok(true);
        }
        match BrokerClient::launch(self.router.clone())? {
            Some(b) => {
                *self.broker.lock() = Some(b);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Sends a request. `admin` requests use the broker unless this process is elevated.
    pub fn call(&self, req: Request, admin: bool) -> Result<Call> {
        let remote = match &req {
            Request::Query { scan, .. }
            | Request::Read { scan, .. }
            | Request::Entries { scan, .. }
            | Request::Forget { scan } => *scan >= BROKER_SCAN_BASE,
            _ => admin && !system::is_elevated(),
        };
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        if remote {
            let broker = self
                .broker
                .lock()
                .clone()
                .filter(|b| b.alive())
                .ok_or(Error::NeedsAdmin)?;
            let rx = self.router.register(id);
            if let Err(e) = broker.send(id, &req) {
                self.router.done(id, Err(ErrorPayload::from(&e)));
                return Err(e);
            }
            let (b, cid) = (broker.clone(), self.next.fetch_add(1, Ordering::Relaxed));
            return Ok(Call {
                id,
                rx,
                cancel: Arc::new(move || {
                    let _ = b.send(cid, &Request::Cancel { id });
                }),
            });
        }
        let rx = self.router.register(id);
        let (session, router) = (self.session.clone(), self.router.clone());
        std::thread::spawn(move || {
            let sink = RouterSink(router.clone());
            let r = session.handle(id, req, &sink);
            router.done(id, r.map_err(|e| ErrorPayload::from(&e)));
        });
        let session = self.session.clone();
        Ok(Call {
            id,
            rx,
            cancel: Arc::new(move || session.cancel(id)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Query, ReadResult, ScanSummary};
    use crate::sources::SourceSpec;

    #[test]
    fn local_calls_stream_and_cancel_on_write_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let (path, photo) = crate::session::tests_support::image(tmp.path());
        let engine = Engine::new();
        let source = SourceSpec::Image {
            path: path.to_string_lossy().into(),
            partition: None,
        };
        let mut ticks = 0;
        let summary: ScanSummary = serde_json::from_value(
            engine
                .call(Request::Scan { source }, false)
                .unwrap()
                .wait(|_| ticks += 1, |_| Ok(()))
                .unwrap(),
        )
        .unwrap();
        assert!(summary.scan < BROKER_SCAN_BASE);
        let page = engine
            .call(
                Request::Query {
                    scan: summary.scan,
                    query: Query::default(),
                },
                false,
            )
            .unwrap()
            .wait(|_| {}, |_| Ok(()))
            .unwrap();
        assert_eq!(page["total"], 3);

        let mut got = Vec::new();
        let r: ReadResult = serde_json::from_value(
            engine
                .call(
                    Request::Read {
                        scan: summary.scan,
                        record: 40,
                    },
                    false,
                )
                .unwrap()
                .wait(
                    |_| {},
                    |b| {
                        got.extend_from_slice(b);
                        Ok(())
                    },
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(got, photo);
        assert_eq!(r.stats.bytes, photo.len() as u64);

        // A destination write error cancels the read and surfaces that error.
        let r = engine
            .call(
                Request::Read {
                    scan: summary.scan,
                    record: 40,
                },
                false,
            )
            .unwrap()
            .wait(|_| {}, |_| Err(Error::Other("disk full".into())));
        assert!(matches!(r, Err(Error::Other(m)) if m == "disk full"));

        // Broker scans need the broker.
        let r = engine.call(
            Request::Query {
                scan: BROKER_SCAN_BASE + 1,
                query: Query::default(),
            },
            false,
        );
        assert!(matches!(r, Err(Error::NeedsAdmin)));
    }
}
