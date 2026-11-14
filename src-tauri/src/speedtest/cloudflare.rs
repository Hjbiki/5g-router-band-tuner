//! Built-in speed test against speed.cloudflare.com (the default provider).
//!
//! * Idle ping: tiny `GET /__down?bytes=0` requests on a warm connection, median.
//! * Download: parallel `GET /__down?bytes=N` streams, counted as the bytes arrive.
//! * Upload: parallel `POST /__up` streams, counted as the body is handed to the socket.
//! * Ping while busy: the same tiny request, repeated on its own connection while each
//!   transfer runs; the medians for download and upload are reported separately.
//!
//! Each transfer stops at its data budget or its time cap, whichever comes first, so a slow
//! link never runs for long. The base URL is injectable so tests use a local mock server.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::{FailReason, Live, SpeedPhase, SpeedTestSize};

pub const DEFAULT_BASE: &str = "https://speed.cloudflare.com";

/// How much data and time each phase may use.
#[derive(Debug, Clone)]
pub struct Plan {
    pub idle_pings: usize,
    /// All the idle pings together. A link still busy can take seconds to answer each one, and
    /// ten of those would be most of the test.
    pub idle_cap: Duration,
    pub down_bytes: u64,
    pub up_bytes: u64,
    pub down_cap: Duration,
    pub up_cap: Duration,
    pub streams: usize,
    pub down_chunk: u64,
    pub up_chunk: u64,
    pub probe_interval: Duration,
    pub probe_timeout: Duration,
    /// For the very first request (DNS, TCP and TLS). A link still busy from an earlier test
    /// can take seconds to answer, so this is longer than a probe and is tried a few times.
    pub connect_timeout: Duration,
    pub connect_tries: usize,
    pub tick: Duration,
}

impl Plan {
    /// Light is about 40 MB in total, Full about 200 MB.
    pub fn for_size(size: SpeedTestSize) -> Self {
        let base = Plan {
            idle_pings: 10,
            idle_cap: Duration::from_secs(5),
            down_bytes: 28_000_000,
            up_bytes: 10_000_000,
            down_cap: Duration::from_secs(10),
            up_cap: Duration::from_secs(10),
            streams: 4,
            down_chunk: 10_000_000,
            up_chunk: 2_000_000,
            probe_interval: Duration::from_millis(300),
            probe_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(12),
            connect_tries: 3,
            tick: Duration::from_millis(250),
        };
        match size {
            SpeedTestSize::Light => base,
            SpeedTestSize::Full => Plan {
                down_bytes: 150_000_000,
                up_bytes: 45_000_000,
                down_cap: Duration::from_secs(25),
                up_cap: Duration::from_secs(25),
                down_chunk: 25_000_000,
                up_chunk: 5_000_000,
                ..base
            },
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Measured {
    pub idle_ping_ms: Option<f64>,
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub ping_down_ms: Option<f64>,
    pub ping_up_ms: Option<f64>,
    pub bytes_used: u64,
    pub server: Option<String>,
}

/// Why the server couldn't be used, and the technical cause.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub reason: FailReason,
    pub detail: String,
}

#[derive(Debug, PartialEq)]
pub enum CfError {
    Cancelled,
    /// No usable reply from the speed-test server (offline, blocked, DNS, too slow).
    Unreachable(Failure),
}

/// Sorts a request error into a plain reason and keeps its whole cause chain (reqwest's own
/// message is only "error sending request"), without the URL.
pub fn classify(e: &reqwest::Error) -> Failure {
    let mut parts = vec![e.to_string()];
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        let t = s.to_string();
        if !parts.iter().any(|p| p.contains(&t)) {
            parts.push(t);
        }
        src = s.source();
    }
    let detail = parts.join(": ");
    let low = detail.to_ascii_lowercase();
    let reason = if e.is_timeout() || low.contains("timed out") || low.contains("timeout") {
        FailReason::Timeout
    } else if low.contains("dns") || low.contains("no such host") || low.contains("resolve") {
        FailReason::NoInternet
    } else if low.contains("tls") || low.contains("certificate") || low.contains("handshake") {
        FailReason::Tls
    } else if e.is_connect() || low.contains("refused") || low.contains("reset") {
        FailReason::Refused
    } else if e.status().is_some() {
        FailReason::Http
    } else {
        FailReason::Other
    };
    Failure { reason, detail }
}

/// Progress callback: phase, fraction of that phase done (0..1), data used so far, live values.
pub type Report<'a> = &'a (dyn Fn(SpeedPhase, f64, u64, &Live) + Send + Sync);

/// An HTTP client for the test. A client that can't be built (TLS backend, system config) ends
/// the test with an error instead of a panic.
pub fn client() -> Result<reqwest::Client, CfError> {
    client_with_agent(concat!("router-band-tuner/", env!("CARGO_PKG_VERSION")))
}

fn client_with_agent(agent: &str) -> Result<reqwest::Client, CfError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .user_agent(agent)
        .build()
        .map_err(|e| {
            CfError::Unreachable(Failure {
                reason: FailReason::Other,
                detail: format!("could not set up the HTTP client: {e}"),
            })
        })
}

pub fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let n = s.len();
    Some(if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    })
}

/// Cloudflare reports its own processing time as `cfRequestDuration;dur=<ms>`; subtracting it
/// leaves the network round trip.
fn server_ms(h: &reqwest::header::HeaderMap) -> f64 {
    h.get_all("server-timing")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .find(|p| p.trim_start().starts_with("cfRequestDuration"))
        .and_then(|p| p.split(';').find_map(|kv| kv.trim().strip_prefix("dur=")))
        .and_then(|d| d.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// The data-centre code at the end of `cf-ray` (for example `JED`), shown as the server.
fn colo(h: &reqwest::header::HeaderMap) -> Option<String> {
    let ray = h.get("cf-ray")?.to_str().ok()?;
    let code = ray.rsplit_once('-')?.1;
    (!code.is_empty() && code.len() <= 5 && code.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| code.to_ascii_uppercase())
}

/// One tiny request; returns the round trip in ms and the response headers.
async fn ping(
    c: &reqwest::Client,
    base: &str,
    timeout: Duration,
) -> Result<(f64, reqwest::header::HeaderMap), Failure> {
    let t0 = Instant::now();
    let r = c
        .get(format!("{base}/__down?bytes=0"))
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| classify(&e.without_url()))?;
    let rtt = t0.elapsed().as_secs_f64() * 1000.0;
    if !r.status().is_success() {
        return Err(Failure {
            reason: FailReason::Http,
            detail: format!("HTTP {}", r.status().as_u16()),
        });
    }
    let headers = r.headers().clone();
    let _ = r.bytes().await;
    Ok(((rtt - server_ms(&headers)).max(0.0), headers))
}

async fn cancelled(rx: &mut watch::Receiver<bool>) {
    // A dropped sender means nobody can cancel any more; wait forever then.
    if rx.wait_for(|c| *c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Throughput in Mbps from `(seconds, bytes)` samples, ignoring the first fifth of the window
/// (connection set-up and send-buffer fill) when there is enough of it.
pub fn throughput(samples: &[(f64, u64)]) -> Option<f64> {
    let &(t_end, b_end) = samples.last()?;
    if t_end <= 0.0 || b_end == 0 {
        return None;
    }
    let skip = t_end * 0.2;
    let (t0, b0) = samples
        .iter()
        .copied()
        .find(|&(t, _)| t >= skip)
        .filter(|&(t, _)| t_end - t >= 0.2)
        .unwrap_or((0.0, 0));
    let dt = t_end - t0;
    (dt > 0.0).then(|| (b_end - b0) as f64 * 8.0 / dt / 1e6)
}

/// Shared counters for one transfer phase.
struct Transfer {
    /// Bytes promised to a request (keeps the phase inside its budget).
    reserved: AtomicU64,
    /// Bytes actually moved.
    moved: AtomicU64,
    budget: u64,
    chunk: u64,
}

impl Transfer {
    fn reserve(&self) -> u64 {
        let mut cur = self.reserved.load(Ordering::Relaxed);
        loop {
            let take = self.chunk.min(self.budget.saturating_sub(cur));
            if take == 0 {
                return 0;
            }
            match self.reserved.compare_exchange(
                cur,
                cur + take,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return take,
                Err(now) => cur = now,
            }
        }
    }
}

async fn download_stream(c: reqwest::Client, base: String, tr: Arc<Transfer>) {
    loop {
        let n = tr.reserve();
        if n == 0 {
            return;
        }
        let Ok(mut r) = c.get(format!("{base}/__down?bytes={n}")).send().await else {
            return;
        };
        if !r.status().is_success() {
            return;
        }
        while let Ok(Some(chunk)) = r.chunk().await {
            tr.moved.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
    }
}

const UP_PIECE: usize = 64 * 1024;

async fn upload_stream(c: reqwest::Client, base: String, tr: Arc<Transfer>) {
    let piece = Bytes::from(vec![0u8; UP_PIECE]);
    loop {
        let n = tr.reserve();
        if n == 0 {
            return;
        }
        // Counted as hyper pulls each piece, so a slow link still shows progress mid-request.
        let tr2 = tr.clone();
        let piece = piece.clone();
        let body = futures_util::stream::unfold(n, move |left| {
            let tr = tr2.clone();
            let piece = piece.clone();
            async move {
                if left == 0 {
                    return None;
                }
                let take = (left as usize).min(UP_PIECE);
                tr.moved.fetch_add(take as u64, Ordering::Relaxed);
                Some((
                    Ok::<Bytes, std::io::Error>(piece.slice(0..take)),
                    left - take as u64,
                ))
            }
        });
        let res = c
            .post(format!("{base}/__up"))
            .header(reqwest::header::CONTENT_LENGTH, n)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(reqwest::Body::wrap_stream(body))
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {
                let _ = r.bytes().await;
            }
            _ => return,
        }
    }
}

/// Probes latency on its own connection until aborted.
async fn probe_loop(
    c: reqwest::Client,
    base: String,
    every: Duration,
    timeout: Duration,
    out: Arc<Mutex<Vec<f64>>>,
) {
    loop {
        let t0 = Instant::now();
        match ping(&c, &base, timeout).await {
            Ok((ms, _)) => out.lock().unwrap().push(ms),
            // A probe that never came back counts as the timeout: the link was that busy.
            Err(_) => out.lock().unwrap().push(timeout.as_secs_f64() * 1000.0),
        }
        if let Some(rest) = every.checked_sub(t0.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }
}

struct PhaseOut {
    mbps: Option<f64>,
    loaded_ms: Option<f64>,
    moved: u64,
}

#[allow(clippy::too_many_arguments)]
async fn transfer_phase(
    phase: SpeedPhase,
    base: &str,
    plan: &Plan,
    budget: u64,
    chunk: u64,
    cap: Duration,
    cancel: &mut watch::Receiver<bool>,
    report: Report<'_>,
    used_before: u64,
    live: &mut Live,
) -> Result<PhaseOut, CfError> {
    let tr = Arc::new(Transfer {
        reserved: AtomicU64::new(0),
        moved: AtomicU64::new(0),
        budget,
        chunk,
    });
    let mut streams = JoinSet::new();
    for _ in 0..plan.streams.max(1) {
        // One client per stream: each transfer gets its own connection.
        let c = client()?;
        let b = base.to_string();
        let t = tr.clone();
        match phase {
            SpeedPhase::Upload => streams.spawn(upload_stream(c, b, t)),
            _ => streams.spawn(download_stream(c, b, t)),
        };
    }
    let probes = Arc::new(Mutex::new(Vec::new()));
    let mut prober = JoinSet::new();
    prober.spawn(probe_loop(
        client()?,
        base.to_string(),
        plan.probe_interval,
        plan.probe_timeout,
        probes.clone(),
    ));

    let start = Instant::now();
    let deadline = tokio::time::Instant::now() + cap;
    let mut samples: Vec<(f64, u64)> = Vec::new();
    let mut tick = tokio::time::interval(plan.tick);
    let update = |samples: &mut Vec<(f64, u64)>, live: &mut Live| {
        let moved = tr.moved.load(Ordering::Relaxed);
        let el = start.elapsed();
        samples.push((el.as_secs_f64(), moved));
        let mbps = throughput(samples);
        let loaded = median(&probes.lock().unwrap());
        match phase {
            SpeedPhase::Upload => {
                live.upload_mbps = mbps;
                live.ping_up_ms = loaded;
            }
            _ => {
                live.download_mbps = mbps;
                live.ping_down_ms = loaded;
            }
        }
        let frac = (moved as f64 / budget.max(1) as f64)
            .max(el.as_secs_f64() / cap.as_secs_f64())
            .min(1.0);
        report(phase, frac, used_before + moved, live);
        (mbps, loaded, moved)
    };
    loop {
        tokio::select! {
            _ = tick.tick() => { update(&mut samples, live); }
            r = streams.join_next() => if r.is_none() { break },
            _ = tokio::time::sleep_until(deadline) => break,
            _ = cancelled(cancel) => {
                streams.abort_all();
                prober.abort_all();
                return Err(CfError::Cancelled);
            }
        }
    }
    streams.abort_all();
    // A fast link can finish before the first probe is back, and that answer is still wanted. A
    // probe always reports by its timeout, even if only that it timed out.
    let probe_end = tokio::time::Instant::now() + plan.probe_timeout + Duration::from_secs(1);
    while probes.lock().unwrap().is_empty() && tokio::time::Instant::now() < probe_end {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
            _ = cancelled(cancel) => {
                prober.abort_all();
                return Err(CfError::Cancelled);
            }
        }
    }
    prober.abort_all();
    let (mbps, loaded, moved) = update(&mut samples, live);
    Ok(PhaseOut {
        mbps,
        loaded_ms: loaded,
        moved,
    })
}

/// Runs the whole test. `report` gets live values about four times a second.
pub async fn run(
    base: &str,
    plan: &Plan,
    mut cancel: watch::Receiver<bool>,
    report: Report<'_>,
) -> Result<Measured, CfError> {
    let base = base.trim_end_matches('/');
    let mut live = Live::default();
    let mut m = Measured::default();
    if *cancel.borrow() {
        return Err(CfError::Cancelled);
    }

    // Idle ping. The first request opens the connection (DNS, TCP, TLS) and is not counted.
    report(SpeedPhase::IdlePing, 0.0, 0, &live);
    let c = client()?;
    let mut first = Err(Failure {
        reason: FailReason::Other,
        detail: "no attempt".into(),
    });
    for attempt in 0..plan.connect_tries.max(1) {
        if attempt > 0 {
            // Give a busy link a moment to drain before trying again.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = cancelled(&mut cancel) => return Err(CfError::Cancelled),
            }
        }
        first = tokio::select! {
            r = ping(&c, base, plan.connect_timeout) => r,
            _ = cancelled(&mut cancel) => return Err(CfError::Cancelled),
        };
        if first.is_ok() {
            break;
        }
    }
    let headers = first.map_err(CfError::Unreachable)?.1;
    m.server = colo(&headers);
    let mut pings = Vec::new();
    let idle_end = tokio::time::Instant::now() + plan.idle_cap;
    for i in 0..plan.idle_pings {
        let r = tokio::select! {
            r = ping(&c, base, plan.probe_timeout) => r,
            _ = tokio::time::sleep_until(idle_end) => break,
            _ = cancelled(&mut cancel) => return Err(CfError::Cancelled),
        };
        if let Ok((ms, _)) = r {
            pings.push(ms);
        }
        live.idle_ping_ms = median(&pings);
        report(
            SpeedPhase::IdlePing,
            (i + 1) as f64 / plan.idle_pings as f64,
            0,
            &live,
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    m.idle_ping_ms = median(&pings);

    let down = transfer_phase(
        SpeedPhase::Download,
        base,
        plan,
        plan.down_bytes,
        plan.down_chunk,
        plan.down_cap,
        &mut cancel,
        report,
        0,
        &mut live,
    )
    .await?;
    m.download_mbps = down.mbps;
    m.ping_down_ms = down.loaded_ms;

    let up = transfer_phase(
        SpeedPhase::Upload,
        base,
        plan,
        plan.up_bytes,
        plan.up_chunk,
        plan.up_cap,
        &mut cancel,
        report,
        down.moved,
        &mut live,
    )
    .await?;
    m.upload_mbps = up.mbps;
    m.ping_up_ms = up.loaded_ms;
    m.bytes_used = down.moved + up.moved;
    report(SpeedPhase::Done, 1.0, m.bytes_used, &live);
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    /// Answers `__down?bytes=N` with N bytes, like the real endpoint.
    struct Down(Duration);
    impl Respond for Down {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let n: usize = req
                .url
                .query_pairs()
                .find(|(k, _)| k == "bytes")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            let t = ResponseTemplate::new(200)
                .insert_header("server-timing", "cfRequestDuration;dur=1.5")
                .insert_header("cf-ray", "8f00aa11bb22cc33-JED")
                .set_body_bytes(vec![0u8; n]);
            // Only the big transfers are slow; pings stay fast.
            if n > 0 {
                t.set_delay(self.0)
            } else {
                t
            }
        }
    }

    fn small_plan() -> Plan {
        Plan {
            idle_pings: 3,
            idle_cap: Duration::from_secs(4),
            down_bytes: 400_000,
            up_bytes: 300_000,
            down_cap: Duration::from_secs(5),
            up_cap: Duration::from_secs(5),
            streams: 2,
            down_chunk: 100_000,
            up_chunk: 100_000,
            probe_interval: Duration::from_millis(20),
            probe_timeout: Duration::from_secs(2),
            connect_timeout: Duration::from_millis(400),
            connect_tries: 2,
            tick: Duration::from_millis(20),
        }
    }

    /// For tests that must finish every phase: limits a loaded CI machine can't reach.
    fn roomy_plan() -> Plan {
        Plan {
            idle_cap: Duration::from_secs(30),
            down_cap: Duration::from_secs(60),
            up_cap: Duration::from_secs(60),
            probe_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(10),
            ..small_plan()
        }
    }

    async fn server(delay: Duration) -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/__down"))
            .respond_with(Down(delay))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/__up"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&s)
            .await;
        s
    }

    fn noop() -> impl Fn(SpeedPhase, f64, u64, &Live) + Send + Sync {
        |_, _, _, _| {}
    }

    #[test]
    fn median_and_throughput() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
        // 1 MB per second after the ramp is 8 Mbps.
        let s = [(0.0, 0), (1.0, 1_000_000), (2.0, 2_000_000)];
        assert!((throughput(&s).unwrap() - 8.0).abs() < 1e-9);
        assert_eq!(throughput(&[(1.0, 0)]), None);
    }

    #[test]
    fn server_timing_and_colo() {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            "server-timing",
            "cfL4;desc=\"x\", cfRequestDuration;dur=12.5"
                .parse()
                .unwrap(),
        );
        h.insert("cf-ray", "8f00aa11bb22cc33-jed".parse().unwrap());
        assert_eq!(server_ms(&h), 12.5);
        assert_eq!(colo(&h).as_deref(), Some("JED"));
        assert_eq!(server_ms(&reqwest::header::HeaderMap::new()), 0.0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn measures_every_phase_within_budget() {
        let s = server(Duration::ZERO).await;
        let (_tx, rx) = watch::channel(false);
        let phases = Mutex::new(Vec::new());
        let report = |p: SpeedPhase, _: f64, _: u64, _: &Live| phases.lock().unwrap().push(p);
        let m = run(&s.uri(), &roomy_plan(), rx, &report).await.unwrap();
        assert!(m.idle_ping_ms.is_some());
        assert!(m.download_mbps.unwrap() > 0.0);
        assert!(m.upload_mbps.unwrap() > 0.0);
        assert!(m.ping_down_ms.is_some() && m.ping_up_ms.is_some());
        assert_eq!(m.bytes_used, 700_000);
        assert_eq!(m.server.as_deref(), Some("JED"));
        let p = phases.lock().unwrap().clone();
        assert_eq!(p.first(), Some(&SpeedPhase::IdlePing));
        assert!(p.contains(&SpeedPhase::Download) && p.contains(&SpeedPhase::Upload));
        assert_eq!(p.last(), Some(&SpeedPhase::Done));
        let reqs = s.received_requests().await.unwrap();
        assert!(reqs.iter().any(|r| r.method.as_str() == "POST"));
    }

    /// Answers transfers at once and pings late, so each transfer ends before its first probe.
    struct SlowPings(Duration);
    impl Respond for SlowPings {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let n: usize = req
                .url
                .query_pairs()
                .find(|(k, _)| k == "bytes")
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            let t = ResponseTemplate::new(200).set_body_bytes(vec![0u8; n]);
            if n == 0 {
                t.set_delay(self.0)
            } else {
                t
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_transfer_that_beats_its_first_probe_still_reports_a_ping() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/__down"))
            .respond_with(SlowPings(Duration::from_millis(400)))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/__up"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&s)
            .await;
        let (_tx, rx) = watch::channel(false);
        let m = run(&s.uri(), &roomy_plan(), rx, &noop()).await.unwrap();
        assert!(m.ping_down_ms.is_some() && m.ping_up_ms.is_some());
    }

    /// Light once took 58 s for 31 MB on a link busy from the test before it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn slow_idle_pings_stop_at_their_time_cap() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/__down"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(100)))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/__up"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&s)
            .await;
        let (_tx, rx) = watch::channel(false);
        let plan = Plan {
            idle_pings: 100,
            idle_cap: Duration::from_secs(1),
            ..roomy_plan()
        };
        let pings = Mutex::new(0usize);
        let report = |p: SpeedPhase, _: f64, _: u64, _: &Live| {
            if p == SpeedPhase::IdlePing {
                *pings.lock().unwrap() += 1;
            }
        };
        let m = run(&s.uri(), &plan, rx, &report).await.unwrap();
        assert!(m.idle_ping_ms.is_some());
        // Each ping takes at least 100 ms, so 1 s can't hold many of the 100.
        assert!(*pings.lock().unwrap() <= 12, "{}", pings.lock().unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_slow_transfer_stops_at_its_time_cap() {
        let s = server(Duration::from_secs(30)).await;
        let (_tx, rx) = watch::channel(false);
        let plan = Plan {
            down_cap: Duration::from_millis(400),
            ..roomy_plan()
        };
        let t0 = Instant::now();
        let m = run(&s.uri(), &plan, rx, &noop()).await.unwrap();
        // Far short of the 30 s the transfer would take.
        assert!(t0.elapsed() < Duration::from_secs(25));
        // Nothing arrived within the cap.
        assert_eq!(m.download_mbps, None);
        assert!(m.upload_mbps.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_stops_the_test() {
        let s = server(Duration::from_secs(30)).await;
        let (tx, rx) = watch::channel(false);
        let plan = Plan {
            down_cap: Duration::from_secs(30),
            ..small_plan()
        };
        let h = tokio::spawn(async move {
            let base = s.uri();
            run(&base, &plan, rx, &noop()).await
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        tx.send(true).unwrap();
        let r = tokio::time::timeout(Duration::from_secs(5), h)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r, Err(CfError::Cancelled));
    }

    #[test]
    fn a_client_that_cannot_be_built_is_an_error_not_a_panic() {
        assert!(client().is_ok());
        let r = client_with_agent("bad\nagent");
        assert!(matches!(r, Err(CfError::Unreachable(m)) if m.detail.contains("HTTP client")));
    }

    #[tokio::test]
    async fn an_unreachable_server_is_reported() {
        let (_tx, rx) = watch::channel(false);
        // Port 9 on localhost: nothing listens there.
        let r = run("http://127.0.0.1:9", &small_plan(), rx, &noop()).await;
        let Err(CfError::Unreachable(f)) = r else {
            panic!("expected unreachable, got {r:?}")
        };
        // Windows waits about 2 s before refusing a loopback connection, so the short test
        // timeout can win; either way the reason is specific.
        assert!(
            matches!(f.reason, FailReason::Refused | FailReason::Timeout),
            "{:?}: {}",
            f.reason,
            f.detail
        );
        // The cause chain is kept, not just "error sending request".
        assert!(
            f.detail.len() > "error sending request".len(),
            "{}",
            f.detail
        );
    }

    #[tokio::test]
    async fn a_slow_first_answer_is_retried_then_measured() {
        let s = MockServer::start().await;
        // The first request hangs past the connect timeout, like a link still busy.
        Mock::given(method("GET"))
            .and(path("/__down"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .up_to_n_times(1)
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/__down"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0u8; 1000]))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/__up"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&s)
            .await;
        let (_tx, rx) = watch::channel(false);
        let plan = Plan {
            connect_timeout: Duration::from_millis(1500),
            ..roomy_plan()
        };
        let r = run(&s.uri(), &plan, rx, &noop()).await;
        assert!(r.is_ok(), "{r:?}");
    }

    #[tokio::test]
    async fn a_server_that_never_answers_is_a_timeout() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&s)
            .await;
        let (_tx, rx) = watch::channel(false);
        let r = run(&s.uri(), &small_plan(), rx, &noop()).await;
        let Err(CfError::Unreachable(f)) = r else {
            panic!("expected unreachable, got {r:?}")
        };
        assert_eq!(f.reason, FailReason::Timeout, "{}", f.detail);
    }
}
