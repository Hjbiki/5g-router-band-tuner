//! The test plan, signal statistics and the final-config choice. No I/O.

pub use router_core::scan::{Sample, Stat};
use router_core::{LteBandSet, NetConfig};
use serde::Serialize;

/// One LTE band set to try, with the rest of the config kept as the router has it.
#[derive(Debug, Clone)]
pub struct Target {
    pub name: &'static str,
    pub bands: &'static [u8],
}

/// The plan from the 2026-10-29 request, in order.
pub const PLAN: &[Target] = &[
    Target {
        name: "B1+B3",
        bands: &[1, 3],
    },
    Target {
        name: "B3 only",
        bands: &[3],
    },
    Target {
        name: "B1+B3+B28",
        bands: &[1, 3, 28],
    },
    Target {
        name: "B1+B3+B20+B28 (8080005)",
        bands: &[1, 3, 20, 28],
    },
];

/// The config to fall back to when no target qualifies: the user's everyday set.
pub const DEFAULT_BANDS: &[u8] = &[1, 3, 20, 28];

/// The preferred serving cell: LTE B3, with NR n41.
pub const PREFERRED_LTE_BAND: u16 = 3;
pub const PREFERRED_NR_BAND: u16 = 41;

/// `base` with its LTE bands replaced.
pub fn with_bands(base: &NetConfig, bands: &[u8]) -> NetConfig {
    NetConfig {
        lte_bands: LteBandSet::from_bands(bands).expect("plan bands are valid"),
        ..base.clone()
    }
}

/// What a step measured over its window.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measurement {
    pub samples: usize,
    pub lte_sinr: Stat,
    pub lte_rsrp: Stat,
    pub nr_sinr: Stat,
    pub nr_rsrp: Stat,
    /// Share of samples with an NR leg.
    pub nr_present_ratio: f32,
    pub in_service_ratio: f32,
    /// Serving cell and NR band of the last sample.
    pub end_lte: Option<(u16, Option<u16>)>,
    pub end_nr_band: Option<u16>,
    /// Distinct (band, PCI) LTE cells seen, in order.
    pub cells_seen: Vec<(Option<u16>, Option<u16>)>,
}

impl Measurement {
    pub fn of(samples: &[Sample]) -> Self {
        let n = samples.len().max(1) as f32;
        let mut cells_seen = Vec::new();
        for s in samples {
            let c = (s.lte_band, s.lte_pci);
            if cells_seen.last() != Some(&c) {
                cells_seen.push(c);
            }
        }
        let last = samples.last();
        Self {
            samples: samples.len(),
            lte_sinr: Stat::of(samples.iter().map(|s| s.lte_sinr)),
            lte_rsrp: Stat::of(samples.iter().map(|s| s.lte_rsrp)),
            nr_sinr: Stat::of(samples.iter().map(|s| s.nr_sinr)),
            nr_rsrp: Stat::of(samples.iter().map(|s| s.nr_rsrp)),
            nr_present_ratio: samples.iter().filter(|s| s.nr_present).count() as f32 / n,
            in_service_ratio: samples.iter().filter(|s| s.in_service).count() as f32 / n,
            end_lte: last.and_then(|s| s.lte_band.map(|b| (b, s.lte_pci))),
            end_nr_band: last.and_then(|s| s.nr_band),
            cells_seen,
        }
    }

    /// Ended on B3 with n41, and had NR for most of the window.
    pub fn preferred(&self) -> bool {
        self.end_lte
            .is_some_and(|(band, _)| band == PREFERRED_LTE_BAND)
            && self.end_nr_band == Some(PREFERRED_NR_BAND)
            && self.nr_present_ratio >= 0.5
    }
}

/// Index of the best preferred measurement: highest mean NR SINR, then mean LTE SINR. Only
/// steps that were kept (`kept[i]`) count.
pub fn pick_final(results: &[(bool, Option<&Measurement>)]) -> Option<usize> {
    let key = |m: &Measurement| {
        (
            m.nr_sinr.avg.unwrap_or(f32::NEG_INFINITY),
            m.lte_sinr.avg.unwrap_or(f32::NEG_INFINITY),
        )
    };
    results
        .iter()
        .enumerate()
        .filter_map(|(i, (kept, m))| match m {
            Some(m) if *kept && m.preferred() => Some((i, key(m))),
            _ => None,
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(band: u16, pci: u16, sinr: f32, nr: Option<(u16, f32)>) -> Sample {
        Sample {
            lte_band: Some(band),
            lte_pci: Some(pci),
            lte_sinr: Some(sinr),
            lte_rsrp: Some(-95.0),
            nr_present: nr.is_some(),
            nr_band: nr.map(|n| n.0),
            nr_sinr: nr.map(|n| n.1),
            nr_rsrp: nr.map(|_| -100.0),
            in_service: true,
        }
    }

    #[test]
    fn plan_masks_match_the_request() {
        let mask = |b: &[u8]| serde_json::to_value(LteBandSet::from_bands(b).unwrap()).unwrap();
        assert_eq!(mask(PLAN[0].bands), "5");
        assert_eq!(mask(PLAN[1].bands), "4");
        assert_eq!(mask(PLAN[2].bands), "8000005");
        assert_eq!(mask(PLAN[3].bands), "8080005");
        assert_eq!(mask(DEFAULT_BANDS), "8080005");
    }

    #[test]
    fn stats_skip_missing_values() {
        let st = Stat::of([Some(1.0), None, Some(3.0)].into_iter());
        assert_eq!(st.avg, Some(2.0));
        assert_eq!((st.min, st.max, st.n), (Some(1.0), Some(3.0), 2));
        assert_eq!(Stat::of([None].into_iter()).avg, None);
    }

    #[test]
    fn measurement_tracks_cells_and_end_state() {
        let m = Measurement::of(&[
            s(20, 120, 5.0, None),
            s(3, 103, 10.0, Some((41, 12.0))),
            s(3, 103, 12.0, Some((41, 14.0))),
        ]);
        assert_eq!(
            m.cells_seen,
            vec![(Some(20), Some(120)), (Some(3), Some(103))]
        );
        assert_eq!(m.end_lte, Some((3, Some(103))));
        assert_eq!(m.end_nr_band, Some(41));
        assert_eq!(m.lte_sinr.avg, Some(9.0));
        assert!(m.preferred());
    }

    #[test]
    fn final_choice_prefers_b3_with_n41_and_best_nr_sinr() {
        let good_low = Measurement::of(&[s(3, 103, 15.0, Some((41, 5.0)))]);
        let good_high = Measurement::of(&[s(3, 103, 8.0, Some((41, 9.0)))]);
        let other_cell = Measurement::of(&[s(20, 120, 20.0, Some((41, 20.0)))]);
        let no_nr = Measurement::of(&[s(3, 103, 20.0, None)]);
        let r = [
            (true, Some(&good_low)),
            (true, Some(&other_cell)),
            (true, Some(&good_high)),
            (true, Some(&no_nr)),
        ];
        assert_eq!(pick_final(&r), Some(2));
        // A step that was reverted doesn't count, however good it looked.
        let r = [(false, Some(&good_high)), (true, Some(&other_cell))];
        assert_eq!(pick_final(&r), None);
    }
}
