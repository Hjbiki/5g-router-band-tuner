//! The signal sampler: folds the actor's snapshots into one row per UTC minute.
//!
//! `MinuteAggregator` is pure. Each fresh snapshot (its `captured_at_ms` moved forward) is a
//! sample that lasts until the next fresh one, at most `SAMPLE_CAP_MS`. Signal statistics come
//! from in-service samples only, and the descriptive columns (band, mode, operator...) hold the
//! value seen for the most time in the minute. A wall-clock jump (sleep, clock change) gives no
//! time at all.
//!
//! Only band, PCI and channel numbers are read: never a cell ID, eNB ID or sector.

use router_core::{AppSnapshot, Rat, SessionView};
use rusqlite::{params, Connection};

use crate::speedtest::band_change_open;

pub const MINUTE_MS: u64 = 60_000;
/// A sample never counts for longer than this.
pub const SAMPLE_CAP_MS: u64 = 10_000;
/// A bigger step of the wall clock between two calls is a sleep or a clock change.
pub const JUMP_MS: u64 = 120_000;

/// The minute's flags.
pub const FLAG_BAND_CHANGE: u8 = 1;
pub const FLAG_SCAN: u8 = 2;
pub const FLAG_SPEED_TEST: u8 = 4;
pub const FLAG_HIDDEN: u8 = 8;
/// Refresh IP had mobile data off and on in this minute: the app's own doing, never an outage.
pub const FLAG_IP_REFRESH: u8 = 16;
/// An APN switch was open in this minute: the app's own doing, never an outage.
pub const FLAG_NET_CHANGE: u8 = 32;

/// Min, time-weighted average and max, in tenths of a dB.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stat {
    pub min: Option<i32>,
    pub avg: Option<i32>,
    pub max: Option<i32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MinuteRow {
    pub minute_ms: u64,
    pub alive_ms: u64,
    pub observed_ms: u64,
    pub service_ms: u64,
    pub nr_ms: u64,
    pub lte_ca_ms: u64,
    pub cell_changes: u32,
    pub flags: u8,
    pub lte_rsrp: Stat,
    pub lte_rsrq: Stat,
    pub lte_sinr: Stat,
    pub nr_rsrp: Stat,
    pub nr_rsrq: Stat,
    pub nr_sinr: Stat,
    pub net_mode: Option<String>,
    pub nr_option: Option<String>,
    /// `LteBandSet` as Huawei hex.
    pub allowed_bands: Option<String>,
    pub network_type: Option<u32>,
    pub bars: Option<u8>,
    pub endc: Option<bool>,
    pub endc_restricted: Option<bool>,
    pub rat: Option<String>,
    pub lte_band: Option<u16>,
    pub nr_band: Option<u16>,
    pub lte_pci: Option<u16>,
    pub lte_earfcn: Option<u32>,
    pub nr_pci: Option<u16>,
    pub nr_arfcn: Option<u32>,
    pub operator: Option<String>,
    /// The internet address's range ("a.b") seen for the most time in the minute.
    pub ip_range: Option<String>,
}

/// One descriptive value, for the "most time" tally.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Val {
    Text(String),
    Int(i64),
}

impl Val {
    fn text(&self) -> Option<String> {
        match self {
            Self::Text(t) => Some(t.clone()),
            Self::Int(_) => None,
        }
    }
    fn int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            Self::Text(_) => None,
        }
    }
}

// Positions in `Sample::modes`.
const M_NET_MODE: usize = 0;
const M_NR_OPTION: usize = 1;
const M_ALLOWED: usize = 2;
const M_NETWORK_TYPE: usize = 3;
const M_BARS: usize = 4;
const M_ENDC: usize = 5;
const M_ENDC_RESTRICTED: usize = 6;
const M_RAT: usize = 7;
const M_LTE_BAND: usize = 8;
const M_NR_BAND: usize = 9;
const M_LTE_PCI: usize = 10;
const M_LTE_EARFCN: usize = 11;
const M_NR_PCI: usize = 12;
const M_NR_ARFCN: usize = 13;
const M_OPERATOR: usize = 14;
const M_IP_RANGE: usize = 15;
const MODES: usize = 16;

// Positions in `Sample::metrics`.
const LTE_RSRP: usize = 0;
const LTE_RSRQ: usize = 1;
const LTE_SINR: usize = 2;
const NR_RSRP: usize = 3;
const NR_RSRQ: usize = 4;
const NR_SINR: usize = 5;
const METRICS: usize = 6;

/// What one snapshot says, reduced to what a minute row keeps.
#[derive(Debug, Clone)]
struct Sample {
    observed: bool,
    service: bool,
    nr: bool,
    ca: bool,
    flags: u8,
    metrics: [Option<f32>; METRICS],
    modes: [Option<Val>; MODES],
}

fn rat_name(r: Rat) -> &'static str {
    match r {
        Rat::Gsm => "gsm",
        Rat::Umts => "umts",
        Rat::Lte => "lte",
        Rat::Nr => "nr",
    }
}

fn int<T: Into<i64>>(v: Option<T>) -> Option<Val> {
    v.map(|v| Val::Int(v.into()))
}

fn text(v: Option<String>) -> Option<Val> {
    v.map(Val::Text)
}

impl Sample {
    fn from_snapshot(s: &AppSnapshot, flags: u8) -> Self {
        let observed = matches!(s.session, SessionView::LoggedIn) && s.status.is_some();
        let service = observed && s.in_service;
        let sig = s.signal.as_ref();
        let lte = sig.and_then(|g| g.lte.as_ref());
        let nr = sig.and_then(|g| g.nr.as_ref());
        let status = s.status.as_ref();
        let mut modes: [Option<Val>; MODES] = Default::default();
        if observed {
            let cfg = s.config.as_ref();
            modes[M_NET_MODE] = text(cfg.map(|c| c.network_mode.code().to_string()));
            modes[M_NR_OPTION] = text(
                cfg.and_then(|c| c.nr_option)
                    .and_then(|o| serde_json::to_value(o).ok())
                    .and_then(|v| v.as_str().map(str::to_owned)),
            );
            modes[M_ALLOWED] = text(cfg.map(|c| c.lte_bands.to_huawei_hex()));
            modes[M_NETWORK_TYPE] = int(status.map(|st| st.network_type));
            modes[M_BARS] = int(status.and_then(|st| st.bars));
            modes[M_ENDC] = int(status.and_then(|st| st.endc).map(i64::from));
            modes[M_ENDC_RESTRICTED] = int(status.and_then(|st| st.endc_restricted).map(i64::from));
            modes[M_RAT] = text(sig.and_then(|g| g.serving).map(|r| rat_name(r).to_string()));
            modes[M_LTE_BAND] = int(lte.and_then(|l| l.band));
            modes[M_NR_BAND] = int(nr.and_then(|n| n.band.as_ref()).map(|b| b.primary));
            modes[M_LTE_PCI] = int(lte.and_then(|l| l.pci));
            modes[M_LTE_EARFCN] = int(lte.and_then(|l| l.earfcn_dl));
            modes[M_NR_PCI] = int(nr.and_then(|n| n.pci));
            modes[M_NR_ARFCN] = int(nr.and_then(|n| n.arfcn_dl));
            modes[M_OPERATOR] = text(
                status
                    .and_then(|st| st.operator.as_ref())
                    .map(|o| o.name.clone()),
            );
            modes[M_IP_RANGE] = text(crate::speedtest::clean_ip_range(
                s.wan.as_ref().and_then(|w| w.range.as_deref()),
            ));
        }
        let metrics = if service {
            [
                lte.and_then(|l| l.rsrp),
                lte.and_then(|l| l.rsrq),
                lte.and_then(|l| l.sinr),
                nr.and_then(|n| n.rsrp),
                nr.and_then(|n| n.rsrq),
                nr.and_then(|n| n.sinr),
            ]
        } else {
            [None; METRICS]
        };
        Self {
            observed,
            service,
            nr: service && nr.is_some(),
            ca: service && lte.is_some_and(|l| l.scc_pci.is_some()),
            flags,
            metrics,
            modes,
        }
    }

    /// The serving cell as (rat, pci, band), for counting cell changes. Never the cell ID.
    fn cell_key(s: &AppSnapshot) -> Option<(Rat, Option<u16>, Option<u16>)> {
        let sig = s.signal.as_ref()?;
        let rat = sig.serving?;
        Some(match rat {
            Rat::Nr => {
                let nr = sig.nr.as_ref()?;
                (rat, nr.pci, nr.band.as_ref().map(|b| b.primary))
            }
            _ => {
                let lte = sig.lte.as_ref()?;
                (rat, lte.pci, lte.band)
            }
        })
    }
}

#[derive(Debug, Clone, Default)]
struct Acc {
    sum: f64,
    weight: u64,
    min: Option<f32>,
    max: Option<f32>,
}

impl Acc {
    fn add(&mut self, v: f32, w: u64) {
        self.sum += f64::from(v) * w as f64;
        self.weight += w;
        self.min = Some(self.min.map_or(v, |m| m.min(v)));
        self.max = Some(self.max.map_or(v, |m| m.max(v)));
    }

    fn stat(&self) -> Stat {
        let tenths = |v: f64| Some((v * 10.0).round() as i32);
        Stat {
            min: self.min.and_then(|v| tenths(f64::from(v))),
            avg: (self.weight > 0)
                .then(|| self.sum / self.weight as f64)
                .and_then(tenths),
            max: self.max.and_then(|v| tenths(f64::from(v))),
        }
    }
}

/// Time per value; the first value to reach the most time wins a tie.
#[derive(Debug, Clone, Default)]
struct Tally(Vec<(Val, u64)>);

impl Tally {
    fn add(&mut self, v: &Val, w: u64) {
        match self.0.iter_mut().find(|(x, _)| x == v) {
            Some((_, t)) => *t += w,
            None => self.0.push((v.clone(), w)),
        }
    }

    fn top(&self) -> Option<&Val> {
        let mut best: Option<&(Val, u64)> = None;
        for e in &self.0 {
            if best.is_none_or(|b| e.1 > b.1) {
                best = Some(e);
            }
        }
        best.map(|(v, _)| v)
    }
}

#[derive(Debug, Clone)]
struct Bucket {
    minute_ms: u64,
    alive: u64,
    observed: u64,
    service: u64,
    nr: u64,
    ca: u64,
    cell_changes: u32,
    flags: u8,
    metrics: [Acc; METRICS],
    modes: [Tally; MODES],
}

impl Bucket {
    fn new(minute_ms: u64) -> Self {
        Self {
            minute_ms,
            alive: 0,
            observed: 0,
            service: 0,
            nr: 0,
            ca: 0,
            cell_changes: 0,
            flags: 0,
            metrics: Default::default(),
            modes: Default::default(),
        }
    }

    fn add(&mut self, s: &Sample, w: u64) {
        if w == 0 {
            return;
        }
        self.alive += w;
        self.flags |= s.flags;
        if s.observed {
            self.observed += w;
        }
        if s.service {
            self.service += w;
        }
        if s.nr {
            self.nr += w;
        }
        if s.ca {
            self.ca += w;
        }
        for (acc, v) in self.metrics.iter_mut().zip(s.metrics) {
            if let Some(v) = v {
                acc.add(v, w);
            }
        }
        for (tally, v) in self.modes.iter_mut().zip(&s.modes) {
            if let Some(v) = v {
                tally.add(v, w);
            }
        }
    }

    fn row(&self) -> MinuteRow {
        let text = |i: usize| self.modes[i].top().and_then(Val::text);
        let num = |i: usize| self.modes[i].top().and_then(Val::int);
        let stat = |i: usize| self.metrics[i].stat();
        MinuteRow {
            minute_ms: self.minute_ms,
            alive_ms: self.alive,
            observed_ms: self.observed,
            service_ms: self.service,
            nr_ms: self.nr,
            lte_ca_ms: self.ca,
            cell_changes: self.cell_changes,
            flags: self.flags,
            lte_rsrp: stat(LTE_RSRP),
            lte_rsrq: stat(LTE_RSRQ),
            lte_sinr: stat(LTE_SINR),
            nr_rsrp: stat(NR_RSRP),
            nr_rsrq: stat(NR_RSRQ),
            nr_sinr: stat(NR_SINR),
            net_mode: text(M_NET_MODE),
            nr_option: text(M_NR_OPTION),
            allowed_bands: text(M_ALLOWED),
            network_type: num(M_NETWORK_TYPE).and_then(|v| u32::try_from(v).ok()),
            bars: num(M_BARS).and_then(|v| u8::try_from(v).ok()),
            endc: num(M_ENDC).map(|v| v != 0),
            endc_restricted: num(M_ENDC_RESTRICTED).map(|v| v != 0),
            rat: text(M_RAT),
            lte_band: num(M_LTE_BAND).and_then(|v| u16::try_from(v).ok()),
            nr_band: num(M_NR_BAND).and_then(|v| u16::try_from(v).ok()),
            lte_pci: num(M_LTE_PCI).and_then(|v| u16::try_from(v).ok()),
            lte_earfcn: num(M_LTE_EARFCN).and_then(|v| u32::try_from(v).ok()),
            nr_pci: num(M_NR_PCI).and_then(|v| u16::try_from(v).ok()),
            nr_arfcn: num(M_NR_ARFCN).and_then(|v| u32::try_from(v).ok()),
            operator: text(M_OPERATOR),
            ip_range: text(M_IP_RANGE),
        }
    }
}

#[derive(Debug, Clone)]
struct Pending {
    sample: Sample,
    start: u64,
    /// Time up to here is already in a bucket.
    accounted: u64,
}

fn minute_of(ms: u64) -> u64 {
    ms - ms % MINUTE_MS
}

/// Folds samples into minutes. Feed it with `push`, and call `tick` at least once a minute.
#[derive(Debug, Default)]
pub struct MinuteAggregator {
    pending: Option<Pending>,
    current: Option<Bucket>,
    ready: std::collections::VecDeque<MinuteRow>,
    last_captured: Option<u64>,
    last_now: Option<u64>,
    last_cell: Option<(Rat, Option<u16>, Option<u16>)>,
}

impl MinuteAggregator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Notes a snapshot seen at `now_ms`. A snapshot that isn't newer than the last one is
    /// ignored.
    pub fn push(&mut self, snap: &AppSnapshot, flags: u8, now_ms: u64) {
        self.check_clock(now_ms);
        if self
            .last_captured
            .is_some_and(|last| snap.captured_at_ms <= last)
        {
            return;
        }
        self.last_captured = Some(snap.captured_at_ms);
        self.account(now_ms);
        let sample = Sample::from_snapshot(snap, flags);
        if sample.service {
            if let Some(key) = Sample::cell_key(snap) {
                if self.last_cell.is_some_and(|last| last != key) {
                    self.bucket(minute_of(now_ms)).cell_changes += 1;
                }
                self.last_cell = Some(key);
            }
        }
        self.pending = Some(Pending {
            sample,
            start: now_ms,
            accounted: now_ms,
        });
    }

    /// Accounts time up to `now_ms` and returns a finished minute, if any. Call it again until
    /// it returns `None`.
    pub fn tick(&mut self, now_ms: u64) -> Option<MinuteRow> {
        self.check_clock(now_ms);
        self.account(now_ms);
        if self
            .current
            .as_ref()
            .is_some_and(|b| b.minute_ms < minute_of(now_ms))
        {
            self.finish();
        }
        self.ready.pop_front()
    }

    /// Ends sampling (the actor went away): the partial minute is returned too.
    pub fn flush(&mut self, now_ms: u64) -> Vec<MinuteRow> {
        self.check_clock(now_ms);
        self.account(now_ms);
        self.finish();
        self.pending = None;
        self.ready.drain(..).collect()
    }

    /// A clock that went back, or jumped forward by more than `JUMP_MS`, closes the minute at
    /// the last call and drops the open sample, so the gap counts for nothing. A clock that went
    /// back also forgets the last capture time, since snapshots are stamped with the same clock:
    /// otherwise every new one would look older and be ignored until the clock caught up.
    fn check_clock(&mut self, now_ms: u64) {
        if let Some(last) = self.last_now {
            if now_ms < last || now_ms - last > JUMP_MS {
                self.finish();
                self.pending = None;
            }
            if now_ms < last {
                self.last_captured = None;
                self.last_cell = None;
            }
        }
        self.last_now = Some(now_ms);
    }

    /// Moves the open sample's time, up to `until` and its cap, into the minutes it covers.
    fn account(&mut self, until: u64) {
        let Some(mut p) = self.pending.take() else {
            return;
        };
        let end = until.min(p.start + SAMPLE_CAP_MS);
        while p.accounted < end {
            let minute = minute_of(p.accounted);
            let seg_end = end.min(minute + MINUTE_MS);
            self.bucket(minute).add(&p.sample, seg_end - p.accounted);
            p.accounted = seg_end;
        }
        self.pending = Some(p);
    }

    /// The bucket for `minute`; an older open bucket is finished first.
    fn bucket(&mut self, minute: u64) -> &mut Bucket {
        if self.current.as_ref().is_some_and(|b| b.minute_ms != minute) {
            self.finish();
        }
        self.current.get_or_insert_with(|| Bucket::new(minute))
    }

    fn finish(&mut self) {
        if let Some(b) = self.current.take() {
            if b.alive > 0 {
                self.ready.push_back(b.row());
            }
        }
    }
}

/// The flags for a snapshot, from the app's state.
fn flags_now(app: &tauri::AppHandle, snap: &AppSnapshot) -> u8 {
    use tauri::Manager;
    let scanning = app
        .try_state::<crate::autoscan::ScanState>()
        .is_some_and(|s| s.is_running());
    let testing = app
        .try_state::<crate::speedtest::SpeedTestState>()
        .is_some_and(|s| s.is_running());
    let hidden = app
        .try_state::<crate::commands::AppState>()
        .is_some_and(|s| !s.is_visible());
    flags_of(snap, scanning, testing, hidden)
}

fn flags_of(snap: &AppSnapshot, scanning: bool, testing: bool, hidden: bool) -> u8 {
    [
        (band_change_open(&snap.safe_apply), FLAG_BAND_CHANGE),
        (scanning, FLAG_SCAN),
        (testing, FLAG_SPEED_TEST),
        (hidden, FLAG_HIDDEN),
        (crate::speedtest::ip_refresh_active(snap), FLAG_IP_REFRESH),
        (crate::speedtest::apn_change_open(snap), FLAG_NET_CHANGE),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(0, |f, (_, flag)| f | flag)
}

/// Samples one actor's snapshots until its watch closes, sending each finished minute to the
/// history writer.
pub fn spawn_sampler(
    app: &tauri::AppHandle,
    mut snaps: tokio::sync::watch::Receiver<AppSnapshot>,
    host: String,
    driver: String,
) {
    use std::sync::Arc;
    use tauri::Manager;

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let send = |rows: Vec<MinuteRow>| {
            let Some(store) = app.try_state::<Arc<super::HistoryStore>>() else {
                return;
            };
            if rows.is_empty() || store.is_disabled() {
                return;
            }
            let store = store.inner().clone();
            let router = (host.clone(), driver.clone());
            tauri::async_runtime::spawn_blocking(move || {
                for row in rows {
                    let _ = store.send(super::WriteMsg::Minute {
                        row: Box::new(row),
                        router: router.clone(),
                    });
                }
            });
        };
        let mut agg = MinuteAggregator::new();
        {
            let snap = snaps.borrow_and_update().clone();
            agg.push(&snap, flags_now(&app, &snap), super::now_ms());
        }
        loop {
            let now = super::now_ms();
            // Just past the next minute boundary.
            let wait = minute_of(now) + MINUTE_MS - now + 50;
            tokio::select! {
                changed = snaps.changed() => {
                    if changed.is_err() {
                        send(agg.flush(super::now_ms()));
                        break;
                    }
                    let snap = snaps.borrow_and_update().clone();
                    agg.push(&snap, flags_now(&app, &snap), super::now_ms());
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(wait)) => {}
            }
            let now = super::now_ms();
            let mut rows = Vec::new();
            while let Some(row) = agg.tick(now) {
                rows.push(row);
            }
            send(rows);
        }
    });
}

/// Columns merged by a weighted average when the same minute arrives twice (an actor replaced
/// mid-minute): LTE values weigh by service time, NR values by NR time.
const STAT_COLS: [(&str, &str); 6] = [
    ("lte_rsrp", "service_ms"),
    ("lte_rsrq", "service_ms"),
    ("lte_sinr", "service_ms"),
    ("nr_rsrp", "nr_ms"),
    ("nr_rsrq", "nr_ms"),
    ("nr_sinr", "nr_ms"),
];
/// Descriptive columns: the copy with more observed time wins.
const MODE_COLS: [&str; 16] = [
    "net_mode",
    "nr_option",
    "allowed_bands",
    "network_type",
    "bars",
    "endc",
    "endc_restricted",
    "rat",
    "lte_band",
    "nr_band",
    "lte_pci",
    "lte_earfcn",
    "nr_pci",
    "nr_arfcn",
    "operator",
    "ip_range",
];

fn upsert_sql() -> String {
    let mut cols = vec![
        "minute_ms",
        "router_id",
        "alive_ms",
        "observed_ms",
        "service_ms",
        "nr_ms",
        "lte_ca_ms",
        "cell_changes",
        "flags",
    ]
    .into_iter()
    .map(String::from)
    .collect::<Vec<_>>();
    for (s, _) in STAT_COLS {
        for part in ["min", "avg", "max"] {
            cols.push(format!("{s}_{part}"));
        }
    }
    cols.extend(MODE_COLS.iter().map(|c| c.to_string()));
    let marks = (1..=cols.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut set = vec![
        "flags = flags | excluded.flags".to_string(),
        "cell_changes = cell_changes + excluded.cell_changes".to_string(),
    ];
    for c in [
        "alive_ms",
        "observed_ms",
        "service_ms",
        "nr_ms",
        "lte_ca_ms",
    ] {
        set.push(format!("{c} = {c} + excluded.{c}"));
    }
    for (s, w) in STAT_COLS {
        set.push(format!(
            "{s}_min = min(coalesce({s}_min, excluded.{s}_min), coalesce(excluded.{s}_min, {s}_min))"
        ));
        set.push(format!(
            "{s}_max = max(coalesce({s}_max, excluded.{s}_max), coalesce(excluded.{s}_max, {s}_max))"
        ));
        set.push(format!(
            "{s}_avg = CASE WHEN {s}_avg IS NULL THEN excluded.{s}_avg \
             WHEN excluded.{s}_avg IS NULL THEN {s}_avg \
             ELSE CAST(round(({s}_avg * {w} + excluded.{s}_avg * excluded.{w}) * 1.0 \
             / max({w} + excluded.{w}, 1)) AS INTEGER) END"
        ));
    }
    for c in MODE_COLS {
        set.push(format!(
            "{c} = CASE WHEN excluded.observed_ms > observed_ms \
             THEN coalesce(excluded.{c}, {c}) ELSE coalesce({c}, excluded.{c}) END"
        ));
    }
    format!(
        "INSERT INTO signal_minute ({}) VALUES ({marks}) \
         ON CONFLICT (minute_ms, router_id) DO UPDATE SET {}",
        cols.join(", "),
        set.join(", ")
    )
}

/// Writes one minute, merging with a row already there for the same minute and router.
pub fn upsert_minute(conn: &Connection, router_id: i64, r: &MinuteRow) -> rusqlite::Result<()> {
    let st = |s: &Stat| [s.min, s.avg, s.max];
    let [a, b, c] = st(&r.lte_rsrp);
    let [d, e, f] = st(&r.lte_rsrq);
    let [g, h, i] = st(&r.lte_sinr);
    let [j, k, l] = st(&r.nr_rsrp);
    let [m, n, o] = st(&r.nr_rsrq);
    let [p, q, s] = st(&r.nr_sinr);
    conn.prepare_cached(&upsert_sql())?.execute(params![
        r.minute_ms as i64,
        router_id,
        r.alive_ms as i64,
        r.observed_ms as i64,
        r.service_ms as i64,
        r.nr_ms as i64,
        r.lte_ca_ms as i64,
        r.cell_changes,
        r.flags,
        a,
        b,
        c,
        d,
        e,
        f,
        g,
        h,
        i,
        j,
        k,
        l,
        m,
        n,
        o,
        p,
        q,
        s,
        r.net_mode,
        r.nr_option,
        r.allowed_bands,
        r.network_type,
        r.bars,
        r.endc,
        r.endc_restricted,
        r.rat,
        r.lte_band,
        r.nr_band,
        r.lte_pci,
        r.lte_earfcn,
        r.nr_pci,
        r.nr_arfcn,
        r.operator,
        r.ip_range,
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::{
        ConnState, Duplex, LteBandSet, LteSignal, NetConfig, NetworkMode, NrBandInfo, NrOption,
        NrSignal, Operator, Phase, SafeApplyView, Signal, Status,
    };

    const T0: u64 = 1_800_000_000_000 - 1_800_000_000_000 % MINUTE_MS;

    /// A logged-in, in-service snapshot on LTE `band` with demo PCI `pci`.
    fn snap(at: u64, band: u16, pci: u16, rsrp: f32) -> AppSnapshot {
        AppSnapshot {
            session: SessionView::LoggedIn,
            device: None,
            status: Some(Status {
                connection: ConnState::Connected,
                network_type: 19,
                operator: Some(Operator {
                    name: "Demo Mobile".into(),
                    mcc_mnc: "00101".into(),
                    spn: None,
                }),
                bars: Some(4),
                nr_bars: None,
                endc: Some(true),
                endc_restricted: Some(false),
            }),
            signal: Some(Signal {
                serving: Some(Rat::Lte),
                lte: Some(LteSignal {
                    band: Some(band),
                    pci: Some(pci),
                    earfcn_dl: Some(1300),
                    cell_id: Some(12_345),
                    enb_id: Some(48),
                    sector: Some(1),
                    rsrp: Some(rsrp),
                    rsrq: Some(-11.0),
                    sinr: Some(-1.0),
                    ..LteSignal::default()
                }),
                nr: None,
            }),
            config: Some(NetConfig {
                network_mode: NetworkMode::Auto,
                lte_bands: LteBandSet::from_bands(&[1, 3]).unwrap(),
                legacy_band_raw: "0".into(),
                nr_option: Some(NrOption::Nsa),
            }),
            capabilities: None,
            safe_apply: SafeApplyView {
                phase: Phase::Idle,
                seconds_left: None,
                previous: None,
                requested: None,
                policy: None,
                sequence: None,
            },
            in_service: true,
            foreign_journal: None,
            captured_at_ms: at,
            relock: None,
            wan: None,
            ip_refresh: None,
            data_switch_off: false,
            apn: None,
            apn_apply: None,
            ip_reattach: None,
        }
    }

    fn with_nr(mut s: AppSnapshot, rsrp: f32) -> AppSnapshot {
        s.signal.as_mut().unwrap().nr = Some(NrSignal {
            band: Some(NrBandInfo {
                primary: 41,
                alternates: vec![],
                duplex: Duplex::Tdd,
                dl_mhz: 100.0,
            }),
            pci: Some(201),
            arfcn_dl: Some(500_000),
            rsrp: Some(rsrp),
            rsrq: Some(-10.0),
            sinr: Some(5.0),
            ..NrSignal::default()
        });
        s
    }

    fn unreachable(at: u64) -> AppSnapshot {
        let mut s = snap(at, 3, 101, -90.0);
        s.session = SessionView::Reconnecting {
            reason: "timeout".into(),
            retry_in_s: 5,
        };
        s.in_service = false;
        s
    }

    /// Pushes a snapshot every `step` ms from `from` to `to` (exclusive).
    fn feed(
        agg: &mut MinuteAggregator,
        from: u64,
        to: u64,
        step: u64,
        f: impl Fn(u64) -> AppSnapshot,
        flags: u8,
    ) {
        let mut t = from;
        while t < to {
            agg.push(&f(t), flags, t);
            t += step;
        }
    }

    fn rows(agg: &mut MinuteAggregator, now: u64) -> Vec<MinuteRow> {
        std::iter::from_fn(|| agg.tick(now)).collect()
    }

    #[test]
    fn a_steady_minute_is_one_full_row() {
        let mut agg = MinuteAggregator::new();
        feed(
            &mut agg,
            T0,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| snap(t, 3, 101, -95.0),
            0,
        );
        let got = rows(&mut agg, T0 + MINUTE_MS + 1);
        assert_eq!(got.len(), 1);
        let r = &got[0];
        assert_eq!(r.minute_ms, T0);
        assert_eq!(r.alive_ms, MINUTE_MS);
        assert_eq!(r.observed_ms, MINUTE_MS);
        assert_eq!(r.service_ms, MINUTE_MS);
        assert_eq!(r.nr_ms, 0);
        assert_eq!(
            r.lte_rsrp,
            Stat {
                min: Some(-950),
                avg: Some(-950),
                max: Some(-950)
            }
        );
        assert_eq!(r.lte_band, Some(3));
        assert_eq!(r.lte_pci, Some(101));
        assert_eq!(r.lte_earfcn, Some(1300));
        assert_eq!(r.allowed_bands.as_deref(), Some("5"));
        assert_eq!(r.net_mode.as_deref(), Some("00"));
        assert_eq!(r.nr_option.as_deref(), Some("nsa"));
        assert_eq!(r.rat.as_deref(), Some("lte"));
        assert_eq!(r.operator.as_deref(), Some("Demo Mobile"));
        assert_eq!(
            (r.bars, r.endc, r.endc_restricted),
            (Some(4), Some(true), Some(false))
        );
        assert_eq!(r.network_type, Some(19));
        assert_eq!(r.cell_changes, 0);
        assert_eq!(r.flags, 0);
        assert_eq!(r.nr_rsrp, Stat::default());
    }

    #[test]
    fn the_average_is_weighted_by_time_and_min_max_kept() {
        let mut agg = MinuteAggregator::new();
        // -100 dBm for 40 s, then -90 dBm for 20 s.
        feed(
            &mut agg,
            T0,
            T0 + 40_000,
            5_000,
            |t| snap(t, 3, 101, -100.0),
            0,
        );
        feed(
            &mut agg,
            T0 + 40_000,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| snap(t, 3, 101, -90.0),
            0,
        );
        let r = &rows(&mut agg, T0 + MINUTE_MS + 1)[0];
        assert_eq!(r.lte_rsrp.min, Some(-1000));
        assert_eq!(r.lte_rsrp.max, Some(-900));
        // (-100 * 40 + -90 * 20) / 60 = -96.67
        assert_eq!(r.lte_rsrp.avg, Some(-967));
    }

    #[test]
    fn a_sample_counts_for_at_most_ten_seconds() {
        let mut agg = MinuteAggregator::new();
        agg.push(&snap(T0, 3, 101, -95.0), 0, T0);
        agg.push(&snap(T0 + 30_000, 3, 101, -95.0), 0, T0 + 30_000);
        let r = &agg.flush(T0 + 35_000)[0];
        assert_eq!(r.alive_ms, 10_000 + 5_000);
    }

    #[test]
    fn a_stale_snapshot_is_not_a_new_sample() {
        let mut agg = MinuteAggregator::new();
        let s = snap(T0, 3, 101, -95.0);
        agg.push(&s, 0, T0);
        // The same snapshot seen again 8 s later doesn't restart the 10 s cap.
        agg.push(&s, 0, T0 + 8_000);
        let r = &agg.flush(T0 + 30_000)[0];
        assert_eq!(r.alive_ms, 10_000);
    }

    #[test]
    fn minutes_split_at_the_boundary() {
        let mut agg = MinuteAggregator::new();
        agg.push(&snap(T0 + 55_000, 3, 101, -95.0), 0, T0 + 55_000);
        agg.push(&snap(T0 + 65_000, 3, 101, -95.0), 0, T0 + 65_000);
        let mut got = rows(&mut agg, T0 + 65_000);
        got.extend(agg.flush(T0 + 70_000));
        assert_eq!(
            got.iter()
                .map(|r| (r.minute_ms, r.alive_ms))
                .collect::<Vec<_>>(),
            vec![(T0, 5_000), (T0 + MINUTE_MS, 10_000)]
        );
    }

    /// After a logout the actor clears the values; a minute logged out is never "observed".
    #[test]
    fn a_logged_out_minute_is_not_observed() {
        let mut agg = MinuteAggregator::new();
        let logged_out = |at| {
            let mut s = snap(at, 3, 101, -90.0);
            s.session = SessionView::LoggedOut;
            s
        };
        feed(&mut agg, T0, T0 + MINUTE_MS + 1, 5_000, logged_out, 0);
        let r = &rows(&mut agg, T0 + MINUTE_MS + 1)[0];
        assert_eq!((r.observed_ms, r.service_ms), (0, 0));
        assert_eq!(r.lte_band, None);
    }

    #[test]
    fn an_unreachable_router_gives_rows_without_values() {
        let mut agg = MinuteAggregator::new();
        feed(&mut agg, T0, T0 + MINUTE_MS + 1, 5_000, unreachable, 0);
        let r = &rows(&mut agg, T0 + MINUTE_MS + 1)[0];
        assert_eq!(r.alive_ms, MINUTE_MS);
        assert_eq!((r.observed_ms, r.service_ms), (0, 0));
        assert_eq!(r.lte_rsrp, Stat::default());
        assert_eq!(r.lte_band, None);
        assert_eq!(r.operator, None);
    }

    #[test]
    fn statistics_come_from_in_service_samples_only() {
        let mut agg = MinuteAggregator::new();
        feed(
            &mut agg,
            T0,
            T0 + 30_000,
            5_000,
            |t| snap(t, 3, 101, -95.0),
            0,
        );
        feed(
            &mut agg,
            T0 + 30_000,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| {
                let mut s = snap(t, 3, 101, -130.0);
                s.in_service = false;
                s
            },
            0,
        );
        let r = &rows(&mut agg, T0 + MINUTE_MS + 1)[0];
        assert_eq!((r.observed_ms, r.service_ms), (MINUTE_MS, 30_000));
        assert_eq!(r.lte_rsrp.min, Some(-950));
    }

    #[test]
    fn descriptive_values_are_the_ones_seen_longest() {
        let mut agg = MinuteAggregator::new();
        feed(
            &mut agg,
            T0,
            T0 + 20_000,
            5_000,
            |t| snap(t, 1, 102, -95.0),
            0,
        );
        feed(
            &mut agg,
            T0 + 20_000,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| snap(t, 3, 101, -95.0),
            0,
        );
        let r = &rows(&mut agg, T0 + MINUTE_MS + 1)[0];
        assert_eq!((r.lte_band, r.lte_pci), (Some(3), Some(101)));
        assert_eq!(r.cell_changes, 1);
    }

    #[test]
    fn cell_changes_count_rat_pci_and_band_and_carry_across_minutes() {
        let mut agg = MinuteAggregator::new();
        agg.push(&snap(T0, 3, 101, -95.0), 0, T0);
        agg.push(&snap(T0 + 5_000, 3, 102, -95.0), 0, T0 + 5_000);
        agg.push(&snap(T0 + 10_000, 1, 102, -95.0), 0, T0 + 10_000);
        // No service in between doesn't count, and the same cell again isn't a change.
        let mut gone = snap(T0 + 15_000, 3, 101, -95.0);
        gone.in_service = false;
        agg.push(&gone, 0, T0 + 15_000);
        agg.push(&snap(T0 + 20_000, 1, 102, -95.0), 0, T0 + 20_000);
        let first = rows(&mut agg, T0 + MINUTE_MS);
        assert_eq!(first[0].cell_changes, 2);
        agg.push(
            &snap(T0 + MINUTE_MS + 1_000, 3, 101, -95.0),
            0,
            T0 + MINUTE_MS + 1_000,
        );
        let second = agg.flush(T0 + MINUTE_MS + 5_000);
        assert_eq!(second[0].cell_changes, 1);
    }

    #[test]
    fn nr_time_and_values_are_kept() {
        let mut agg = MinuteAggregator::new();
        feed(
            &mut agg,
            T0,
            T0 + 30_000,
            5_000,
            |t| with_nr(snap(t, 3, 101, -95.0), -88.5),
            0,
        );
        feed(
            &mut agg,
            T0 + 30_000,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| snap(t, 3, 101, -95.0),
            0,
        );
        let r = &rows(&mut agg, T0 + MINUTE_MS + 1)[0];
        assert_eq!(r.nr_ms, 30_000);
        assert_eq!(r.nr_rsrp.avg, Some(-885));
        assert_eq!(
            (r.nr_band, r.nr_pci, r.nr_arfcn),
            (Some(41), Some(201), Some(500_000))
        );
    }

    #[test]
    fn carrier_aggregation_time_is_counted() {
        let mut agg = MinuteAggregator::new();
        feed(
            &mut agg,
            T0,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| {
                let mut s = snap(t, 3, 101, -95.0);
                s.signal.as_mut().unwrap().lte.as_mut().unwrap().scc_pci = Some(104);
                s
            },
            0,
        );
        assert_eq!(rows(&mut agg, T0 + MINUTE_MS + 1)[0].lte_ca_ms, MINUTE_MS);
    }

    #[test]
    fn an_apn_switch_flags_the_minute_as_the_apps_own() {
        let mut s = snap(T0, 3, 101, -95.0);
        s.apn_apply = Some(crate::speedtest::apn_change(router_core::Phase::Probation));
        assert_eq!(flags_of(&s, false, false, false), FLAG_NET_CHANGE);
        for f in [
            FLAG_BAND_CHANGE,
            FLAG_SCAN,
            FLAG_SPEED_TEST,
            FLAG_HIDDEN,
            FLAG_IP_REFRESH,
        ] {
            assert_eq!(f & FLAG_NET_CHANGE, 0);
        }
        assert_ne!(super::super::query::OWN_CHANGE & FLAG_NET_CHANGE, 0);
        s.apn_apply = Some(crate::speedtest::apn_change(router_core::Phase::Committed));
        assert_eq!(flags_of(&s, false, false, false), 0);
    }

    #[test]
    fn a_refresh_ip_run_flags_the_minute() {
        let mut s = snap(T0, 3, 101, -95.0);
        assert_eq!(flags_of(&s, false, false, false), 0);
        s.ip_refresh = Some(crate::speedtest::running_ip_refresh());
        assert_eq!(flags_of(&s, false, false, false), FLAG_IP_REFRESH);
        assert_eq!(
            flags_of(&s, true, true, true),
            FLAG_IP_REFRESH | FLAG_SCAN | FLAG_SPEED_TEST | FLAG_HIDDEN
        );
        // Distinct from every other flag, and counted as the app's own change.
        for f in [FLAG_BAND_CHANGE, FLAG_SCAN, FLAG_SPEED_TEST, FLAG_HIDDEN] {
            assert_eq!(f & FLAG_IP_REFRESH, 0);
        }
        assert_ne!(super::super::query::OWN_CHANGE & FLAG_IP_REFRESH, 0);
    }

    #[test]
    fn flags_are_combined_over_the_minute() {
        let mut agg = MinuteAggregator::new();
        feed(
            &mut agg,
            T0,
            T0 + 20_000,
            5_000,
            |t| snap(t, 3, 101, -95.0),
            FLAG_SCAN,
        );
        feed(
            &mut agg,
            T0 + 20_000,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| snap(t, 3, 101, -95.0),
            FLAG_HIDDEN | FLAG_SPEED_TEST,
        );
        let r = &rows(&mut agg, T0 + MINUTE_MS + 1)[0];
        assert_eq!(r.flags, FLAG_SCAN | FLAG_HIDDEN | FLAG_SPEED_TEST);
    }

    #[test]
    fn a_wall_clock_jump_gives_no_time() {
        let mut agg = MinuteAggregator::new();
        agg.push(&snap(T0, 3, 101, -95.0), 0, T0);
        agg.push(&snap(T0 + 5_000, 3, 101, -95.0), 0, T0 + 5_000);
        // The PC slept for 10 minutes.
        let later = T0 + 10 * MINUTE_MS + 5_000;
        agg.push(&snap(later, 3, 101, -95.0), 0, later);
        let got = rows(&mut agg, later);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].minute_ms, got[0].alive_ms), (T0, 5_000));
        let after = agg.flush(later + 3_000);
        assert_eq!(after[0].alive_ms, 3_000);
        // A clock set back closes the minute too, and the open sample gets no more time.
        let mut agg = MinuteAggregator::new();
        agg.push(&snap(T0, 3, 101, -95.0), 0, T0);
        agg.push(&snap(T0 + 5_000, 3, 101, -95.0), 0, T0 + 5_000);
        agg.push(&snap(T0 + 5_001, 3, 101, -95.0), 0, T0 + 1_000);
        let back = rows(&mut agg, T0 + 1_000);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].alive_ms, 5_000);
    }

    #[test]
    fn recording_goes_on_after_the_clock_is_set_back() {
        let mut agg = MinuteAggregator::new();
        let start = T0 + 2 * 60 * MINUTE_MS;
        agg.push(&snap(start, 3, 101, -95.0), 0, start);
        agg.push(&snap(start + 5_000, 3, 101, -95.0), 0, start + 5_000);
        // An hour back: the snapshots are stamped with the same clock, so they look older.
        let back = start + 5_000 - 60 * MINUTE_MS;
        assert!(agg.tick(back).is_some(), "the open minute is closed");
        agg.push(&snap(back + 1_000, 3, 101, -95.0), 0, back + 1_000);
        agg.push(&snap(back + 6_000, 3, 101, -95.0), 0, back + 6_000);
        let got = agg.flush(back + 11_000);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].alive_ms, 10_000, "the new samples count");
    }

    #[test]
    fn nothing_alive_gives_no_row() {
        let mut agg = MinuteAggregator::new();
        assert!(agg.tick(T0).is_none());
        assert!(agg.flush(T0 + 5 * MINUTE_MS).is_empty());
    }

    fn db() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        crate::history::schema::migrate(&mut c).unwrap();
        c
    }

    #[test]
    fn the_same_minute_from_two_actors_is_merged() {
        let c = db();
        let id = crate::history::store::router_id(&c, "192.168.8.1", "mock").unwrap();
        let mut a = MinuteRow {
            minute_ms: T0,
            alive_ms: 20_000,
            observed_ms: 20_000,
            service_ms: 20_000,
            cell_changes: 1,
            flags: FLAG_BAND_CHANGE,
            lte_rsrp: Stat {
                min: Some(-1000),
                avg: Some(-1000),
                max: Some(-1000),
            },
            lte_band: Some(1),
            ..MinuteRow::default()
        };
        upsert_minute(&c, id, &a).unwrap();
        a.alive_ms = 40_000;
        a.observed_ms = 40_000;
        a.service_ms = 40_000;
        a.flags = FLAG_HIDDEN;
        a.lte_rsrp = Stat {
            min: Some(-900),
            avg: Some(-900),
            max: Some(-900),
        };
        a.lte_band = Some(3);
        upsert_minute(&c, id, &a).unwrap();
        let got: (i64, i64, i64, i64, i64, i64, i64, i64) = c
            .query_row(
                "SELECT alive_ms, service_ms, flags, cell_changes, lte_rsrp_min, lte_rsrp_avg,
                        lte_rsrp_max, lte_band FROM signal_minute",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(got, (60_000, 60_000, 9, 2, -1000, -933, -900, 3));
    }

    #[test]
    fn a_row_without_values_merges_with_one_that_has_them() {
        let c = db();
        let id = crate::history::store::router_id(&c, "192.168.8.1", "mock").unwrap();
        let empty = MinuteRow {
            minute_ms: T0,
            alive_ms: 10_000,
            ..MinuteRow::default()
        };
        let full = MinuteRow {
            minute_ms: T0,
            alive_ms: 10_000,
            observed_ms: 10_000,
            service_ms: 10_000,
            lte_rsrp: Stat {
                min: Some(-950),
                avg: Some(-950),
                max: Some(-950),
            },
            ..MinuteRow::default()
        };
        upsert_minute(&c, id, &empty).unwrap();
        upsert_minute(&c, id, &full).unwrap();
        let (min, avg): (i64, i64) = c
            .query_row(
                "SELECT lte_rsrp_min, lte_rsrp_avg FROM signal_minute",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((min, avg), (-950, -950));
    }
    #[test]
    fn the_writer_stores_minutes_per_router() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::history::HistoryStore::open(dir.path());
        let row = MinuteRow {
            minute_ms: T0,
            alive_ms: 60_000,
            ..MinuteRow::default()
        };
        for driver in ["mock", "mock", "huaweiH122"] {
            store
                .send(crate::history::WriteMsg::Minute {
                    row: Box::new(row.clone()),
                    router: ("192.168.8.1".into(), driver.into()),
                })
                .unwrap();
        }
        assert!(store.flush(std::time::Duration::from_secs(5)));
        let rows: Vec<(i64, i64)> = store
            .with_read(|c| {
                let mut st =
                    c.prepare("SELECT router_id, alive_ms FROM signal_minute ORDER BY router_id")?;
                let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect()
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, 120_000, "merged");
        assert_eq!(rows[1].1, 60_000);
        store.shutdown();
    }

    fn with_wan(mut s: AppSnapshot, ip: [u8; 4]) -> AppSnapshot {
        s.wan = Some(router_core::WanView::new(
            Some(std::net::Ipv4Addr::from(ip)),
            false,
        ));
        s
    }

    #[test]
    fn a_minute_keeps_the_ip_range_seen_most() {
        let mut agg = MinuteAggregator::new();
        feed(
            &mut agg,
            T0,
            T0 + 20_000,
            5_000,
            |t| with_wan(snap(t, 3, 101, -95.0), [10, 45, 1, 2]),
            0,
        );
        feed(
            &mut agg,
            T0 + 20_000,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| with_wan(snap(t, 3, 101, -95.0), [100, 70, 3, 4]),
            0,
        );
        let r = &rows(&mut agg, T0 + MINUTE_MS + 1)[0];
        assert_eq!(r.ip_range.as_deref(), Some("100.70"));

        let c = {
            let mut c = Connection::open_in_memory().unwrap();
            crate::history::schema::migrate(&mut c).unwrap();
            c
        };
        let id = crate::history::store::router_id(&c, "192.168.8.1", "mock").unwrap();
        upsert_minute(&c, id, r).unwrap();
        let stored: Option<String> = c
            .query_row("SELECT ip_range FROM signal_minute", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored.as_deref(), Some("100.70"));
        let all: String = c
            .query_row(
                "SELECT group_concat(quote(ip_range)) FROM signal_minute",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!all.contains("100.70.3.4"));
    }

    #[test]
    fn a_minute_without_an_address_has_no_range() {
        let mut agg = MinuteAggregator::new();
        feed(
            &mut agg,
            T0,
            T0 + MINUTE_MS + 1,
            5_000,
            |t| snap(t, 3, 101, -95.0),
            0,
        );
        assert_eq!(rows(&mut agg, T0 + MINUTE_MS + 1)[0].ip_range, None);
    }
}
