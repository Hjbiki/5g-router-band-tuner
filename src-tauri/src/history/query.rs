//! Read-only queries for the History page: the overview, signal buckets, speed tests and the
//! event log. All run on the store's read connection.
//!
//! Buckets are aligned to the user's local time (`utc_offset_min`), so a day bucket starts at
//! local midnight. Signal statistics come from the minute rows: min of the minima, max of the
//! maxima, and averages weighted by time (LTE values by in-service time, NR values by NR time,
//! the same weights the sampler merges with).

use std::collections::BTreeMap;

use router_core::event_text::{self, kinds, ChangeOutcome, EventCategory, EventEntry, Severity};
use router_core::EventRecord;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::sampler::{FLAG_BAND_CHANGE, FLAG_IP_REFRESH, FLAG_NET_CHANGE, FLAG_SCAN, MINUTE_MS};
use crate::speedtest::SpeedTestResult;

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;
/// `Bucket::Auto` picks the smallest bucket that gives at most this many points.
pub const AUTO_POINTS: u64 = 800;
/// No series has more points than this.
pub const MAX_POINTS: u64 = 1000;

/// A time range, both ends inclusive, and the user's offset from UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryRange {
    #[ts(type = "number")]
    pub from_ms: u64,
    #[ts(type = "number")]
    pub to_ms: u64,
    /// `-new Date().getTimezoneOffset()`: 180 for Saudi Arabia.
    pub utc_offset_min: i32,
}

impl HistoryRange {
    pub fn offset_ms(&self) -> i64 {
        i64::from(self.utc_offset_min) * 60_000
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Bucket {
    Auto,
    Min1,
    Min5,
    Min15,
    Hour1,
    Hour6,
    Day1,
}

const FIXED: [(Bucket, u64); 6] = [
    (Bucket::Min1, MINUTE_MS),
    (Bucket::Min5, 5 * MINUTE_MS),
    (Bucket::Min15, 15 * MINUTE_MS),
    (Bucket::Hour1, HOUR_MS),
    (Bucket::Hour6, 6 * HOUR_MS),
    (Bucket::Day1, DAY_MS),
];

impl Bucket {
    fn ms(self) -> Option<u64> {
        FIXED.iter().find(|(b, _)| *b == self).map(|(_, ms)| *ms)
    }
}

fn points(span_ms: u64, bucket_ms: u64) -> u64 {
    span_ms.div_ceil(bucket_ms).max(1)
}

/// The bucket to use and its length. `Auto` is the smallest with at most `AUTO_POINTS` points;
/// any choice is made coarser until it gives at most `MAX_POINTS`. Past `MAX_POINTS` days the
/// day bucket grows to several days.
pub fn resolve_bucket(bucket: Bucket, range: &HistoryRange) -> (Bucket, u64) {
    let span = range.to_ms.saturating_sub(range.from_ms) + 1;
    let fits = |limit: u64| {
        FIXED
            .iter()
            .find(|(_, ms)| points(span, *ms) <= limit)
            .copied()
    };
    let chosen = match bucket.ms() {
        Some(ms) if points(span, ms) <= MAX_POINTS => Some((bucket, ms)),
        Some(ms) => FIXED
            .iter()
            .find(|(_, m)| *m > ms && points(span, *m) <= MAX_POINTS)
            .copied(),
        None => fits(AUTO_POINTS),
    };
    chosen.unwrap_or_else(|| {
        let days = span.div_ceil(DAY_MS).div_ceil(MAX_POINTS).max(1);
        (Bucket::Day1, days * DAY_MS)
    })
}

/// The start of the bucket holding `t_ms`, aligned to local time.
pub fn bucket_start(t_ms: u64, bucket_ms: u64, offset_ms: i64) -> u64 {
    let b = bucket_ms as i64;
    let local = t_ms as i64 + offset_ms;
    (local.div_euclid(b) * b - offset_ms).max(0) as u64
}

/// Minimum, weighted average and maximum, in dB.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Stat3 {
    pub min: Option<f64>,
    pub avg: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SignalPoint {
    /// The bucket's start.
    #[ts(type = "number")]
    pub t_ms: u64,
    /// Seconds the app was running.
    #[ts(type = "number")]
    pub alive_s: u64,
    /// Seconds the router answered while logged in.
    #[ts(type = "number")]
    pub observed_s: u64,
    /// Seconds in service.
    #[ts(type = "number")]
    pub service_s: u64,
    /// Seconds of minutes with a band change or auto-scan, which the app itself caused.
    #[ts(type = "number")]
    pub own_change_s: u64,
    pub lte_rsrp: Stat3,
    pub lte_rsrq: Stat3,
    pub lte_sinr: Stat3,
    pub nr_rsrp: Stat3,
    pub nr_rsrq: Stat3,
    pub nr_sinr: Stat3,
    /// Share of in-service time with 5G, 0 to 1.
    pub nr_share: Option<f64>,
    /// The LTE band in service longest.
    pub lte_band: Option<u16>,
    /// The allowed band set (Huawei hex) seen longest.
    pub allowed_bands: Option<String>,
    pub cell_changes: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SignalSeries {
    pub bucket: Bucket,
    #[ts(type = "number")]
    pub bucket_ms: u64,
    /// Oldest first; buckets without any minute are left out.
    pub points: Vec<SignalPoint>,
}

const STATS: [&str; 6] = [
    "lte_rsrp", "lte_rsrq", "lte_sinr", "nr_rsrp", "nr_rsrq", "nr_sinr",
];

fn signal_sql() -> String {
    let mut cols = String::new();
    for name in STATS {
        let w = if name.starts_with("nr_") {
            "nr_ms"
        } else {
            "service_ms"
        };
        cols.push_str(&format!(
            ", MIN({name}_min), MAX({name}_max), \
             SUM({name}_avg * {w}), SUM(CASE WHEN {name}_avg IS NOT NULL THEN {w} END)"
        ));
    }
    format!(
        "SELECT ((minute_ms + ?3) / ?4) * ?4 - ?3 AS b,
                SUM(alive_ms), SUM(observed_ms), SUM(service_ms), SUM(nr_ms),
                SUM(CASE WHEN flags & {own} != 0 THEN alive_ms ELSE 0 END),
                SUM(cell_changes){cols}
         FROM signal_minute
         WHERE minute_ms >= ?1 AND minute_ms <= ?2
         GROUP BY b ORDER BY b",
        own = OWN_CHANGE,
    )
}

/// Minutes the app itself caused (a band change, an auto-scan, a Refresh IP, an APN switch) never
/// count as lost service.
pub const OWN_CHANGE: u8 = FLAG_BAND_CHANGE | FLAG_SCAN | FLAG_IP_REFRESH | FLAG_NET_CHANGE;

fn tenths(v: Option<i64>) -> Option<f64> {
    v.map(|x| x as f64 / 10.0)
}

fn weighted(sum: Option<f64>, weight: Option<i64>) -> Option<f64> {
    match (sum, weight) {
        (Some(s), Some(w)) if w > 0 => Some((s / w as f64).round() / 10.0),
        _ => None,
    }
}

/// For each bucket, the value of `col` with the most `weight`.
pub(crate) fn modes<T: rusqlite::types::FromSql + Clone>(
    c: &Connection,
    col: &str,
    weight: &str,
    args: (i64, i64, i64, i64),
) -> rusqlite::Result<BTreeMap<i64, T>> {
    let sql = format!(
        "SELECT ((minute_ms + ?3) / ?4) * ?4 - ?3 AS b, {col}, SUM({weight}) AS w
         FROM signal_minute
         WHERE minute_ms >= ?1 AND minute_ms <= ?2 AND {col} IS NOT NULL
         GROUP BY b, {col} ORDER BY b, w DESC, {col}"
    );
    let mut st = c.prepare(&sql)?;
    let rows = st.query_map(params![args.0, args.1, args.2, args.3], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, T>(1)?))
    })?;
    let mut out = BTreeMap::new();
    for row in rows {
        let (b, v) = row?;
        out.entry(b).or_insert(v);
    }
    Ok(out)
}

pub fn signal(
    c: &Connection,
    range: &HistoryRange,
    bucket: Bucket,
) -> rusqlite::Result<SignalSeries> {
    let (bucket, bucket_ms) = resolve_bucket(bucket, range);
    Ok(SignalSeries {
        bucket,
        bucket_ms,
        points: buckets(c, range, bucket_ms)?,
    })
}

/// The SQL arguments of a bucketed query: from, to, offset, bucket length.
pub(crate) fn bucket_args(range: &HistoryRange, bucket_ms: u64) -> (i64, i64, i64, i64) {
    (
        range.from_ms as i64,
        range.to_ms as i64,
        range.offset_ms(),
        bucket_ms as i64,
    )
}

/// Buckets of any length, with no cap on how many: the CSV export's hours.
pub(crate) fn buckets(
    c: &Connection,
    range: &HistoryRange,
    bucket_ms: u64,
) -> rusqlite::Result<Vec<SignalPoint>> {
    let args = (
        range.from_ms as i64,
        range.to_ms as i64,
        range.offset_ms(),
        bucket_ms as i64,
    );
    let bands: BTreeMap<i64, u16> = modes(c, "lte_band", "service_ms", args)?;
    let allowed: BTreeMap<i64, String> = modes(c, "allowed_bands", "alive_ms", args)?;
    let mut st = c.prepare(&signal_sql())?;
    let rows = st.query_map(params![args.0, args.1, args.2, args.3], |r| {
        let b: i64 = r.get(0)?;
        let secs = |i: usize| -> rusqlite::Result<u64> {
            Ok(r.get::<_, Option<i64>>(i)?.unwrap_or(0).max(0) as u64 / 1000)
        };
        let service_ms: i64 = r.get::<_, Option<i64>>(3)?.unwrap_or(0);
        let nr_ms: i64 = r.get::<_, Option<i64>>(4)?.unwrap_or(0);
        let mut stats = [Stat3::default(); 6];
        for (i, s) in stats.iter_mut().enumerate() {
            let at = 7 + i * 4;
            *s = Stat3 {
                min: tenths(r.get(at)?),
                max: tenths(r.get(at + 1)?),
                avg: weighted(r.get(at + 2)?, r.get(at + 3)?),
            };
        }
        Ok(SignalPoint {
            t_ms: b.max(0) as u64,
            alive_s: secs(1)?,
            observed_s: secs(2)?,
            service_s: secs(3)?,
            own_change_s: secs(5)?,
            lte_rsrp: stats[0],
            lte_rsrq: stats[1],
            lte_sinr: stats[2],
            nr_rsrp: stats[3],
            nr_rsrq: stats[4],
            nr_sinr: stats[5],
            nr_share: (service_ms > 0).then(|| (nr_ms as f64 / service_ms as f64).min(1.0)),
            lte_band: bands.get(&b).copied(),
            allowed_bands: allowed.get(&b).cloned(),
            cell_changes: r.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u32,
        })
    })?;
    rows.collect()
}

/// Speed tests in the range, oldest first.
pub fn speedtests(c: &Connection, range: &HistoryRange) -> rusqlite::Result<Vec<SpeedTestResult>> {
    let mut st = c.prepare(
        "SELECT record_json FROM speedtest WHERE at_ms >= ?1 AND at_ms <= ?2
         ORDER BY at_ms, id",
    )?;
    let rows = st.query_map(params![range.from_ms as i64, range.to_ms as i64], |r| {
        r.get::<_, String>(0)
    })?;
    let mut out = Vec::new();
    for j in rows {
        if let Ok(r) = serde_json::from_str(&j?) {
            out.push(r);
        }
    }
    Ok(out)
}

/// What the overview counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub first_ms: Option<u64>,
    pub last_ms: Option<u64>,
    pub minutes: u64,
    pub speedtests: u64,
    pub events: u64,
}

pub fn counts(c: &Connection) -> rusqlite::Result<Counts> {
    let span = |table: &str, col: &str| -> rusqlite::Result<(Option<i64>, Option<i64>, i64)> {
        c.query_row(
            &format!("SELECT MIN({col}), MAX({col}), COUNT(*) FROM {table}"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
    };
    let m = span("signal_minute", "minute_ms")?;
    let s = span("speedtest", "at_ms")?;
    let e = span("event", "at_ms")?;
    let all = [m, s, e];
    let first = all.iter().filter_map(|x| x.0).min();
    // A minute row's time is its start; it lasts until the minute ends.
    let last = [m.1.map(|t| t + MINUTE_MS as i64 - 1), s.1, e.1]
        .into_iter()
        .flatten()
        .max();
    Ok(Counts {
        first_ms: first.map(|t| t.max(0) as u64),
        last_ms: last.map(|t| t.max(0) as u64),
        minutes: m.2.max(0) as u64,
        speedtests: s.2.max(0) as u64,
        events: e.2.max(0) as u64,
    })
}

// ---------------------------------------------------------------------------------------------
// The event log

pub const EVENTS_DEFAULT_LIMIT: u32 = 100;
pub const EVENTS_MAX_LIMIT: u32 = 200;
/// Records read per round. A change is about 20 records, so a round holds many.
const CHUNK: usize = 2000;
/// Records after the page's end read to finish a change that started before it.
const TAIL: usize = 200;
/// Rounds per page, so a filter that matches nothing can't read the whole file at once.
const MAX_ROUNDS: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EventQuery {
    pub range: HistoryRange,
    /// Only entries that started before this; `None` for the first page.
    #[serde(default)]
    #[ts(optional = nullable, type = "number | null")]
    pub before_ms: Option<u64>,
    /// 1 to 200; 100 when missing.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
    /// Empty: every category.
    #[serde(default)]
    pub categories: Vec<EventCategory>,
    /// Only warnings and problems, and band changes that didn't keep.
    #[serde(default)]
    pub problems_only: bool,
    /// Also internal steps and bookkeeping.
    #[serde(default)]
    pub technical: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EventPage {
    /// Newest first.
    pub entries: Vec<EventEntry>,
    /// Pass as `beforeMs` for the next page; `None` when there is nothing older.
    #[ts(type = "number | null")]
    pub next_before_ms: Option<u64>,
}

fn entry_ms(e: &EventEntry) -> u64 {
    match e {
        EventEntry::Single { item } => item.at_ms,
        EventEntry::Change { group } => group.started_ms,
    }
}

fn wanted(e: &EventEntry, q: &EventQuery) -> bool {
    match e {
        EventEntry::Single { item } => {
            (q.categories.is_empty() || q.categories.contains(&item.category))
                && (q.technical || !item.technical)
                && (!q.problems_only
                    || matches!(item.severity, Severity::Warning | Severity::Problem))
        }
        EventEntry::Change { group } => {
            (q.categories.is_empty() || q.categories.contains(&EventCategory::BandChange))
                && (!q.problems_only
                    || matches!(
                        group.outcome,
                        ChangeOutcome::Reverted { .. }
                            | ChangeOutcome::RevertFailed
                            | ChangeOutcome::Rejected
                    ))
        }
    }
}

fn read_records(
    st: &mut rusqlite::Statement<'_>,
    args: &[i64],
) -> rusqlite::Result<Vec<EventRecord>> {
    let rows = st.query_map(rusqlite::params_from_iter(args.iter()), |r| {
        let detail: String = r.get(2)?;
        Ok(EventRecord {
            at_ms: r.get::<_, i64>(0)?.max(0) as u64,
            kind: r.get(1)?,
            detail: serde_json::from_str(&detail).unwrap_or(serde_json::Value::Null),
        })
    })?;
    rows.collect()
}

fn is_change_step(kind: &str) -> bool {
    kind.starts_with("safe_apply.") || kind.starts_with("journal.")
}

fn ends_change(kind: &str) -> bool {
    matches!(
        kind,
        kinds::SAFE_APPLY_COMMITTED
            | kinds::SAFE_APPLY_REVERTED
            | kinds::SAFE_APPLY_REVERT_FAILED
            | kinds::SAFE_APPLY_REJECTED
    )
}

/// One page of the event log, newest first, with each band change folded into one entry.
///
/// Records are read newest first in rounds. A change that started before the page's end and
/// finished after it is completed from the records after the end, so a change is never split
/// between two pages. When a round is full, the entries before its oldest change are left for
/// the next page: their change may have started further back.
pub fn events(c: &Connection, q: &EventQuery) -> rusqlite::Result<EventPage> {
    let limit = q
        .limit
        .unwrap_or(EVENTS_DEFAULT_LIMIT)
        .clamp(1, EVENTS_MAX_LIMIT) as usize;
    let from = q.range.from_ms as i64;
    let mut before = q
        .before_ms
        .unwrap_or(q.range.to_ms.saturating_add(1))
        .min(q.range.to_ms.saturating_add(1)) as i64;
    let mut chunk_st = c.prepare(
        "SELECT at_ms, kind, detail_json FROM event WHERE at_ms >= ?1 AND at_ms < ?2
         ORDER BY at_ms DESC, id DESC LIMIT ?3",
    )?;
    let mut tail_st = c.prepare(
        "SELECT at_ms, kind, detail_json FROM event
         WHERE at_ms >= ?1 AND (kind LIKE 'safe\\_apply.%' ESCAPE '\\' OR kind LIKE 'journal.%')
         ORDER BY at_ms, id LIMIT ?2",
    )?;
    let mut out: Vec<EventEntry> = Vec::new();
    for _ in 0..MAX_ROUNDS {
        if before <= from {
            return Ok(EventPage {
                entries: out,
                next_before_ms: None,
            });
        }
        let mut records = read_records(&mut chunk_st, &[from, before, CHUNK as i64])?;
        let full = records.len() == CHUNK;
        records.reverse();
        let oldest = records.first().map_or(from, |r| r.at_ms as i64);
        let mut tail = read_records(&mut tail_st, &[before, TAIL as i64])?;
        if let Some(end) = tail.iter().position(|r| ends_change(&r.kind)) {
            tail.truncate(end + 1);
        }
        records.extend(tail.into_iter().filter(|r| is_change_step(&r.kind)));
        let entries = event_text::group_changes(&records);
        // In a full round, a change may have started before its oldest record.
        let floor = if full {
            entries
                .iter()
                .filter_map(|e| match e {
                    EventEntry::Change { group } => Some(group.started_ms as i64),
                    EventEntry::Single { .. } => None,
                })
                .min()
                .unwrap_or(oldest)
                .max(oldest)
        } else {
            from
        };
        let mut page: Vec<EventEntry> = entries
            .into_iter()
            .filter(|e| {
                let t = entry_ms(e) as i64;
                t < before && t >= floor
            })
            .collect();
        page.reverse();
        // Same time: the newer record came later in the file.
        page.sort_by_key(|e| std::cmp::Reverse(entry_ms(e)));
        for e in page.into_iter().filter(|e| wanted(e, q)) {
            if out.len() >= limit && out.last().map(entry_ms) != Some(entry_ms(&e)) {
                let next = out.last().map(entry_ms);
                return Ok(EventPage {
                    entries: out,
                    next_before_ms: next,
                });
            }
            out.push(e);
        }
        if !full {
            return Ok(EventPage {
                entries: out,
                next_before_ms: None,
            });
        }
        // The next round reads from the floor; entries before it weren't taken.
        before = if floor < before { floor } else { before - 1 };
    }
    Ok(EventPage {
        entries: out,
        next_before_ms: Some(before.max(0) as u64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::sampler::{upsert_minute, MinuteRow, Stat};
    use crate::history::schema;
    use crate::history::store::{insert_event_on, router_id};
    use serde_json::json;

    fn db() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        schema::migrate(&mut c).unwrap();
        c
    }

    fn range(from: u64, to: u64, off: i32) -> HistoryRange {
        HistoryRange {
            from_ms: from,
            to_ms: to,
            utc_offset_min: off,
        }
    }

    fn stat(v: i32) -> Stat {
        Stat {
            min: Some(v),
            avg: Some(v),
            max: Some(v),
        }
    }

    fn minute(t: u64, service_ms: u64, rsrp_tenths: i32, band: u16) -> MinuteRow {
        MinuteRow {
            minute_ms: t,
            alive_ms: 60_000,
            observed_ms: 60_000,
            service_ms,
            lte_rsrp: stat(rsrp_tenths),
            lte_band: Some(band),
            allowed_bands: Some("4".into()),
            ..MinuteRow::default()
        }
    }

    // 2026-11-01T00:00:00Z
    const DAY0: u64 = 1_793_491_200_000;

    #[test]
    fn auto_picks_the_smallest_bucket_with_at_most_800_points() {
        let r = |span: u64| range(DAY0, DAY0 + span - 1, 0);
        assert_eq!(resolve_bucket(Bucket::Auto, &r(HOUR_MS)).0, Bucket::Min1);
        assert_eq!(
            resolve_bucket(Bucket::Auto, &r(800 * MINUTE_MS)).0,
            Bucket::Min1
        );
        assert_eq!(
            resolve_bucket(Bucket::Auto, &r(801 * MINUTE_MS)).0,
            Bucket::Min5
        );
        assert_eq!(resolve_bucket(Bucket::Auto, &r(DAY_MS)).0, Bucket::Min5);
        assert_eq!(
            resolve_bucket(Bucket::Auto, &r(7 * DAY_MS)).0,
            Bucket::Min15
        );
        assert_eq!(
            resolve_bucket(Bucket::Auto, &r(30 * DAY_MS)).0,
            Bucket::Hour1
        );
        assert_eq!(
            resolve_bucket(Bucket::Auto, &r(90 * DAY_MS)).0,
            Bucket::Hour6
        );
        assert_eq!(
            resolve_bucket(Bucket::Auto, &r(365 * DAY_MS)).0,
            Bucket::Day1
        );
    }

    #[test]
    fn no_bucket_gives_more_than_1000_points() {
        let week = range(DAY0, DAY0 + 7 * DAY_MS - 1, 0);
        // 10080 one-minute points: made coarser.
        assert_eq!(
            resolve_bucket(Bucket::Min1, &week),
            (Bucket::Min15, 15 * MINUTE_MS)
        );
        assert_eq!(
            resolve_bucket(Bucket::Hour1, &week),
            (Bucket::Hour1, HOUR_MS)
        );
        let ten_years = range(DAY0, DAY0 + 3650 * DAY_MS - 1, 0);
        let (b, ms) = resolve_bucket(Bucket::Auto, &ten_years);
        assert_eq!(b, Bucket::Day1);
        assert_eq!(ms, 4 * DAY_MS);
        assert!(points(3650 * DAY_MS, ms) <= MAX_POINTS);
    }

    #[test]
    fn buckets_follow_local_time() {
        // 22:30 UTC is 01:30 the next day in UTC+3.
        let t = DAY0 + 22 * HOUR_MS + 30 * MINUTE_MS;
        assert_eq!(bucket_start(t, DAY_MS, 0), DAY0);
        assert_eq!(bucket_start(t, DAY_MS, 180 * 60_000), DAY0 + 21 * HOUR_MS);
        // UTC+5:30 moves hour buckets by half an hour.
        assert_eq!(
            bucket_start(t, HOUR_MS, 330 * 60_000),
            DAY0 + 22 * HOUR_MS + 30 * MINUTE_MS
        );
        assert_eq!(bucket_start(t, HOUR_MS, -60 * 60_000), DAY0 + 22 * HOUR_MS);
    }

    fn seed(c: &Connection, rows: &[MinuteRow]) {
        let id = router_id(c, "192.168.8.1", "mock").unwrap();
        for r in rows {
            upsert_minute(c, id, r).unwrap();
        }
    }

    #[test]
    fn hour_buckets_weigh_averages_by_service_time() {
        let c = db();
        seed(
            &c,
            &[
                // 60 s at -90.0 dB and 20 s at -110.0 dB: average -95.0.
                minute(DAY0, 60_000, -900, 3),
                minute(DAY0 + MINUTE_MS, 20_000, -1100, 3),
                // Next hour.
                minute(DAY0 + HOUR_MS, 60_000, -800, 1),
            ],
        );
        let s = signal(&c, &range(DAY0, DAY0 + 2 * HOUR_MS - 1, 0), Bucket::Hour1).unwrap();
        assert_eq!(s.bucket_ms, HOUR_MS);
        assert_eq!(s.points.len(), 2);
        let p = &s.points[0];
        assert_eq!(p.t_ms, DAY0);
        assert_eq!(p.lte_rsrp.avg, Some(-95.0));
        assert_eq!(p.lte_rsrp.min, Some(-110.0));
        assert_eq!(p.lte_rsrp.max, Some(-90.0));
        assert_eq!(p.alive_s, 120);
        assert_eq!(p.service_s, 80);
        assert_eq!(p.lte_band, Some(3));
        assert_eq!(p.allowed_bands.as_deref(), Some("4"));
        assert_eq!(p.nr_rsrp, Stat3::default());
        assert_eq!(p.nr_share, Some(0.0));
        assert_eq!(s.points[1].lte_band, Some(1));
    }

    #[test]
    fn an_offset_of_180_moves_the_day_boundary() {
        let c = db();
        // 22:00 and 23:00 UTC on day 0: day 0 in UTC, day 1 (01:00, 02:00) in UTC+3.
        seed(
            &c,
            &[
                minute(DAY0 + 20 * HOUR_MS, 60_000, -900, 3),
                minute(DAY0 + 22 * HOUR_MS, 60_000, -1000, 3),
                minute(DAY0 + 23 * HOUR_MS, 60_000, -1000, 3),
            ],
        );
        let all = range(DAY0 - DAY_MS, DAY0 + 2 * DAY_MS, 0);
        let utc = signal(&c, &all, Bucket::Day1).unwrap();
        assert_eq!(utc.points.len(), 1);
        assert_eq!(utc.points[0].t_ms, DAY0);
        let ksa = signal(
            &c,
            &HistoryRange {
                utc_offset_min: 180,
                ..all
            },
            Bucket::Day1,
        )
        .unwrap();
        assert_eq!(
            ksa.points.iter().map(|p| p.t_ms).collect::<Vec<_>>(),
            vec![DAY0 - 3 * HOUR_MS, DAY0 + 21 * HOUR_MS]
        );
        assert_eq!(ksa.points[1].lte_rsrp.avg, Some(-100.0));
        assert_eq!(ksa.points[0].alive_s, 60);
    }

    #[test]
    fn own_changes_and_nr_share_are_counted() {
        let c = db();
        let mut a = minute(DAY0, 60_000, -900, 3);
        a.flags = FLAG_BAND_CHANGE;
        a.nr_ms = 30_000;
        a.nr_rsrp = stat(-950);
        let mut b = minute(DAY0 + MINUTE_MS, 60_000, -900, 3);
        b.flags = 4; // a speed test: not the app's own change
        b.cell_changes = 2;
        let mut ip = minute(DAY0 + 2 * MINUTE_MS, 60_000, -900, 3);
        ip.flags = FLAG_IP_REFRESH;
        ip.service_ms = 0;
        seed(&c, &[a, b, ip]);
        let s = signal(&c, &range(DAY0, DAY0 + HOUR_MS, 0), Bucket::Hour1).unwrap();
        let p = &s.points[0];
        assert_eq!(p.own_change_s, 120, "band change and Refresh IP minutes");
        assert_eq!(p.nr_share, Some(0.25));
        assert_eq!(p.nr_rsrp.avg, Some(-95.0), "weighted by NR time only");
        assert_eq!(p.cell_changes, 2);
    }

    #[test]
    fn counts_cover_all_three_tables() {
        let c = db();
        assert_eq!(counts(&c).unwrap(), Counts::default());
        seed(&c, &[minute(DAY0, 60_000, -900, 3)]);
        insert_event_on(
            &c,
            &EventRecord {
                at_ms: DAY0 + 5 * MINUTE_MS,
                kind: kinds::APP_STARTED.into(),
                detail: json!({}),
            },
        )
        .unwrap();
        let n = counts(&c).unwrap();
        assert_eq!(n.first_ms, Some(DAY0));
        assert_eq!(n.last_ms, Some(DAY0 + 5 * MINUTE_MS));
        assert_eq!((n.minutes, n.speedtests, n.events), (1, 0, 1));
    }

    // -- events

    fn ev(c: &Connection, at: u64, kind: &str, detail: serde_json::Value) {
        insert_event_on(
            c,
            &EventRecord {
                at_ms: at,
                kind: kind.into(),
                detail,
            },
        )
        .unwrap();
    }

    fn config(bands: &[u8]) -> serde_json::Value {
        json!({
            "networkMode": "auto",
            "lteBands": router_core::LteBandSet::from_bands(bands).unwrap(),
            "legacyBandRaw": "3FFFFFFF",
            "nrOption": null,
        })
    }

    /// A kept change: snapshot read at `t`, then started, post, committed.
    fn change(c: &Connection, t: u64, end_kind: &str) {
        ev(
            c,
            t,
            kinds::SAFE_APPLY_READ,
            json!({"phase": {"kind": "snapshotting"}, "config": config(&[1, 3])}),
        );
        ev(c, t + 1, kinds::SAFE_APPLY_STARTED, json!({}));
        ev(
            c,
            t + 2,
            kinds::SAFE_APPLY_POST,
            json!({"phase": {"kind": "applying"}, "config": config(&[3])}),
        );
        let detail = if end_kind == kinds::SAFE_APPLY_REVERTED {
            json!({"reason": "noReattach"})
        } else {
            json!({})
        };
        ev(c, t + 50, end_kind, detail);
    }

    fn q(limit: u32, before: Option<u64>) -> EventQuery {
        EventQuery {
            range: range(0, DAY0 * 2, 180),
            before_ms: before,
            limit: Some(limit),
            categories: Vec::new(),
            problems_only: false,
            technical: false,
        }
    }

    fn times(p: &EventPage) -> Vec<u64> {
        p.entries.iter().map(entry_ms).collect()
    }

    #[test]
    fn events_come_newest_first_with_changes_folded() {
        let c = db();
        ev(&c, 100, kinds::APP_STARTED, json!({"version": "1"}));
        change(&c, 200, kinds::SAFE_APPLY_COMMITTED);
        ev(&c, 300, kinds::APP_STOPPED, json!({}));
        let p = events(&c, &q(100, None)).unwrap();
        assert_eq!(times(&p), vec![300, 200, 100]);
        assert_eq!(p.next_before_ms, None);
        let EventEntry::Change { group } = &p.entries[1] else {
            panic!("a change")
        };
        assert_eq!(group.outcome, ChangeOutcome::Kept);
        assert_eq!(group.from, vec![1, 3]);
        assert_eq!(group.to, vec![3]);
    }

    #[test]
    fn events_page_without_splitting_a_change() {
        let c = db();
        for i in 0..5u64 {
            ev(
                &c,
                1000 * i + 10,
                kinds::APP_STARTED,
                json!({"version": "1"}),
            );
        }
        change(&c, 2500, kinds::SAFE_APPLY_COMMITTED);
        let first = events(&c, &q(3, None)).unwrap();
        assert_eq!(times(&first), vec![4010, 3010, 2500]);
        assert_eq!(first.next_before_ms, Some(2500));
        let second = events(&c, &q(3, first.next_before_ms)).unwrap();
        assert_eq!(times(&second), vec![2010, 1010, 10]);
        assert_eq!(second.next_before_ms, None, "nothing older");

        // A page that ends inside a change still shows the whole change.
        let p = events(&c, &q(100, Some(2520))).unwrap();
        let EventEntry::Change { group } = &p.entries[0] else {
            panic!("a change")
        };
        assert_eq!(
            group.outcome,
            ChangeOutcome::Kept,
            "finished after the page's end"
        );
        assert_eq!(group.ended_ms, Some(2550));
    }

    #[test]
    fn events_filter_by_category_problems_and_technical() {
        let c = db();
        change(&c, 100, kinds::SAFE_APPLY_COMMITTED);
        change(&c, 1000, kinds::SAFE_APPLY_REVERTED);
        ev(&c, 2000, kinds::APP_STARTED, json!({"version": "1"}));
        ev(&c, 2100, kinds::SESSION_REFRESHED, json!({"ok": true}));
        let mut query = q(100, None);
        query.categories = vec![EventCategory::BandChange];
        assert_eq!(times(&events(&c, &query).unwrap()), vec![1000, 100]);
        query.problems_only = true;
        assert_eq!(times(&events(&c, &query).unwrap()), vec![1000]);

        // A session refresh is bookkeeping, shown only with technical events.
        assert_eq!(
            times(&events(&c, &q(100, None)).unwrap()),
            vec![2000, 1000, 100]
        );
        let tech = EventQuery {
            technical: true,
            ..q(100, None)
        };
        assert_eq!(
            times(&events(&c, &tech).unwrap()),
            vec![2100, 2000, 1000, 100]
        );
    }

    #[test]
    fn a_limit_is_clamped_and_ties_stay_together() {
        let c = db();
        for i in 0..3 {
            ev(&c, 500, kinds::APP_STARTED, json!({"version": i}));
        }
        ev(&c, 400, kinds::APP_STARTED, json!({"version": "x"}));
        let p = events(&c, &q(1, None)).unwrap();
        assert_eq!(times(&p), vec![500, 500, 500], "same instant, one page");
        assert_eq!(p.next_before_ms, Some(500));
        let mut zero = q(0, None);
        zero.limit = Some(0);
        assert_eq!(events(&c, &zero).unwrap().entries.len(), 3);
    }

    #[test]
    fn many_records_are_read_in_rounds() {
        let c = db();
        let tx = c.unchecked_transaction().unwrap();
        for i in 0..(CHUNK as u64 + 500) {
            ev(&tx, 10 + i, kinds::APP_STARTED, json!({"version": i}));
        }
        tx.commit().unwrap();
        let mut seen = 0;
        let mut before = None;
        loop {
            let p = events(&c, &q(200, before)).unwrap();
            seen += p.entries.len();
            match p.next_before_ms {
                Some(b) => before = Some(b),
                None => break,
            }
        }
        assert_eq!(seen, CHUNK + 500);
    }

    #[test]
    fn the_range_limits_events() {
        let c = db();
        ev(&c, 100, kinds::APP_STARTED, json!({"version": "1"}));
        ev(&c, 200, kinds::APP_STARTED, json!({"version": "2"}));
        ev(&c, 300, kinds::APP_STARTED, json!({"version": "3"}));
        let mut query = q(100, None);
        query.range = range(150, 250, 0);
        assert_eq!(times(&events(&c, &query).unwrap()), vec![200]);
    }
}
