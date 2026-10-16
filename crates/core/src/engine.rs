//! The UI's view of the engine: one call API, two backends. Image files are scanned in this
//! process; raw volumes and shadow copies go to the elevated broker. Results come back as a
//! stream of events per call, with backpressure so a fast reader cannot flood memory.
//!
//! Scan results always end up in this process's session databases. A broker scan's findings
//! arrive as data events and are written here as they come; queries and selections are then
//! answered here for every scan, and only reads of a broker scan go back to the broker.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use parking_lot::Mutex;
use serde_json::Value;

use crate::broker::BrokerClient;
use crate::error::ErrorPayload;
use crate::job::ProgressSnapshot;
use crate::protocol::{Request, ScanSummary};
use crate::session::{Session, Sink};
use crate::sources::SourceSpec;
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

fn is_broker_scan(scan: u32) -> bool {
    scan >= BROKER_SCAN_BASE
}

impl Engine {
    /// `workspace` is the default folder for session databases; see [`crate::workspace`].
    pub fn new(workspace: impl Into<PathBuf>) -> Arc<Self> {
        Arc::new(Engine {
            session: Arc::new(Session::new(workspace)),
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

    /// Drops every scan and deletes its database; called when the app closes.
    pub fn forget_all(&self) {
        self.session.forget_all();
    }

    /// Sends a request. `admin` requests use the broker unless this process is elevated.
    pub fn call(&self, req: Request, admin: bool) -> Result<Call> {
        let to_broker = admin && !system::is_elevated();
        match req {
            Request::Scan { source, workspace } if to_broker => self.broker_scan(source, workspace),
            // Results of every scan live here.
            Request::Query { .. } | Request::Entries { .. } => Ok(self.local(req)),
            Request::Read { scan, record, .. } if is_broker_scan(scan) => {
                let sealed = self.session.sealed(scan, record)?;
                self.remote(&Request::Read {
                    scan,
                    record,
                    sealed: Some(sealed),
                })
            }
            Request::Forget { scan } => {
                if is_broker_scan(scan)
                    && let Some(b) = self.live_broker()
                {
                    // The broker only holds the source handle; a lost message costs nothing.
                    let _ = b.send(self.next_id(), &Request::Forget { scan });
                }
                Ok(self.local(req))
            }
            req if to_broker => self.remote(&req),
            req => Ok(self.local(req)),
        }
    }

    fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    fn live_broker(&self) -> Option<Arc<BrokerClient>> {
        self.broker.lock().clone().filter(|b| b.alive())
    }

    fn local(&self, req: Request) -> Call {
        let id = self.next_id();
        let rx = self.router.register(id);
        let (session, router) = (self.session.clone(), self.router.clone());
        std::thread::spawn(move || {
            let sink = RouterSink(router.clone());
            let r = session.handle(id, req, &sink);
            router.done(id, r.map_err(|e| ErrorPayload::from(&e)));
        });
        let session = self.session.clone();
        Call {
            id,
            rx,
            cancel: Arc::new(move || session.cancel(id)),
        }
    }

    fn remote(&self, req: &Request) -> Result<Call> {
        let broker = self.live_broker().ok_or(Error::NeedsAdmin)?;
        let id = self.next_id();
        let rx = self.router.register(id);
        if let Err(e) = broker.send(id, req) {
            self.router.done(id, Err(ErrorPayload::from(&e)));
            return Err(e);
        }
        let (b, cid) = (broker.clone(), self.next_id());
        Ok(Call {
            id,
            rx,
            cancel: Arc::new(move || {
                let _ = b.send(cid, &Request::Cancel { id });
            }),
        })
    }

    /// A scan in the broker, its findings written into a database here as they arrive. The
    /// caller sees the broker's progress and, at the end, a summary counted from that
    /// database.
    fn broker_scan(&self, source: SourceSpec, workspace: Option<String>) -> Result<Call> {
        let store = self
            .session
            .create_store(workspace.as_deref().map(std::path::Path::new))?;
        let call = self.remote(&Request::Scan {
            source,
            workspace: None,
        })?;
        let (id, cancel) = (call.id, call.canceller());
        let (tx, rx) = sync_channel(CHANNEL_DEPTH);
        let session = self.session.clone();
        std::thread::spawn(move || {
            let progress = tx.clone();
            let r = call
                .wait(
                    |p| {
                        let _ = progress.try_send(Event::Progress(p));
                    },
                    |batch| store.ingest(batch),
                )
                .and_then(|v| {
                    let mut summary: ScanSummary = serde_json::from_value(v)?;
                    if !is_broker_scan(summary.scan) {
                        return Err(Error::Broker(format!("scan id {}", summary.scan)));
                    }
                    // The database is the truth for what the UI shows.
                    (summary.files, summary.counts) = store.finish()?;
                    let scan = summary.scan;
                    session.adopt(scan, store);
                    Ok(serde_json::to_value(summary)?)
                });
            let _ = tx.send(Event::Done(r.map_err(|e| ErrorPayload::from(&e))));
        });
        Ok(Call { id, rx, cancel })
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
        let engine = Engine::new(tmp.path().join("sessions"));
        let source = SourceSpec::Image {
            path: path.to_string_lossy().into(),
            partition: None,
        };
        let mut ticks = 0;
        let summary: ScanSummary = serde_json::from_value(
            engine
                .call(
                    Request::Scan {
                        source,
                        workspace: None,
                    },
                    false,
                )
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

        let read = || Request::Read {
            scan: summary.scan,
            record: 40,
            sealed: None,
        };
        let mut got = Vec::new();
        let r: ReadResult = serde_json::from_value(
            engine
                .call(read(), false)
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
            .call(read(), false)
            .unwrap()
            .wait(|_| {}, |_| Err(Error::Other("disk full".into())));
        assert!(matches!(r, Err(Error::Other(m)) if m == "disk full"));

        // Broker scans need the broker for reads; their results would be here.
        let r = engine.call(
            Request::Read {
                scan: BROKER_SCAN_BASE + 1,
                record: 40,
                sealed: None,
            },
            false,
        );
        assert!(matches!(r, Err(Error::Refused(_))));
        let r = engine.call(
            Request::Scan {
                source: SourceSpec::Volume {
                    root: "C:\\".into(),
                },
                workspace: None,
            },
            true,
        );
        if !system::is_elevated() {
            assert!(matches!(r, Err(Error::NeedsAdmin)));
        }

        // Closing the app deletes the databases.
        engine.forget_all();
        assert!(!tmp.path().join("sessions").exists());
    }
}
