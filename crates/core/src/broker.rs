//! The elevated broker: the same exe started with `--broker` through UAC.
//!
//! It accepts nothing but a pipe name and the UI's process id on its command line, refuses to
//! serve another user than the one running the UI, connects to the UI's pipes, and answers
//! requests with the shared [`Session`]. It never writes files; content goes back over the pipe
//! and the non-elevated UI writes it. It exits when the pipe closes or the UI process ends.

use std::fs::File;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use crate::engine::{BROKER_SCAN_BASE, Router};
use crate::error::ErrorPayload;
use crate::job::ProgressSnapshot;
use crate::pipe::{self, Direction, Server};
use crate::protocol::{
    Envelope, KIND_DATA, KIND_ERR, KIND_OK, KIND_PROGRESS, KIND_REQUEST, Request, read_frame,
    write_frame,
};
use crate::session::{Session, Sink};
use crate::system::{self, Process};
use crate::{Error, Result};

pub const FLAG: &str = "--broker";

pub const EXIT_OK: i32 = 0;
pub const EXIT_BAD_ARGS: i32 = 2;
pub const EXIT_OTHER_USER: i32 = 3;
pub const EXIT_PIPE: i32 = 4;

fn arg<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

/// Broker entry point. Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let (Some(base), Some(pid)) = (
        arg(args, "--pipe"),
        arg(args, "--ui-pid").and_then(|p| p.parse::<u32>().ok()),
    ) else {
        return EXIT_BAD_ARGS;
    };
    if !pipe::valid_base(base) {
        return EXIT_BAD_ARGS;
    }
    // Over-the-shoulder elevation (a different administrator account) would let a standard user
    // read raw disks through us. The pipe DACL blocks it too; this gives a clear exit code.
    match (system::current_user_sid(), system::process_user_sid(pid)) {
        (Ok(me), Ok(ui)) if me == ui => {}
        _ => return EXIT_OTHER_USER,
    }
    let (mut req, evt) = match (
        pipe::connect(base, Direction::ToClient, pid),
        pipe::connect(base, Direction::ToServer, pid),
    ) {
        (Ok(r), Ok(e)) => (r, e),
        (Err(Error::OtherUser), _) | (_, Err(Error::OtherUser)) => return EXIT_OTHER_USER,
        _ => return EXIT_PIPE,
    };
    std::thread::spawn(move || {
        while system::process_alive(pid) {
            std::thread::sleep(Duration::from_secs(1));
        }
        std::process::exit(EXIT_OK);
    });
    let sink = Arc::new(PipeSink {
        evt: Mutex::new(evt),
    });
    let session = Arc::new(Session::with_scan_base(BROKER_SCAN_BASE));
    loop {
        let frame = match read_frame(&mut req) {
            Ok(Some(f)) => f,
            _ => return EXIT_OK,
        };
        let (kind, id, payload) = frame;
        if kind != KIND_REQUEST {
            return EXIT_PIPE;
        }
        let Ok(env) = serde_json::from_slice::<Envelope>(&payload) else {
            sink.err(id, &Error::Refused("malformed request".into()));
            continue;
        };
        if env.id != id {
            sink.err(id, &Error::Refused("request id mismatch".into()));
            continue;
        }
        if let Request::Cancel { id: target } = env.req {
            session.cancel(target);
            continue;
        }
        let (session, sink) = (session.clone(), sink.clone());
        std::thread::spawn(move || match session.handle(id, env.req, sink.as_ref()) {
            Ok(v) => sink.ok(id, &v),
            Err(e) => sink.err(id, &e),
        });
    }
}

struct PipeSink {
    evt: Mutex<File>,
}

impl PipeSink {
    fn send(&self, kind: u8, id: u64, payload: &[u8]) -> std::io::Result<()> {
        write_frame(&mut *self.evt.lock(), kind, id, payload)
    }
    fn ok(&self, id: u64, v: &serde_json::Value) {
        let _ = self.send(KIND_OK, id, &serde_json::to_vec(v).unwrap_or_default());
    }
    fn err(&self, id: u64, e: &Error) {
        let _ = self.send(
            KIND_ERR,
            id,
            &serde_json::to_vec(&ErrorPayload::from(e)).unwrap_or_default(),
        );
    }
}

impl Sink for PipeSink {
    fn progress(&self, id: u64, p: ProgressSnapshot) {
        let _ = self.send(
            KIND_PROGRESS,
            id,
            &serde_json::to_vec(&p).unwrap_or_default(),
        );
    }
    fn data(&self, id: u64, bytes: &[u8]) -> Result<()> {
        self.send(KIND_DATA, id, bytes)
            .map_err(|e| Error::Broker(e.to_string()))
    }
}

/// The UI's handle on a running broker.
pub struct BrokerClient {
    req: Mutex<File>,
    alive: Arc<AtomicBool>,
    _process: Process,
}

impl BrokerClient {
    /// Creates the pipes, starts the broker through UAC and connects. `Ok(None)` when the user
    /// declined the UAC prompt.
    pub fn launch(router: Arc<Router>) -> Result<Option<Arc<Self>>> {
        let sid = system::current_user_sid()?;
        let base = pipe::random_base();
        let req_server = Server::create(&base, Direction::ToClient, &sid)?;
        let evt_server = Server::create(&base, Direction::ToServer, &sid)?;
        let exe = std::env::current_exe().map_err(|e| Error::io("current_exe", e))?;
        let params = format!("{FLAG} --pipe {base} --ui-pid {}", std::process::id());
        let Some(process) = system::launch_elevated(&exe, &params)? else {
            return Ok(None);
        };
        let connected = req_server
            .accept(&process)
            .and_then(|r| evt_server.accept(&process).map(|e| (r, e)));
        let (req, mut evt) = match connected {
            Ok(pair) => pair,
            Err(e) => {
                process.wait(2000);
                return Err(match process.exit_code() {
                    Some(c) if c as i32 == EXIT_OTHER_USER => Error::OtherUser,
                    _ => e,
                });
            }
        };
        let alive = Arc::new(AtomicBool::new(true));
        let flag = alive.clone();
        std::thread::spawn(move || {
            while let Ok(Some((kind, id, payload))) = read_frame(&mut evt) {
                match kind {
                    KIND_PROGRESS => {
                        if let Ok(p) = serde_json::from_slice(&payload) {
                            router.progress(id, p);
                        }
                    }
                    KIND_DATA => {
                        let _ = router.data(id, payload);
                    }
                    KIND_OK => router.done(
                        id,
                        serde_json::from_slice(&payload)
                            .map_err(|e| ErrorPayload::from(&Error::from(e))),
                    ),
                    KIND_ERR => router.done(
                        id,
                        Err(serde_json::from_slice(&payload).unwrap_or(ErrorPayload {
                            code: "broker".into(),
                            message: "unreadable error from the broker".into(),
                        })),
                    ),
                    _ => break,
                }
            }
            flag.store(false, Ordering::Relaxed);
            router.fail_all(&Error::Broker("the administrator helper stopped".into()));
        });
        Ok(Some(Arc::new(BrokerClient {
            req: Mutex::new(req),
            alive,
            _process: process,
        })))
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn send(&self, id: u64, req: &Request) -> Result<()> {
        let payload = serde_json::to_vec(&Envelope {
            id,
            req: req.clone(),
        })?;
        write_frame(&mut *self.req.lock(), KIND_REQUEST, id, &payload).map_err(|e| {
            self.alive.store(false, Ordering::Relaxed);
            Error::Broker(e.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_bad_arguments() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(run(&a(&[FLAG])), EXIT_BAD_ARGS);
        assert_eq!(
            run(&a(&[FLAG, "--pipe", "evil", "--ui-pid", "1"])),
            EXIT_BAD_ARGS
        );
        assert_eq!(
            run(&a(&[FLAG, "--pipe", &pipe::random_base(), "--ui-pid", "x"])),
            EXIT_BAD_ARGS
        );
        // PID 4 is the System process: a different user, so the broker refuses to serve it.
        assert_eq!(
            run(&a(&[FLAG, "--pipe", &pipe::random_base(), "--ui-pid", "4"])),
            EXIT_OTHER_USER
        );
    }
}
