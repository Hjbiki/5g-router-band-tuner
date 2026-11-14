//! What past band changes say about single bands, for the tints on the Bands page. Read from the
//! local journal archive and event log; nothing here leaves the PC or touches the router.
//!
//! Only a lock on one band alone counts: when a set of bands loses service, nothing says which
//! band was at fault.

use std::path::Path;

use router_core::event_text::kinds;
use router_core::{JournalPhase, JournalRecord};
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::commands::{AppState, CmdError};

/// A reverted change's matching `safe_apply.reverted` event lies this close to its archived time.
const MATCH_WINDOW_MS: u64 = 10_000;
/// Reverts that mean the band gave no service. Asked-for or refused changes say nothing about it.
const NO_SERVICE_REASONS: [&str; 3] = ["noReattach", "lossOfService", "resumedWithoutService"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BandOutcomeKind {
    NoService,
    Kept,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BandOutcome {
    #[ts(type = "number")]
    pub at_ms: u64,
    pub band: u8,
    pub kind: BandOutcomeKind,
}

fn no_service_reverts(data_dir: &Path) -> Vec<u64> {
    let text = std::fs::read_to_string(data_dir.join("events.jsonl")).unwrap_or_default();
    text.lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == kinds::SAFE_APPLY_REVERTED)
        .filter(|v| {
            v["detail"]["reason"]
                .as_str()
                .is_some_and(|r| NO_SERVICE_REASONS.contains(&r))
        })
        .filter_map(|v| v["at_ms"].as_u64())
        .collect()
}

/// Oldest first is not promised; the UI sorts. Records of another router are skipped.
pub fn load_outcomes(data_dir: &Path, host: &str) -> Vec<BandOutcome> {
    let reverts = no_service_reverts(data_dir);
    let Ok(dir) = std::fs::read_dir(data_dir.join("journal-archive")) else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<JournalRecord>(&t).ok())
        .filter(|r| r.host == host)
        .filter_map(|r| {
            let bands = r.requested.lte_bands.bands();
            let [band] = bands[..] else { return None };
            let at_ms = r.phase_changed_at_ms;
            let kind = match r.phase {
                JournalPhase::Committed => BandOutcomeKind::Kept,
                JournalPhase::Reverted
                    if reverts.iter().any(|t| t.abs_diff(at_ms) <= MATCH_WINDOW_MS) =>
                {
                    BandOutcomeKind::NoService
                }
                _ => return None,
            };
            Some(BandOutcome { at_ms, band, kind })
        })
        .collect()
}

#[tauri::command]
pub async fn band_outcomes(state: State<'_, AppState>) -> Result<Vec<BandOutcome>, CmdError> {
    let dir = state.data_dir().to_path_buf();
    let host = state.host();
    tokio::task::spawn_blocking(move || load_outcomes(&dir, &host))
        .await
        .map_err(|e| CmdError::Message(e.to_string()))
}

/// When auto-scans ran, as `[from, to]` in wall-clock ms, from the actor's `scan.active` events. A
/// scan still open at the end of the log runs to `u64::MAX`, sent as a number the UI can compare.
pub fn load_scan_periods(data_dir: &Path) -> Vec<(u64, u64)> {
    let text = std::fs::read_to_string(data_dir.join("events.jsonl")).unwrap_or_default();
    let mut periods = Vec::new();
    let mut open = None;
    for v in text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["kind"] == kinds::SCAN_ACTIVE)
    {
        let Some(at) = v["at_ms"].as_u64() else {
            continue;
        };
        if v["detail"]["active"] == true {
            open.get_or_insert(at);
        } else if let Some(from) = open.take() {
            periods.push((from, at));
        }
    }
    if let Some(from) = open {
        periods.push((from, u64::MAX));
    }
    periods
}

#[tauri::command]
pub async fn scan_periods(state: State<'_, AppState>) -> Result<Vec<(f64, f64)>, CmdError> {
    let dir = state.data_dir().to_path_buf();
    let periods = tokio::task::spawn_blocking(move || load_scan_periods(&dir))
        .await
        .map_err(|e| CmdError::Message(e.to_string()))?;
    Ok(periods
        .into_iter()
        .map(|(from, to)| (from as f64, to as f64))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::{LteBandSet, NetConfig, NetworkMode};

    fn archive(dir: &Path, name: &str, host: &str, bands: &[u8], phase: JournalPhase, at_ms: u64) {
        let cfg = NetConfig {
            network_mode: NetworkMode::Auto,
            lte_bands: LteBandSet::from_bands(bands).unwrap(),
            legacy_band_raw: "0".into(),
            nr_option: None,
        };
        let r = JournalRecord {
            version: 1,
            id: name.into(),
            driver: "mock".into(),
            host: host.into(),
            previous: cfg.clone(),
            requested: cfg,
            phase,
            phase_changed_at_ms: at_ms,
            created_at_ms: at_ms,
        };
        let d = dir.join("journal-archive");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join(format!("{name}.json")),
            serde_json::to_string(&r).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn only_lone_bands_and_real_service_losses_count() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::write(
            d.join("events.jsonl"),
            concat!(
                "{\"at_ms\":100000,\"kind\":\"safe_apply.reverted\",\"detail\":{\"reason\":\"noReattach\"}}\n",
                "{\"at_ms\":500000,\"kind\":\"safe_apply.reverted\",\"detail\":{\"reason\":\"userRequested\"}}\n",
                "not json\n",
            ),
        )
        .unwrap();
        archive(d, "lost", "h", &[43], JournalPhase::Reverted, 101_000);
        archive(d, "kept", "h", &[3], JournalPhase::Committed, 200_000);
        archive(d, "set", "h", &[1, 3], JournalPhase::Reverted, 100_500);
        archive(d, "user", "h", &[7], JournalPhase::Reverted, 500_000);
        archive(
            d,
            "other",
            "elsewhere",
            &[8],
            JournalPhase::Committed,
            300_000,
        );

        let mut got = load_outcomes(d, "h");
        got.sort_by_key(|o| o.band);
        assert_eq!(
            got,
            vec![
                BandOutcome {
                    at_ms: 200_000,
                    band: 3,
                    kind: BandOutcomeKind::Kept
                },
                BandOutcome {
                    at_ms: 101_000,
                    band: 43,
                    kind: BandOutcomeKind::NoService
                },
            ]
        );
    }

    #[test]
    fn scan_periods_pair_up_and_a_scan_still_open_runs_on() {
        let t = tempfile::tempdir().unwrap();
        let line = |at: u64, on: bool| {
            format!(r#"{{"at_ms":{at},"kind":"scan.active","detail":{{"active":{on}}}}}"#)
        };
        let log = [
            line(10, true),
            line(20, false),
            r#"{"at_ms":25,"kind":"other","detail":{}}"#.to_string(),
            line(30, true),
            line(31, true),
            line(40, false),
            line(50, true),
        ]
        .join(
            "
",
        );
        std::fs::write(t.path().join("events.jsonl"), log).unwrap();
        assert_eq!(
            load_scan_periods(t.path()),
            vec![(10, 20), (30, 40), (50, u64::MAX)]
        );
    }

    #[test]
    fn no_files_means_no_history() {
        let t = tempfile::tempdir().unwrap();
        assert!(load_outcomes(t.path(), "h").is_empty());
    }
}
