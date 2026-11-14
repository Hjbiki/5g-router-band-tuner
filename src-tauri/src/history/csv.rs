//! The CSV export, for operator complaints and spreadsheets.
//!
//! The file is UTF-8 with a BOM and CRLF line ends, quoted as RFC 4180 says, with no `sep=`
//! line, so Excel opens it with Arabic headers intact. The separator is the Windows list
//! separator (`,` or `;`). Numbers use Western digits and a `.` decimal, and stay numbers; a
//! text cell that starts like a formula gets a leading `'`.
//!
//! Headers and values come from the UI's language (`CsvLabels`); a missing one falls back to
//! English. PCI and channel columns appear only when the user ticks the box, and so does the
//! IP range column (the first two numbers only; nothing more is stored). Cell, eNB and TAC
//! IDs aren't stored, so they can't be exported. Free text (error messages and the like) is
//! never exported, server names go through [`scrub_cell`], and [`router_core::diagnostics::
//! identifier_hits`] scans every file before anything is written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use router_core::diagnostics::identifier_hits;
use router_core::event_text::{
    self, ChangeGroup, ChangeOutcome, EventCategory, EventEntry, EventItem, EventParam, Severity,
};
use router_core::{EventRecord, LteBandSet};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::query::OWN_CHANGE;
use super::query::{self, HistoryRange, SignalPoint};
use super::sampler::{
    FLAG_BAND_CHANGE, FLAG_IP_REFRESH, FLAG_NET_CHANGE, FLAG_SCAN, FLAG_SPEED_TEST,
};
use crate::speedtest::SpeedTestResult;

const HOUR_MS: u64 = 3_600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportDataset {
    /// One row per hour: the complaint-friendly summary.
    Hourly,
    SpeedTests,
    /// One row per recorded minute.
    Minutes,
    Events,
    /// All four, as four files in one folder.
    All,
}

impl ExportDataset {
    /// The part of the file name after `5g-band-tuner-`.
    pub fn file_part(self) -> &'static str {
        match self {
            Self::Hourly => "hourly",
            Self::SpeedTests => "speed-tests",
            Self::Minutes => "minutes",
            Self::Events => "events",
            Self::All => "all",
        }
    }

    fn parts(self) -> Vec<Self> {
        match self {
            Self::All => vec![Self::Hourly, Self::SpeedTests, Self::Minutes, Self::Events],
            one => vec![one],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportOptions {
    pub range: HistoryRange,
    pub dataset: ExportDataset,
    /// Adds the LTE PCI, EARFCN, 5G PCI and NR-ARFCN columns. Off unless ticked each time.
    pub include_cell_numbers: bool,
    /// Adds the IP range column ("10.45.x.x") to the speed-test and minute files. Off unless
    /// ticked each time.
    #[serde(default)]
    pub include_ip_range: bool,
}

/// Text in the UI's language. Keys: column ids (`headers`), value ids (`values`), and the event
/// log's i18n keys (`texts`, such as `events.kind.appStarted`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct CsvLabels {
    pub headers: BTreeMap<String, String>,
    pub values: BTreeMap<String, String>,
    pub texts: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ExportOutcome {
    Cancelled,
    /// File names only, never the folder.
    Saved {
        files: Vec<String>,
    },
}

// ---------------------------------------------------------------------------------------------
// The writer

/// One cell. Numbers are written as numbers, never guarded or quoted.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Text(String),
    /// A value and its decimals.
    Num(Option<f64>, usize),
    Int(Option<i64>),
}

fn text(s: impl Into<String>) -> Cell {
    Cell::Text(s.into())
}

fn num(v: Option<f64>, decimals: usize) -> Cell {
    Cell::Num(v, decimals)
}

fn int<T: Into<i64>>(v: Option<T>) -> Cell {
    Cell::Int(v.map(Into::into))
}

fn format_num(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return String::new();
    }
    let s = format!("{v:.decimals$}");
    // "-0.0" reads oddly in a spreadsheet.
    match s.strip_prefix('-') {
        Some(rest) if rest.bytes().all(|b| b == b'0' || b == b'.') => rest.to_string(),
        _ => s,
    }
}

fn text_field(s: &str, sep: char) -> String {
    let mut v = String::with_capacity(s.len() + 2);
    if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        v.push('\'');
    }
    v.push_str(s);
    if v.contains([sep, '"', '\r', '\n']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

/// The field separator and the decimal mark of number cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub sep: char,
    pub decimal: char,
}

impl From<char> for Format {
    /// A separator with `.` decimals.
    fn from(sep: char) -> Self {
        Self { sep, decimal: '.' }
    }
}

/// The decimal mark goes with the separator: `,` only when the separator is `;` and the
/// Windows decimal is `,` (de-DE, fr-FR...), where Excel would read `-95.5` as text and `1.5`
/// as a date. Everything else keeps `.`.
pub fn pick_format(sep: char, locale_decimal: &str) -> Format {
    let decimal = if sep == ';' && locale_decimal.trim() == "," {
        ','
    } else {
        '.'
    };
    Format { sep, decimal }
}

fn field(c: &Cell, fmt: impl Into<Format>) -> String {
    let fmt = fmt.into();
    match c {
        Cell::Text(s) => text_field(s, fmt.sep),
        Cell::Num(Some(v), d) if fmt.decimal != '.' => {
            format_num(*v, *d).replace('.', &fmt.decimal.to_string())
        }
        Cell::Num(Some(v), d) => format_num(*v, *d),
        Cell::Int(Some(i)) => i.to_string(),
        Cell::Num(None, _) | Cell::Int(None) => String::new(),
    }
}

/// The whole file: BOM, header line, rows, each line ended by CRLF.
pub fn render(headers: &[String], rows: &[Vec<Cell>], fmt: impl Into<Format>) -> String {
    let fmt = fmt.into();
    let sep = fmt.sep;
    let mut out = String::from("\u{feff}");
    let line = |cells: Vec<String>| cells.join(&sep.to_string()) + "\r\n";
    out.push_str(&line(headers.iter().map(|h| text_field(h, sep)).collect()));
    for row in rows {
        out.push_str(&line(row.iter().map(|c| field(c, fmt)).collect()));
    }
    out
}

/// `,` or `;` from the Windows setting; anything else gives `,`.
pub fn pick_separator(list_sep: &str) -> char {
    match list_sep.trim() {
        ";" => ';',
        _ => ',',
    }
}

/// The Windows list separator (`LOCALE_SLIST`), so a double-click opens the file in columns.
#[cfg(windows)]
pub fn list_separator() -> char {
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SLIST};
    let mut buf = [0u16; 8];
    // SAFETY: a null locale name means the user's default locale; the buffer and its length match.
    let n = unsafe {
        GetLocaleInfoEx(
            std::ptr::null(),
            LOCALE_SLIST,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if n <= 1 {
        return ',';
    }
    pick_separator(&String::from_utf16_lossy(&buf[..(n - 1) as usize]))
}

#[cfg(not(windows))]
pub fn list_separator() -> char {
    ','
}

/// The Windows decimal mark (`LOCALE_SDECIMAL`).
#[cfg(windows)]
fn locale_decimal() -> String {
    use windows_sys::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SDECIMAL};
    let mut buf = [0u16; 8];
    // SAFETY: a null locale name means the user's default locale; the buffer and its length match.
    let n = unsafe {
        GetLocaleInfoEx(
            std::ptr::null(),
            LOCALE_SDECIMAL,
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if n <= 1 {
        return ".".into();
    }
    String::from_utf16_lossy(&buf[..(n - 1) as usize])
}

#[cfg(not(windows))]
fn locale_decimal() -> String {
    ".".into()
}

/// The separator and decimal mark for this PC's Excel.
pub fn locale_format() -> Format {
    pick_format(list_separator(), &locale_decimal())
}

/// Drops every word the leak scan would flag, such as an IP address in a server name.
pub fn scrub_cell(s: &str) -> String {
    s.split_whitespace()
        .filter(|w| identifier_hits(w).is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------------------------------------
// Times

/// `UTC+03:00`, `UTC-04:30`.
pub fn offset_label(offset_min: i32) -> String {
    let sign = if offset_min < 0 { '-' } else { '+' };
    let m = offset_min.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", m / 60, m % 60)
}

fn utc(ms: u64) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::from_timestamp_millis(i64::try_from(ms).ok()?)
}

/// `YYYY-MM-DD HH:MM` in the user's time.
pub fn local_time(ms: u64, offset_min: i32) -> String {
    let Some(off) = chrono::FixedOffset::east_opt(offset_min.saturating_mul(60)) else {
        return String::new();
    };
    utc(ms)
        .map(|t| t.with_timezone(&off).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// ISO 8601 in UTC: `2026-11-01T10:00:00Z`.
pub fn utc_time(ms: u64) -> String {
    utc(ms)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

/// `YYYY-MM-DD` in the user's time, for file names.
pub fn local_date(ms: u64, offset_min: i32) -> String {
    local_time(ms, offset_min)
        .split(' ')
        .next()
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------------------------
// Columns

/// A column: its id (the key in `CsvLabels::headers`), its English header, and whether it's one
/// of the cell-number columns or the IP range column, each behind its own checkbox.
#[derive(Debug, Clone, Copy)]
pub struct Col {
    pub id: &'static str,
    pub en: &'static str,
    pub cell_number: bool,
    pub ip_range: bool,
}

const fn col(id: &'static str, en: &'static str) -> Col {
    Col {
        id,
        en,
        cell_number: false,
        ip_range: false,
    }
}

const fn cell_col(id: &'static str, en: &'static str) -> Col {
    Col {
        id,
        en,
        cell_number: true,
        ip_range: false,
    }
}

const IP_RANGE: Col = Col {
    id: "ipRange",
    en: "IP range (first two numbers)",
    cell_number: false,
    ip_range: true,
};

const TIME: [Col; 2] = [
    col("localTime", "Local time ({offset})"),
    col("utcTime", "UTC time"),
];

const CELLS: [Col; 4] = [
    cell_col("ltePci", "LTE PCI"),
    cell_col("lteEarfcn", "LTE EARFCN"),
    cell_col("nrPci", "5G PCI"),
    cell_col("nrArfcn", "5G NR-ARFCN"),
];

pub const HOURLY: &[Col] = &[
    TIME[0],
    TIME[1],
    col("recordedMin", "Minutes recorded"),
    col("serviceMin", "Minutes in service"),
    col("noServiceMin", "Minutes without service"),
    col("ownChangeMin", "Minutes changing bands (this app)"),
    col("nrSharePct", "Time on 5G (%)"),
    col("lteRsrpAvg", "LTE RSRP average (dBm)"),
    col("lteRsrpMin", "LTE RSRP lowest (dBm)"),
    col("lteRsrqAvg", "LTE RSRQ average (dB)"),
    col("lteSinrAvg", "LTE SINR average (dB)"),
    col("lteSinrMin", "LTE SINR lowest (dB)"),
    col("nrRsrpAvg", "5G RSRP average (dBm)"),
    col("nrSinrAvg", "5G SINR average (dB)"),
    col("lteBand", "LTE band"),
    col("nrBand", "5G band"),
    col("allowedBands", "Allowed bands"),
    col("cellChanges", "Cell changes"),
    col("speedTests", "Speed tests"),
    col("downloadMedian", "Download median (Mbps)"),
    col("uploadMedian", "Upload median (Mbps)"),
    col("pingMedian", "Idle ping median (ms)"),
    col("operator", "Operator"),
    CELLS[0],
    CELLS[1],
    CELLS[2],
    CELLS[3],
];

pub const SPEED_TESTS: &[Col] = &[
    TIME[0],
    TIME[1],
    col("source", "Started by"),
    col("provider", "Provider"),
    col("size", "Size"),
    col("downloadMbps", "Download (Mbps)"),
    col("uploadMbps", "Upload (Mbps)"),
    col("idlePingMs", "Idle ping (ms)"),
    col("pingDownMs", "Ping while downloading (ms)"),
    col("pingUpMs", "Ping while uploading (ms)"),
    col("dataMb", "Data used (MB)"),
    col("server", "Server"),
    col("lteBand", "LTE band"),
    col("nrBand", "5G band"),
    col("allowedBands", "Allowed bands"),
    col("valid", "Comparable"),
    IP_RANGE,
    CELLS[0],
    CELLS[1],
    CELLS[2],
    CELLS[3],
];

pub const MINUTES: &[Col] = &[
    TIME[0],
    TIME[1],
    col("recordedS", "Seconds recorded"),
    col("serviceS", "Seconds in service"),
    col("nrS", "Seconds on 5G"),
    col("lteRsrpAvg", "LTE RSRP average (dBm)"),
    col("lteRsrpMin", "LTE RSRP lowest (dBm)"),
    col("lteRsrpMax", "LTE RSRP highest (dBm)"),
    col("lteRsrqAvg", "LTE RSRQ average (dB)"),
    col("lteSinrAvg", "LTE SINR average (dB)"),
    col("lteSinrMin", "LTE SINR lowest (dB)"),
    col("lteSinrMax", "LTE SINR highest (dB)"),
    col("nrRsrpAvg", "5G RSRP average (dBm)"),
    col("nrRsrqAvg", "5G RSRQ average (dB)"),
    col("nrSinrAvg", "5G SINR average (dB)"),
    col("lteBand", "LTE band"),
    col("nrBand", "5G band"),
    col("allowedBands", "Allowed bands"),
    col("bars", "Signal bars"),
    col("cellChanges", "Cell changes"),
    col("activity", "App activity"),
    col("operator", "Operator"),
    IP_RANGE,
    CELLS[0],
    CELLS[1],
    CELLS[2],
    CELLS[3],
];

pub const EVENTS: &[Col] = &[
    TIME[0],
    TIME[1],
    col("category", "Category"),
    col("severity", "Severity"),
    col("event", "Event"),
    col("code", "Code"),
];

/// Every value id with its English text.
pub const VALUES: &[(&str, &str)] = &[
    ("yes", "Yes"),
    ("no", "No"),
    ("allBands", "All bands"),
    ("source.manual", "You"),
    ("source.scan", "Auto-scan"),
    ("source.auto", "Automatic test"),
    ("provider.cloudflare", "Cloudflare"),
    ("provider.ookla", "Ookla"),
    ("size.light", "Light"),
    ("size.full", "Full"),
    ("activity.bandChange", "Band change"),
    ("activity.autoScan", "Auto-scan"),
    ("activity.speedTest", "Speed test"),
    ("activity.ipRefresh", "IP refresh"),
    ("activity.apnChange", "APN change"),
    ("category.bandChange", "Band change"),
    ("category.connection", "Connection"),
    ("category.speedTest", "Speed test"),
    ("category.device", "Router"),
    ("category.app", "App"),
    ("severity.info", "Info"),
    ("severity.good", "Good"),
    ("severity.warning", "Warning"),
    ("severity.problem", "Problem"),
];

/// Event parameters that may hold free text (error messages): never exported.
const FREE_TEXT_PARAMS: &[&str] = &["message", "error", "detail", "fallbackDetail"];

struct Ctx<'a> {
    labels: &'a CsvLabels,
    offset_min: i32,
    cells: bool,
    ip_range: bool,
    fmt: Format,
}

impl Ctx<'_> {
    fn value(&self, id: &str) -> String {
        self.labels
            .values
            .get(id)
            .cloned()
            .or_else(|| {
                VALUES
                    .iter()
                    .find(|(k, _)| *k == id)
                    .map(|(_, v)| v.to_string())
            })
            .unwrap_or_else(|| id.to_string())
    }

    fn headers(&self, cols: &[Col]) -> Vec<String> {
        let offset = offset_label(self.offset_min);
        cols.iter()
            .filter(|c| self.wants(c))
            .map(|c| {
                self.labels
                    .headers
                    .get(c.id)
                    .map_or(c.en, String::as_str)
                    .replace("{offset}", &offset)
            })
            .collect()
    }

    /// Whether a column is written: the opt-in ones only when ticked.
    fn wants(&self, c: &Col) -> bool {
        (self.cells || !c.cell_number) && (self.ip_range || !c.ip_range)
    }

    /// Drops the opt-in cells unless they're wanted.
    fn row(&self, cols: &[Col], cells: Vec<Cell>) -> Vec<Cell> {
        debug_assert_eq!(cols.len(), cells.len());
        cols.iter()
            .zip(cells)
            .filter(|(c, _)| self.wants(c))
            .map(|(_, v)| v)
            .collect()
    }

    /// "10.45.x.x". Anything but two numbers was never stored, and is written empty here too.
    fn ip_range(&self, range: Option<&str>) -> Cell {
        text(
            crate::speedtest::clean_ip_range(range)
                .map(|r| format!("{r}.x.x"))
                .unwrap_or_default(),
        )
    }

    fn times(&self, ms: u64) -> [Cell; 2] {
        [text(local_time(ms, self.offset_min)), text(utc_time(ms))]
    }

    fn bands(&self, hex: Option<&str>) -> Cell {
        let Some(set) = hex.and_then(|h| LteBandSet::from_huawei_hex(h).ok()) else {
            return text("");
        };
        if set == LteBandSet::AUTO {
            text(self.value("allBands"))
        } else {
            text(set.label())
        }
    }

    fn yes_no(&self, v: bool) -> Cell {
        text(self.value(if v { "yes" } else { "no" }))
    }

    fn enum_value<T: Serialize>(&self, prefix: &str, v: &T) -> Cell {
        let name = serde_json::to_value(v)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        text(self.value(&format!("{prefix}.{name}")))
    }
}

fn band(prefix: &str, b: Option<i64>) -> Cell {
    text(b.map(|b| format!("{prefix}{b}")).unwrap_or_default())
}

fn db(v: Option<f64>) -> Cell {
    num(v, 1)
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// A built file: its dataset, its text and how many rows it has.
pub struct CsvFile {
    pub dataset: ExportDataset,
    pub text: String,
    pub rows: usize,
}

// ---------------------------------------------------------------------------------------------
// Hourly

/// The hour's extras that the History page doesn't need.
#[derive(Default)]
struct HourExtra {
    no_service_ms: i64,
    nr_band: Option<i64>,
    operator: Option<String>,
    lte_pci: Option<i64>,
    lte_earfcn: Option<i64>,
    nr_pci: Option<i64>,
    nr_arfcn: Option<i64>,
}

fn hour_extras(c: &Connection, range: &HistoryRange) -> rusqlite::Result<BTreeMap<i64, HourExtra>> {
    let args = query::bucket_args(range, HOUR_MS);
    let mut out: BTreeMap<i64, HourExtra> = BTreeMap::new();
    // Minutes the app itself caused (a band change, auto-scan, Refresh IP or APN switch) aren't
    // counted against the operator.
    let mut st = c.prepare(&format!(
        "SELECT ((minute_ms + ?3) / ?4) * ?4 - ?3 AS b,
                SUM(CASE WHEN flags & {own} = 0 THEN MAX(observed_ms - service_ms, 0) ELSE 0 END)
         FROM signal_minute WHERE minute_ms >= ?1 AND minute_ms <= ?2 GROUP BY b",
        own = OWN_CHANGE,
    ))?;
    let rows = st.query_map(params![args.0, args.1, args.2, args.3], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
    })?;
    for row in rows {
        let (b, ms) = row?;
        out.entry(b).or_default().no_service_ms = ms.unwrap_or(0);
    }
    type Set = fn(&mut HourExtra, i64);
    let ints: [(&str, Set); 5] = [
        ("nr_band", |e, v| e.nr_band = Some(v)),
        ("lte_pci", |e, v| e.lte_pci = Some(v)),
        ("lte_earfcn", |e, v| e.lte_earfcn = Some(v)),
        ("nr_pci", |e, v| e.nr_pci = Some(v)),
        ("nr_arfcn", |e, v| e.nr_arfcn = Some(v)),
    ];
    for (name, set) in ints {
        for (b, v) in query::modes::<i64>(c, name, "service_ms", args)? {
            set(out.entry(b).or_default(), v);
        }
    }
    for (b, v) in query::modes::<String>(c, "operator", "alive_ms", args)? {
        out.entry(b).or_default().operator = Some(v);
    }
    Ok(out)
}

fn minutes(ms: i64) -> Cell {
    num(Some(ms.max(0) as f64 / 60_000.0), 1)
}

fn hourly(c: &Connection, ctx: &Ctx<'_>, range: &HistoryRange) -> rusqlite::Result<CsvFile> {
    let points: BTreeMap<i64, SignalPoint> = query::buckets(c, range, HOUR_MS)?
        .into_iter()
        .map(|p| (p.t_ms as i64, p))
        .collect();
    let extras = hour_extras(c, range)?;
    let mut tests: BTreeMap<i64, Vec<SpeedTestResult>> = BTreeMap::new();
    for t in query::speedtests(c, range)? {
        let b = query::bucket_start(t.at_ms, HOUR_MS, range.offset_ms()) as i64;
        tests.entry(b).or_default().push(t);
    }
    let mut hours: Vec<i64> = points.keys().chain(tests.keys()).copied().collect();
    hours.sort_unstable();
    hours.dedup();
    let empty = HourExtra::default();
    let rows = hours
        .iter()
        .map(|h| {
            let p = points.get(h);
            let x = extras.get(h).unwrap_or(&empty);
            let valid: Vec<&SpeedTestResult> = tests
                .get(h)
                .map(|v| v.iter().filter(|t| t.invalid_reason.is_none()).collect())
                .unwrap_or_default();
            let med = |f: fn(&SpeedTestResult) -> Option<f64>| {
                median(valid.iter().filter_map(|t| f(t)).collect())
            };
            let [lt, ut] = ctx.times(*h as u64);
            let secs = |f: fn(&SignalPoint) -> u64| p.map(|p| f(p) as i64 * 1000).unwrap_or(0);
            let cells = vec![
                lt,
                ut,
                minutes(secs(|p| p.alive_s)),
                minutes(secs(|p| p.service_s)),
                minutes(x.no_service_ms),
                minutes(secs(|p| p.own_change_s)),
                num(p.and_then(|p| p.nr_share).map(|s| s * 100.0), 0),
                db(p.and_then(|p| p.lte_rsrp.avg)),
                db(p.and_then(|p| p.lte_rsrp.min)),
                db(p.and_then(|p| p.lte_rsrq.avg)),
                db(p.and_then(|p| p.lte_sinr.avg)),
                db(p.and_then(|p| p.lte_sinr.min)),
                db(p.and_then(|p| p.nr_rsrp.avg)),
                db(p.and_then(|p| p.nr_sinr.avg)),
                band("B", p.and_then(|p| p.lte_band).map(i64::from)),
                band("n", x.nr_band),
                ctx.bands(p.and_then(|p| p.allowed_bands.as_deref())),
                int(p.map(|p| i64::from(p.cell_changes))),
                int(Some(tests.get(h).map_or(0, Vec::len) as i64)),
                num(med(|t| t.download_mbps), 1),
                num(med(|t| t.upload_mbps), 1),
                num(med(|t| t.idle_ping_ms), 0),
                text(scrub_cell(x.operator.as_deref().unwrap_or(""))),
                int(x.lte_pci),
                int(x.lte_earfcn),
                int(x.nr_pci),
                int(x.nr_arfcn),
            ];
            ctx.row(HOURLY, cells)
        })
        .collect::<Vec<_>>();
    Ok(file(ctx, ExportDataset::Hourly, HOURLY, rows))
}

fn file(ctx: &Ctx<'_>, dataset: ExportDataset, cols: &[Col], rows: Vec<Vec<Cell>>) -> CsvFile {
    CsvFile {
        dataset,
        rows: rows.len(),
        text: render(&ctx.headers(cols), &rows, ctx.fmt),
    }
}

// ---------------------------------------------------------------------------------------------
// Speed tests

fn speed_tests(c: &Connection, ctx: &Ctx<'_>, range: &HistoryRange) -> rusqlite::Result<CsvFile> {
    let rows = query::speedtests(c, range)?
        .iter()
        .map(|t| {
            let [lt, ut] = ctx.times(t.at_ms);
            let s = &t.serving;
            let cells = vec![
                lt,
                ut,
                ctx.enum_value(
                    "source",
                    &t.source
                        .unwrap_or(crate::speedtest::SpeedTestSource::Manual),
                ),
                ctx.enum_value("provider", &t.provider),
                ctx.enum_value("size", &t.size),
                num(t.download_mbps, 1),
                num(t.upload_mbps, 1),
                num(t.idle_ping_ms, 0),
                num(t.ping_down_ms, 0),
                num(t.ping_up_ms, 0),
                num(Some(t.bytes_used as f64 / 1e6), 1),
                text(scrub_cell(t.server.as_deref().unwrap_or(""))),
                band("B", s.lte_band.map(i64::from)),
                band("n", s.nr_band.map(i64::from)),
                ctx.bands(t.allowed_bands.map(|b| b.to_huawei_hex()).as_deref()),
                ctx.yes_no(t.invalid_reason.is_none()),
                ctx.ip_range(t.ip_range.as_deref()),
                int(s.lte_pci),
                int(s.lte_earfcn),
                int(s.nr_pci),
                int(s.nr_arfcn),
            ];
            ctx.row(SPEED_TESTS, cells)
        })
        .collect();
    Ok(file(ctx, ExportDataset::SpeedTests, SPEED_TESTS, rows))
}

// ---------------------------------------------------------------------------------------------
// Minutes

fn minute_rows(c: &Connection, ctx: &Ctx<'_>, range: &HistoryRange) -> rusqlite::Result<CsvFile> {
    let mut st = c.prepare(
        "SELECT minute_ms, SUM(alive_ms), SUM(service_ms), SUM(nr_ms),
                SUM(lte_rsrp_avg * service_ms) * 1.0 / SUM(CASE WHEN lte_rsrp_avg IS NOT NULL
                    THEN service_ms END),
                MIN(lte_rsrp_min), MAX(lte_rsrp_max),
                SUM(lte_rsrq_avg * service_ms) * 1.0 / SUM(CASE WHEN lte_rsrq_avg IS NOT NULL
                    THEN service_ms END),
                SUM(lte_sinr_avg * service_ms) * 1.0 / SUM(CASE WHEN lte_sinr_avg IS NOT NULL
                    THEN service_ms END),
                MIN(lte_sinr_min), MAX(lte_sinr_max),
                SUM(nr_rsrp_avg * nr_ms) * 1.0 / SUM(CASE WHEN nr_rsrp_avg IS NOT NULL
                    THEN nr_ms END),
                SUM(nr_rsrq_avg * nr_ms) * 1.0 / SUM(CASE WHEN nr_rsrq_avg IS NOT NULL
                    THEN nr_ms END),
                SUM(nr_sinr_avg * nr_ms) * 1.0 / SUM(CASE WHEN nr_sinr_avg IS NOT NULL
                    THEN nr_ms END),
                MAX(lte_band), MAX(nr_band), MAX(allowed_bands), MAX(bars), SUM(cell_changes),
                MAX(flags), MAX(operator),
                MAX(lte_pci), MAX(lte_earfcn), MAX(nr_pci), MAX(nr_arfcn), MAX(ip_range)
         FROM signal_minute WHERE minute_ms >= ?1 AND minute_ms <= ?2
         GROUP BY minute_ms ORDER BY minute_ms",
    )?;
    let rows = st.query_map(params![range.from_ms as i64, range.to_ms as i64], |r| {
        let t: i64 = r.get(0)?;
        let secs = |i: usize| -> rusqlite::Result<Cell> {
            Ok(int(r.get::<_, Option<i64>>(i)?.map(|ms| ms / 1000)))
        };
        let avg = |i: usize| -> rusqlite::Result<Cell> {
            Ok(db(r.get::<_, Option<f64>>(i)?.map(|v| (v).round() / 10.0)))
        };
        let tenth = |i: usize| -> rusqlite::Result<Cell> {
            Ok(db(r.get::<_, Option<i64>>(i)?.map(|v| v as f64 / 10.0)))
        };
        let flags = r.get::<_, Option<i64>>(19)?.unwrap_or(0) as u8;
        let activity: Vec<String> = [
            (FLAG_BAND_CHANGE, "activity.bandChange"),
            (FLAG_SCAN, "activity.autoScan"),
            (FLAG_SPEED_TEST, "activity.speedTest"),
            (FLAG_IP_REFRESH, "activity.ipRefresh"),
            (FLAG_NET_CHANGE, "activity.apnChange"),
        ]
        .iter()
        .filter(|(f, _)| flags & f != 0)
        .map(|(_, k)| ctx.value(k))
        .collect();
        let [lt, ut] = ctx.times(t.max(0) as u64);
        let cells = vec![
            lt,
            ut,
            secs(1)?,
            secs(2)?,
            secs(3)?,
            avg(4)?,
            tenth(5)?,
            tenth(6)?,
            avg(7)?,
            avg(8)?,
            tenth(9)?,
            tenth(10)?,
            avg(11)?,
            avg(12)?,
            avg(13)?,
            band("B", r.get(14)?),
            band("n", r.get(15)?),
            ctx.bands(r.get::<_, Option<String>>(16)?.as_deref()),
            int(r.get::<_, Option<i64>>(17)?),
            int(r.get::<_, Option<i64>>(18)?),
            text(activity.join("; ")),
            text(scrub_cell(
                &r.get::<_, Option<String>>(20)?.unwrap_or_default(),
            )),
            ctx.ip_range(r.get::<_, Option<String>>(25)?.as_deref()),
            int(r.get::<_, Option<i64>>(21)?),
            int(r.get::<_, Option<i64>>(22)?),
            int(r.get::<_, Option<i64>>(23)?),
            int(r.get::<_, Option<i64>>(24)?),
        ];
        Ok(ctx.row(MINUTES, cells))
    })?;
    let rows = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(file(ctx, ExportDataset::Minutes, MINUTES, rows))
}

// ---------------------------------------------------------------------------------------------
// Events

fn param_text(ctx: &Ctx<'_>, name: &str, p: &EventParam) -> String {
    let join = |prefix: &str, v: &[u16]| {
        v.iter()
            .map(|b| format!("{prefix}{b}"))
            .collect::<Vec<_>>()
            .join("+")
    };
    match p {
        EventParam::Text(_) if FREE_TEXT_PARAMS.contains(&name) => String::new(),
        EventParam::Text(s) => scrub_cell(s),
        EventParam::Number(n) if n.fract() == 0.0 => format_num(*n, 0),
        EventParam::Number(n) => format_num(*n, 1),
        EventParam::Key(k) => ctx
            .labels
            .texts
            .get(k)
            .cloned()
            .unwrap_or_else(|| k.clone()),
        EventParam::Bands(v) => join("B", v),
        EventParam::NrBands(v) => join("n", v),
    }
}

/// The sentence for an i18n key and its parameters, from `CsvLabels::texts`. A key the UI didn't
/// send is written as the key.
fn sentence(ctx: &Ctx<'_>, key: &str, params: &BTreeMap<String, EventParam>) -> String {
    let mut s = ctx
        .labels
        .texts
        .get(key)
        .cloned()
        .unwrap_or_else(|| key.to_string());
    for (name, p) in params {
        s = s.replace(&format!("{{{name}}}"), &param_text(ctx, name, p));
    }
    s
}

fn outcome_name(o: &ChangeOutcome) -> &'static str {
    match o {
        ChangeOutcome::Open => "open",
        ChangeOutcome::Kept => "kept",
        ChangeOutcome::Reverted { .. } => "reverted",
        ChangeOutcome::RevertFailed => "revertFailed",
        ChangeOutcome::Rejected => "rejected",
    }
}

/// A band change in one row: `events.change.<outcome>` with `{from}`, `{to}` and `{reason}`.
fn change_row(ctx: &Ctx<'_>, g: &ChangeGroup) -> (u64, EventCategory, Severity, String, String) {
    let mut params = BTreeMap::new();
    params.insert("from".to_string(), EventParam::Bands(g.from.clone()));
    params.insert("to".to_string(), EventParam::Bands(g.to.clone()));
    if let ChangeOutcome::Reverted { reason, .. } = &g.outcome {
        params.insert("reason".to_string(), EventParam::Key(reason.clone()));
    }
    let severity = match g.outcome {
        ChangeOutcome::Open => Severity::Info,
        ChangeOutcome::Kept => Severity::Good,
        ChangeOutcome::Reverted { .. } | ChangeOutcome::Rejected => Severity::Warning,
        ChangeOutcome::RevertFailed => Severity::Problem,
    };
    let key = format!("events.change.{}", outcome_name(&g.outcome));
    let code = g.steps.last().map(|s| s.kind.clone()).unwrap_or_default();
    (
        g.started_ms,
        EventCategory::BandChange,
        severity,
        sentence(ctx, &key, &params),
        code,
    )
}

fn single_row(ctx: &Ctx<'_>, i: &EventItem) -> (u64, EventCategory, Severity, String, String) {
    (
        i.at_ms,
        i.category,
        i.severity,
        sentence(ctx, &i.key, &i.params),
        i.kind.clone(),
    )
}

fn event_rows(c: &Connection, ctx: &Ctx<'_>, range: &HistoryRange) -> rusqlite::Result<CsvFile> {
    let mut st = c.prepare(
        "SELECT at_ms, kind, detail_json FROM event WHERE at_ms >= ?1 AND at_ms <= ?2
         ORDER BY at_ms, id",
    )?;
    let records = st
        .query_map(params![range.from_ms as i64, range.to_ms as i64], |r| {
            let detail: String = r.get(2)?;
            Ok(EventRecord {
                at_ms: r.get::<_, i64>(0)?.max(0) as u64,
                kind: r.get(1)?,
                detail: serde_json::from_str(&detail).unwrap_or(serde_json::Value::Null),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let rows = event_text::group_changes(&records)
        .iter()
        .filter_map(|e| match e {
            // Internal steps stay out, as on the History page by default.
            EventEntry::Single { item } if item.technical => None,
            EventEntry::Single { item } => Some(single_row(ctx, item)),
            EventEntry::Change { group } => Some(change_row(ctx, group)),
        })
        .map(|(at, cat, sev, sentence, code)| {
            let [lt, ut] = ctx.times(at);
            ctx.row(
                EVENTS,
                vec![
                    lt,
                    ut,
                    ctx.enum_value("category", &cat),
                    ctx.enum_value("severity", &sev),
                    text(sentence),
                    text(code),
                ],
            )
        })
        .collect();
    Ok(file(ctx, ExportDataset::Events, EVENTS, rows))
}

// ---------------------------------------------------------------------------------------------
// Building and saving

/// Builds every file the options ask for, in memory.
pub fn build(
    c: &Connection,
    opts: &ExportOptions,
    labels: &CsvLabels,
    fmt: impl Into<Format>,
) -> rusqlite::Result<Vec<CsvFile>> {
    let ctx = Ctx {
        labels,
        offset_min: opts.range.utc_offset_min,
        cells: opts.include_cell_numbers,
        ip_range: opts.include_ip_range,
        fmt: fmt.into(),
    };
    let range = &opts.range;
    opts.dataset
        .parts()
        .into_iter()
        .map(|d| match d {
            ExportDataset::Hourly => hourly(c, &ctx, range),
            ExportDataset::SpeedTests => speed_tests(c, &ctx, range),
            ExportDataset::Minutes => minute_rows(c, &ctx, range),
            ExportDataset::Events | ExportDataset::All => event_rows(c, &ctx, range),
        })
        .collect()
}

/// The last check: refuses when any file holds something that looks like an identifier.
pub fn privacy_check(files: &[CsvFile]) -> Result<(), String> {
    let mut hits: Vec<String> = files
        .iter()
        .flat_map(|f| identifier_hits(&f.text))
        .collect();
    hits.sort();
    hits.dedup();
    if hits.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Export stopped: the privacy check found {}. Nothing was saved.",
            hits.join(", ")
        ))
    }
}

/// `5g-band-tuner-hourly-2026-11-01.csv`.
pub fn file_name(dataset: ExportDataset, now_ms: u64, offset_min: i32) -> String {
    format!(
        "5g-band-tuner-{}-{}.csv",
        dataset.file_part(),
        local_date(now_ms, offset_min)
    )
}

/// `name.csv`, or `name (2).csv` and so on when it's taken.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    (2..)
        .map(|n| {
            let candidate = if ext.is_empty() {
                format!("{stem} ({n})")
            } else {
                format!("{stem} ({n}).{ext}")
            };
            dir.join(candidate)
        })
        .find(|p| !p.exists())
        .expect("an unused name")
}

/// Writes `<path>.part`, then renames it into place.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, text.as_bytes())?;
    std::fs::rename(&part, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&part);
    })
}

/// Where the user chose to save.
pub enum Target {
    /// One file, at the path from the save dialog.
    File(PathBuf),
    /// All files, in this folder, each under its own name.
    Folder(PathBuf),
}

/// Runs the privacy check, then writes. Returns the file names written.
pub fn save(
    files: &[CsvFile],
    target: &Target,
    now_ms: u64,
    offset_min: i32,
) -> Result<Vec<String>, String> {
    privacy_check(files)?;
    let mut names = Vec::new();
    for f in files {
        let path = match target {
            Target::File(p) => p.clone(),
            Target::Folder(dir) => unique_path(dir, &file_name(f.dataset, now_ms, offset_min)),
        };
        write_atomic(&path, &f.text).map_err(|e| e.to_string())?;
        names.push(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::sampler::{upsert_minute, MinuteRow, Stat};
    use crate::history::schema;
    use crate::history::store::{insert_event_on, insert_speedtest, router_id};
    use crate::speedtest::SpeedTestSource;
    use router_core::event_text::kinds;
    use serde_json::json;

    // 2026-11-01T10:00:00Z
    const T0: u64 = 1_793_491_200_000 + 10 * HOUR_MS;

    fn db_conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        schema::migrate(&mut c).unwrap();
        c
    }

    fn opts(dataset: ExportDataset, cells: bool) -> ExportOptions {
        ExportOptions {
            range: HistoryRange {
                from_ms: T0 - 24 * HOUR_MS,
                to_ms: T0 + 24 * HOUR_MS,
                utc_offset_min: 180,
            },
            dataset,
            include_cell_numbers: cells,
            include_ip_range: false,
        }
    }

    fn stat(v: i32) -> Stat {
        Stat {
            min: Some(v),
            avg: Some(v),
            max: Some(v),
        }
    }

    fn seed(c: &Connection) {
        let id = router_id(c, "192.168.8.1", "mock").unwrap();
        let mut a = MinuteRow {
            minute_ms: T0,
            alive_ms: 60_000,
            observed_ms: 60_000,
            service_ms: 30_000,
            lte_rsrp: stat(-955),
            lte_sinr: stat(-10),
            lte_band: Some(3),
            lte_pci: Some(101),
            lte_earfcn: Some(1300),
            nr_pci: Some(202),
            nr_arfcn: Some(528_980),
            allowed_bands: Some(LteBandSet::from_bands(&[1, 3]).unwrap().to_huawei_hex()),
            operator: Some("Zain KSA".into()),
            ip_range: Some("10.45".into()),
            ..MinuteRow::default()
        };
        upsert_minute(c, id, &a).unwrap();
        // The app's own band change: its lost service isn't the operator's fault.
        a.minute_ms = T0 + 60_000;
        a.flags = FLAG_BAND_CHANGE;
        a.service_ms = 0;
        upsert_minute(c, id, &a).unwrap();
        let r: SpeedTestResult = serde_json::from_value(json!({
            "atMs": T0 + 120_000,
            "provider": "cloudflare",
            "requested": "cloudflare",
            "fallback": null,
            "fallbackDetail": null,
            "size": "light",
            "serving": {"lteBand": 3, "ltePci": 101, "lteEarfcn": 1300, "nrBand": 41},
            "idlePingMs": 40.4,
            "downloadMbps": 116.44,
            "uploadMbps": 10.0,
            "pingDownMs": 300.0,
            "pingUpMs": null,
            "bytesUsed": 25_000_000,
            "server": "Cloudflare DXB 10.1.2.3",
            "sizeApplies": true,
            "ipRange": "10.62",
        }))
        .unwrap();
        insert_speedtest(c, &r, Some(id), SpeedTestSource::Auto).unwrap();
        insert_event_on(
            c,
            &EventRecord {
                at_ms: T0 + 5,
                kind: kinds::APP_STARTED.into(),
                detail: json!({"version": "1.0.0"}),
            },
        )
        .unwrap();
    }

    fn one(c: &Connection, dataset: ExportDataset, cells: bool, labels: &CsvLabels) -> String {
        let files = build(c, &opts(dataset, cells), labels, ',').unwrap();
        assert_eq!(files.len(), 1);
        files.into_iter().next().unwrap().text
    }

    fn lines(text: &str) -> Vec<&str> {
        text.trim_start_matches('\u{feff}')
            .split("\r\n")
            .filter(|l| !l.is_empty())
            .collect()
    }

    #[test]
    fn a_file_has_a_bom_and_crlf() {
        let s = render(
            &["a".into(), "b".into()],
            &[vec![int(Some(1)), text("x")]],
            ',',
        );
        assert_eq!(s, "\u{feff}a,b\r\n1,x\r\n");
        assert!(!s.contains("sep="));
    }

    #[test]
    fn cells_are_quoted_as_rfc_4180_says() {
        assert_eq!(text_field("plain", ','), "plain");
        assert_eq!(text_field("a,b", ','), "\"a,b\"");
        assert_eq!(text_field("say \"hi\"", ','), "\"say \"\"hi\"\"\"");
        assert_eq!(text_field("two\nlines", ','), "\"two\nlines\"");
        // The separator itself is what needs quoting.
        assert_eq!(text_field("a;b", ','), "a;b");
        assert_eq!(text_field("a;b", ';'), "\"a;b\"");
        let s = render(&["h".into()], &[vec![text("x;y")]], ';');
        assert_eq!(lines(&s), vec!["h", "\"x;y\""]);
    }

    #[test]
    fn formulas_are_guarded_and_numbers_stay_numbers() {
        assert_eq!(field(&text("=cmd"), ','), "'=cmd");
        for s in ["+1", "-1", "@SUM(A1)", "\tx", "\rx"] {
            assert!(
                field(&text(s), ',')
                    .trim_start_matches('"')
                    .starts_with('\''),
                "{s:?}"
            );
        }
        assert_eq!(field(&num(Some(-95.5), 1), ','), "-95.5");
        assert_eq!(field(&num(Some(-95.45), 1), ','), "-95.5");
        assert_eq!(field(&num(Some(-0.04), 1), ','), "0.0");
        assert_eq!(field(&num(Some(40.4), 0), ','), "40");
        assert_eq!(field(&int(Some(-3i64)), ','), "-3");
        assert_eq!(field(&num(None, 1), ','), "");
        // A `;` file in a locale with decimal commas writes them; `,` files never do.
        let de = pick_format(';', ",");
        assert_eq!(
            de,
            Format {
                sep: ';',
                decimal: ','
            }
        );
        assert_eq!(field(&num(Some(-95.5), 1), de), "-95,5");
        assert_eq!(field(&int(Some(-3i64)), de), "-3");
        assert_eq!(pick_format(';', "."), Format::from(';'), "ar-SA");
        assert_eq!(pick_format(',', ","), Format::from(','));
        let s = render(&["h".into()], &[vec![num(Some(1.5), 1)]], de);
        assert!(s.ends_with("h\r\n1,5\r\n"), "{s:?}");
    }

    #[test]
    fn digits_are_western_with_a_point() {
        let s = format_num(1234.56, 1);
        assert_eq!(s, "1234.6");
        assert!(s.chars().all(|c| c.is_ascii_digit() || c == '.'));
    }

    #[test]
    fn the_separator_is_comma_or_semicolon() {
        assert_eq!(pick_separator(","), ',');
        assert_eq!(pick_separator(";"), ';');
        assert_eq!(pick_separator("|"), ',');
        assert_eq!(pick_separator(""), ',');
        assert!(matches!(list_separator(), ',' | ';'));
    }

    #[test]
    fn local_and_utc_times() {
        assert_eq!(local_time(T0, 180), "2026-11-01 13:00");
        assert_eq!(utc_time(T0), "2026-11-01T10:00:00Z");
        assert_eq!(local_time(T0, -330), "2026-11-01 04:30");
        assert_eq!(offset_label(180), "UTC+03:00");
        assert_eq!(offset_label(-330), "UTC-05:30");
        assert_eq!(offset_label(0), "UTC+00:00");
        assert_eq!(
            file_name(ExportDataset::Hourly, T0 + 12 * HOUR_MS, 180),
            "5g-band-tuner-hourly-2026-11-02.csv"
        );
    }

    #[test]
    fn hourly_rows_leave_the_apps_own_changes_out_of_no_service() {
        let c = db_conn();
        seed(&c);
        let text = one(&c, ExportDataset::Hourly, false, &CsvLabels::default());
        let l = lines(&text);
        assert!(l[0].starts_with("Local time (UTC+03:00),UTC time,Minutes recorded"));
        assert_eq!(l.len(), 2, "{l:?}");
        let cols: Vec<&str> = l[1].split(',').collect();
        assert_eq!(cols[0], "2026-11-01 13:00");
        assert_eq!(cols[1], "2026-11-01T10:00:00Z");
        assert_eq!(cols[2], "2.0", "recorded");
        assert_eq!(cols[3], "0.5", "in service");
        assert_eq!(
            cols[4], "0.5",
            "without service: the band-change minute isn't counted"
        );
        assert_eq!(cols[5], "1.0", "changing bands");
        assert_eq!(cols[7], "-95.5");
        assert_eq!(cols[14], "B3");
        assert_eq!(cols[16], "B1+B3");
        assert_eq!(cols[18], "1", "one speed test");
        assert_eq!(cols[19], "116.4");
        assert_eq!(cols[22], "Zain KSA");
        assert_eq!(cols.len(), HOURLY.len() - 4);
    }

    #[test]
    fn cell_columns_appear_only_when_asked() {
        let c = db_conn();
        seed(&c);
        for d in [
            ExportDataset::Hourly,
            ExportDataset::SpeedTests,
            ExportDataset::Minutes,
        ] {
            let plain = one(&c, d, false, &CsvLabels::default());
            assert!(!plain.contains("PCI") && !plain.contains("EARFCN"), "{d:?}");
            assert!(!plain.contains("101") && !plain.contains("528980"), "{d:?}");
            let with = one(&c, d, true, &CsvLabels::default());
            let l = lines(&with);
            assert!(
                l[0].ends_with("LTE PCI,LTE EARFCN,5G PCI,5G NR-ARFCN"),
                "{d:?}"
            );
            assert!(l[1].contains(",101,1300,"), "{d:?}: {}", l[1]);
        }
    }

    #[test]
    fn the_ip_range_column_appears_only_when_asked() {
        let c = db_conn();
        seed(&c);
        for (d, range) in [
            (ExportDataset::SpeedTests, "10.62.x.x"),
            (ExportDataset::Minutes, "10.45.x.x"),
        ] {
            let plain = one(&c, d, false, &CsvLabels::default());
            assert!(!plain.contains("IP range"), "{d:?}");
            assert!(!plain.contains(".x.x"), "{d:?}");
            let mut o = opts(d, false);
            o.include_ip_range = true;
            let files = build(&c, &o, &CsvLabels::default(), ',').unwrap();
            let with = &files[0].text;
            let l = lines(with);
            assert!(
                l[0].ends_with(",IP range (first two numbers)"),
                "{d:?}: {}",
                l[0]
            );
            assert!(l[1].ends_with(&format!(",{range}")), "{d:?}: {}", l[1]);
            assert!(privacy_check(&files).is_ok());
            // With the cell numbers too, the range sits before them.
            o.include_cell_numbers = true;
            let both = build(&c, &o, &CsvLabels::default(), ',').unwrap();
            let l = lines(&both[0].text);
            assert!(
                l[0].ends_with(
                    "IP range (first two numbers),LTE PCI,LTE EARFCN,5G PCI,5G NR-ARFCN"
                ),
                "{d:?}: {}",
                l[0]
            );
        }
        // The hourly file has no IP range column.
        let mut o = opts(ExportDataset::Hourly, false);
        o.include_ip_range = true;
        let hourly = build(&c, &o, &CsvLabels::default(), ',').unwrap();
        assert!(!hourly[0].text.contains("IP range"));
    }

    #[test]
    fn options_from_an_older_ui_leave_the_ip_range_out() {
        let o: ExportOptions = serde_json::from_value(json!({
            "range": {"fromMs": 0, "toMs": 1, "utcOffsetMin": 0},
            "dataset": "speedTests",
            "includeCellNumbers": false,
        }))
        .unwrap();
        assert!(!o.include_ip_range);
    }

    #[test]
    fn a_server_address_is_scrubbed() {
        assert_eq!(scrub_cell("10.1.2.3"), "");
        assert_eq!(scrub_cell("Cloudflare DXB 10.1.2.3"), "Cloudflare DXB");
        assert_eq!(scrub_cell("Cloudflare DXB"), "Cloudflare DXB");
        let c = db_conn();
        seed(&c);
        let text = one(&c, ExportDataset::SpeedTests, false, &CsvLabels::default());
        assert!(text.contains(",Cloudflare DXB,"));
        assert!(!text.contains("10.1.2.3"));
        assert!(text.contains(",Automatic test,"));
        assert!(text.contains(",25.0,"), "MB");
    }

    #[test]
    fn events_are_plain_sentences_with_a_code() {
        let c = db_conn();
        seed(&c);
        let mut labels = CsvLabels::default();
        labels.texts.insert(
            "events.kind.appStarted".into(),
            "The app started ({version})".into(),
        );
        let text = one(&c, ExportDataset::Events, false, &labels);
        let l = lines(&text);
        assert_eq!(
            l[0],
            "Local time (UTC+03:00),UTC time,Category,Severity,Event,Code"
        );
        assert_eq!(
            l[1],
            "2026-11-01 13:00,2026-11-01T10:00:00Z,App,Info,The app started (1.0.0),app.started"
        );
    }

    #[test]
    fn a_leak_in_a_label_blocks_the_export_and_writes_nothing() {
        let c = db_conn();
        seed(&c);
        let mut labels = CsvLabels::default();
        labels
            .headers
            .insert("server".into(), "Server 192.168.1.20".into());
        let files = build(&c, &opts(ExportDataset::All, false), &labels, ',').unwrap();
        let dir = tempfile::tempdir().unwrap();
        let err = save(&files, &Target::Folder(dir.path().into()), T0, 180).unwrap_err();
        assert_eq!(
            err,
            "Export stopped: the privacy check found an IPv4 address. Nothing was saved."
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn all_four_go_to_a_folder_and_a_clash_gets_a_number() {
        let c = db_conn();
        seed(&c);
        let files = build(
            &c,
            &opts(ExportDataset::All, false),
            &CsvLabels::default(),
            ',',
        )
        .unwrap();
        assert_eq!(files.len(), 4);
        let dir = tempfile::tempdir().unwrap();
        let first = save(&files, &Target::Folder(dir.path().into()), T0, 180).unwrap();
        assert_eq!(
            first,
            vec![
                "5g-band-tuner-hourly-2026-11-01.csv",
                "5g-band-tuner-speed-tests-2026-11-01.csv",
                "5g-band-tuner-minutes-2026-11-01.csv",
                "5g-band-tuner-events-2026-11-01.csv",
            ]
        );
        let again = save(&files[..1], &Target::Folder(dir.path().into()), T0, 180).unwrap();
        assert_eq!(again, vec!["5g-band-tuner-hourly-2026-11-01 (2).csv"]);
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn arabic_labels_round_trip() {
        let c = db_conn();
        seed(&c);
        let mut labels = CsvLabels::default();
        labels
            .headers
            .insert("localTime".into(), "الوقت المحلي ({offset})".into());
        labels
            .values
            .insert("source.auto".into(), "اختبار تلقائي".into());
        let text = one(&c, ExportDataset::SpeedTests, false, &labels);
        assert!(text.starts_with("\u{feff}الوقت المحلي (UTC+03:00),"));
        assert!(text.contains(",اختبار تلقائي,"));
        let path = tempfile::tempdir().unwrap();
        let p = path.path().join("a.csv");
        write_atomic(&p, &text).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), text);
    }

    /// The UI's test checks that both languages have a header for every column id and a text
    /// for every value id in this file.
    #[test]
    fn the_column_ids_are_written_for_the_ui() {
        let mut headers: Vec<&str> = [HOURLY, SPEED_TESTS, MINUTES, EVENTS]
            .iter()
            .flat_map(|cols| cols.iter().map(|c| c.id))
            .collect();
        headers.sort_unstable();
        headers.dedup();
        let values: Vec<&str> = VALUES.iter().map(|(k, _)| *k).collect();
        let json = serde_json::to_string_pretty(&json!({
            "headers": headers,
            "values": values,
        }))
        .unwrap()
            + "\n";
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/src/lib/bindings/csvColumns.json");
        if std::fs::read_to_string(&path).ok().as_deref() != Some(json.as_str()) {
            std::fs::write(&path, json).unwrap();
        }
    }
}
