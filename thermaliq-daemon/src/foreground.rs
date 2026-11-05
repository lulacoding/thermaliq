//! What's in the foreground (SPEC §7, inputs: the exe name, never window titles). A service in
//! session 0 can't see the desktop, so a small helper (this same exe) runs in the user's session
//! and prints one JSON line a second. It only reads; it can't touch fans. It ends by itself when
//! its pipe breaks, which is when the daemon has gone. Silence means unknown, and unknown keeps
//! the local model out and decides nothing.

use serde::Deserialize;
use serde_json::json;
use std::io::{self, BufRead, BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use thermaliq_agent::decision::Foreground;

/// A reading older than this isn't believed.
const FRESH: Duration = Duration::from_secs(3);
/// While nobody is logged on, or after a helper ended.
const RETRY: Duration = Duration::from_secs(5);
const MAX_LINE: u64 = 4096;
pub const HELPER_ARG: &str = "foreground-helper";

#[derive(Deserialize)]
struct Line {
    exe: Option<String>,
    fullscreen: bool,
}

fn parse_line(line: &str) -> Option<Foreground> {
    let l: Line = serde_json::from_str(line).ok()?;
    Some(Foreground {
        exe: l.exe,
        fullscreen: l.fullscreen,
    })
}

fn format_line(f: &Foreground) -> String {
    json!({"exe": f.exe, "fullscreen": f.fullscreen}).to_string()
}

/// `thermaliq-daemon foreground-helper`: runs until nobody is reading.
pub fn run_helper() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::io::Write;
        let mut out = io::stdout();
        loop {
            let (exe, fullscreen) = crate::win::foreground_now();
            let line = format_line(&Foreground { exe, fullscreen });
            if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    #[cfg(not(windows))]
    Err("the foreground helper is only available on Windows".into())
}

/// A running helper: where it prints, and how to end it.
pub struct Helper {
    pub stdout: Box<dyn Read + Send>,
    pub stop: Box<dyn FnOnce() + Send>,
}

type Stopper = Box<dyn FnOnce() + Send>;

/// Where the daemon gets the foreground app from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForegroundSource {
    /// Tests, and platforms without a desktop to read.
    Off,
    /// Run from a console: the helper shares our session.
    SameSession,
    /// The Windows service: the helper runs in the console user's session.
    ActiveUserSession,
}

pub struct Feed {
    latest: Arc<Mutex<Option<(Instant, Foreground)>>>,
    fresh: Duration,
    stopping: Arc<AtomicBool>,
    stopper: Arc<Mutex<Option<Stopper>>>,
    wake: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Feed {
    pub fn off() -> Self {
        Self {
            latest: Arc::default(),
            fresh: FRESH,
            stopping: Arc::default(),
            stopper: Arc::default(),
            wake: None,
            thread: None,
        }
    }

    pub fn from_source(source: ForegroundSource) -> Self {
        match spawner(source) {
            Some(spawn) => Self::start(FRESH, RETRY, spawn),
            None => Self::off(),
        }
    }

    pub fn start(
        fresh: Duration,
        retry: Duration,
        spawn: Box<dyn Fn() -> io::Result<Helper> + Send>,
    ) -> Self {
        let mut feed = Self::off();
        feed.fresh = fresh;
        let (wake, woken) = mpsc::channel();
        feed.wake = Some(wake);
        let (latest, stopping, stopper) = (
            feed.latest.clone(),
            feed.stopping.clone(),
            feed.stopper.clone(),
        );
        feed.thread = Some(std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                if let Ok(helper) = spawn() {
                    *stopper.lock().unwrap_or_else(PoisonError::into_inner) = Some(helper.stop);
                    if stopping.load(Ordering::SeqCst) {
                        stop_helper(&stopper);
                    }
                    read_lines(helper.stdout, &latest);
                    stop_helper(&stopper);
                }
                if !matches!(
                    woken.recv_timeout(retry),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    return;
                }
            }
        }));
        feed
    }

    /// The foreground app, or `None` if the helper hasn't spoken lately.
    pub fn latest(&self) -> Option<Foreground> {
        self.latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .filter(|(at, _)| at.elapsed() < self.fresh)
            .map(|(_, f)| f.clone())
    }

    /// Ends the helper and the thread.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        stop_helper(&self.stopper);
        self.wake = None; // wakes the retry wait
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn stop_helper(stopper: &Mutex<Option<Stopper>>) {
    let stop = stopper
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(stop) = stop {
        stop();
    }
}

fn read_lines(stdout: Box<dyn Read + Send>, latest: &Mutex<Option<(Instant, Foreground)>>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.by_ref().take(MAX_LINE).read_line(&mut line) {
            Ok(n) if n > 0 => {
                if let Some(f) = parse_line(line.trim()) {
                    *latest.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some((Instant::now(), f));
                }
            }
            _ => return,
        }
    }
}

#[cfg(windows)]
fn spawner(source: ForegroundSource) -> Option<Box<dyn Fn() -> io::Result<Helper> + Send>> {
    match source {
        ForegroundSource::Off => None,
        ForegroundSource::SameSession => Some(Box::new(|| {
            use std::os::windows::process::CommandExt;
            let mut child = std::process::Command::new(std::env::current_exe()?)
                .arg(HELPER_ARG)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                .spawn()?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("no stdout"))?;
            Ok(Helper {
                stdout: Box::new(stdout),
                stop: Box::new(move || {
                    let _ = child.kill();
                    let _ = child.wait();
                }),
            })
        })),
        ForegroundSource::ActiveUserSession => Some(Box::new(|| {
            let (stdout, child) =
                crate::win::spawn_in_user_session(&std::env::current_exe()?, HELPER_ARG)?;
            Ok(Helper {
                stdout: Box::new(stdout),
                stop: Box::new(move || child.kill()),
            })
        })),
    }
}

#[cfg(not(windows))]
fn spawner(_: ForegroundSource) -> Option<Box<dyn Fn() -> io::Result<Helper> + Send>> {
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn a_line_parses_or_is_dropped() {
        assert_eq!(
            parse_line(r#"{"exe":"cs2.exe","fullscreen":true}"#),
            Some(Foreground {
                exe: Some("cs2.exe".into()),
                fullscreen: true
            })
        );
        assert_eq!(
            parse_line(r#"{"exe":null,"fullscreen":false}"#),
            Some(Foreground {
                exe: None,
                fullscreen: false
            })
        );
        for bad in [
            "",
            "junk",
            r#"{"exe":"a.exe"}"#,
            r#"{"fullscreen":"yes","exe":null}"#,
        ] {
            assert_eq!(parse_line(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_helpers_line_is_what_the_parser_reads() {
        let f = Foreground {
            exe: Some("a \"b\".exe".into()),
            fullscreen: true,
        };
        assert_eq!(parse_line(&format_line(&f)), Some(f));
    }

    #[test]
    fn a_helper_that_stops_talking_goes_stale_and_is_started_again() {
        let starts = Arc::new(AtomicU32::new(0));
        let counted = starts.clone();
        let feed = Feed::start(
            Duration::from_millis(50),
            Duration::from_millis(100),
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                let line = format_line(&Foreground {
                    exe: Some("a.exe".into()),
                    fullscreen: false,
                });
                Ok(Helper {
                    stdout: Box::new(Cursor::new(format!("{line}\n").into_bytes())),
                    stop: Box::new(|| {}),
                })
            }),
        );
        let end = Instant::now() + Duration::from_secs(5);
        while starts.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < end, "never restarted");
            std::thread::sleep(Duration::from_millis(5));
        }
        feed.stop();
    }

    #[test]
    fn the_latest_reading_is_served_while_fresh_then_unknown() {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        struct Pipe(std::sync::mpsc::Receiver<Vec<u8>>);
        impl std::io::Read for Pipe {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let Ok(b) = self.0.recv() else { return Ok(0) };
                buf[..b.len()].copy_from_slice(&b);
                Ok(b.len())
            }
        }
        let rx = Mutex::new(Some(rx));
        let feed = Feed::start(
            Duration::from_millis(200),
            Duration::from_secs(60),
            Box::new(move || {
                let rx = rx
                    .lock()
                    .unwrap()
                    .take()
                    .ok_or_else(|| std::io::Error::other("only once"))?;
                Ok(Helper {
                    stdout: Box::new(Pipe(rx)),
                    stop: Box::new(|| {}),
                })
            }),
        );
        assert_eq!(feed.latest(), None, "nothing yet");
        tx.send(b"{\"exe\":\"a.exe\",\"fullscreen\":false}\n".to_vec())
            .unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while feed.latest().is_none() {
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(feed.latest().unwrap().exe.as_deref(), Some("a.exe"));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(feed.latest(), None, "stale");
        drop(tx);
        feed.stop();
    }

    #[test]
    fn stopping_ends_a_helper_that_is_still_talking() {
        let stopped = Arc::new(AtomicU32::new(0));
        let counted = stopped.clone();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        struct Blocks(std::sync::mpsc::Receiver<()>);
        impl std::io::Read for Blocks {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                let _ = self.0.recv();
                Ok(0)
            }
        }
        let rx = Mutex::new(Some(rx));
        let tx = Mutex::new(Some(tx));
        let feed = Feed::start(
            Duration::from_secs(1),
            Duration::from_secs(60),
            Box::new(move || {
                let rx = rx
                    .lock()
                    .unwrap()
                    .take()
                    .ok_or_else(|| std::io::Error::other("only once"))?;
                let tx = tx.lock().unwrap().take();
                let counted = counted.clone();
                Ok(Helper {
                    stdout: Box::new(Blocks(rx)),
                    stop: Box::new(move || {
                        counted.fetch_add(1, Ordering::SeqCst);
                        drop(tx); // as killing the helper closes its stdout
                    }),
                })
            }),
        );
        std::thread::sleep(Duration::from_millis(100));
        feed.stop();
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn with_no_foreground_source_it_is_always_unknown() {
        assert_eq!(Feed::off().latest(), None);
    }
}
