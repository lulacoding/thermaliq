//! Lifecycle lines for `service.log` (SPEC §5.1): start, every way the service stops, and the API
//! server ending. The service redirects stderr to that file first thing, so these go to stderr.
//! A process that vanishes with no line here was killed from outside or crashed natively.

use std::io::Write;
use std::net::SocketAddr;
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    ScmStop,
    Shutdown,
    /// The control handler went away without a stop: the process is being torn down.
    HandlerDropped,
}

/// `YYYY-MM-DDTHH:MM:SSZ` for seconds since the Unix epoch.
pub fn stamp(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from a day count (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn line(w: &mut dyn Write, msg: &str) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = writeln!(w, "{} lifecycle: {msg}", stamp(now));
    let _ = w.flush();
}

pub fn started(w: &mut dyn Write, version: &str, pid: u32) {
    line(
        w,
        &format!("service starting (version {version}, pid {pid})"),
    );
}

pub fn running(w: &mut dyn Write, api: SocketAddr) {
    line(w, &format!("service running, API on {api}"));
}

pub fn startup_failed(w: &mut dyn Write, why: &str) {
    line(w, &format!("startup failed: {why}"));
}

pub fn stop_requested(w: &mut dyn Write, reason: StopReason) {
    line(
        w,
        match reason {
            StopReason::ScmStop => "stop requested by the service manager",
            StopReason::Shutdown => "the system is shutting down",
            StopReason::HandlerDropped => {
                "the service control handler was dropped; stopping (the process is being torn down)"
            }
        },
    );
}

/// How the stop ended. An `Err` is the engine not stopping inside its budget.
pub fn stopped(w: &mut dyn Write, result: &Result<(), String>) {
    match result {
        Ok(()) => line(w, "stopped cleanly, fans handed back"),
        Err(e) => line(w, &format!("STOP FAILED: {e}")),
    }
}

/// The API server's task ended. `asked`: we told it to stop.
pub fn api_exited(w: &mut dyn Write, asked: bool, result: &std::io::Result<()>) {
    match (asked, result) {
        (true, _) => line(w, "API server stopped"),
        (false, Ok(())) => line(w, "UNEXPECTED: the API server exited on its own"),
        (false, Err(e)) => line(w, &format!("UNEXPECTED: the API server failed: {e}")),
    }
}

/// Blocks until the control handler asks for a stop. A dropped handler also means stop.
pub fn wait_for_stop(rx: &Receiver<StopReason>) -> StopReason {
    rx.recv().unwrap_or(StopReason::HandlerDropped)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn text(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_time_stamp_is_utc_iso_8601() {
        assert_eq!(stamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(stamp(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(stamp(951_782_400), "2000-02-29T00:00:00Z", "a leap day");
    }

    #[test]
    fn every_line_is_stamped_and_says_what_happened() {
        let l = text(|w| started(w, "2.0.0-dev.0", 4242));
        assert!(
            l.contains("lifecycle") && l.contains("starting") && l.contains("4242"),
            "{l}"
        );
        assert!(l.contains("2.0.0-dev.0") && l.ends_with('\n'), "{l}");
        assert!(l.as_bytes()[4] == b'-' && l.contains("Z "), "stamped: {l}");
        let l = text(|w| running(w, "127.0.0.1:7272".parse().unwrap()));
        assert!(l.contains("running") && l.contains("127.0.0.1:7272"), "{l}");
    }

    #[test]
    fn each_way_to_stop_has_its_own_reason() {
        let lines: Vec<String> = [
            StopReason::ScmStop,
            StopReason::Shutdown,
            StopReason::HandlerDropped,
        ]
        .into_iter()
        .map(|r| text(|w| stop_requested(w, r)))
        .collect();
        assert!(
            lines[0].contains("service manager") && lines[0].contains("stop"),
            "{}",
            lines[0]
        );
        assert!(lines[1].contains("shutting down"), "{}", lines[1]);
        assert!(lines[2].contains("dropped"), "{}", lines[2]);
    }

    #[test]
    fn a_stop_reason_comes_from_the_handler_or_a_dropped_handler() {
        let (tx, rx) = mpsc::channel();
        tx.send(StopReason::Shutdown).unwrap();
        assert_eq!(wait_for_stop(&rx), StopReason::Shutdown);
        drop(tx);
        assert_eq!(wait_for_stop(&rx), StopReason::HandlerDropped);
    }

    #[test]
    fn startup_failures_and_stop_outcomes_are_logged_with_the_error() {
        let l = text(|w| startup_failed(w, "cannot listen on 127.0.0.1:7272"));
        assert!(
            l.contains("startup failed") && l.contains("cannot listen"),
            "{l}"
        );
        assert!(text(|w| stopped(w, &Ok(()))).contains("stopped cleanly"));
        let timeout =
            Err("engine did not stop in time; fans may not have been handed back".to_owned());
        let l = text(|w| stopped(w, &timeout));
        assert!(
            l.contains("STOP FAILED") && l.contains("did not stop in time"),
            "{l}"
        );
    }

    #[test]
    fn the_api_server_ending_on_its_own_is_loud_and_a_requested_stop_is_not() {
        let asked = text(|w| api_exited(w, true, &Ok(())));
        assert!(
            asked.contains("stopped") && !asked.contains("UNEXPECTED"),
            "{asked}"
        );
        let own = text(|w| api_exited(w, false, &Ok(())));
        assert!(
            own.contains("UNEXPECTED") && own.contains("API server"),
            "{own}"
        );
        let err = std::io::Error::other("accept failed");
        let l = text(|w| api_exited(w, false, &Err(err)));
        assert!(
            l.contains("UNEXPECTED") && l.contains("accept failed"),
            "{l}"
        );
    }
}
