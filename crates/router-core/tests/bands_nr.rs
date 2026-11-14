use router_core::bands::nr::{nr_arfcn_to_mhz, resolve_nr_band};
use router_core::bands::quality::{classify, Metric, Quality};
use router_core::bands::Duplex;

fn check(dl: u32, ul: Option<u32>, primary: u16, alts: &[u16]) -> router_core::NrBandInfo {
    let info = resolve_nr_band(dl, ul).expect("band resolved");
    assert_eq!(info.primary, primary, "primary for {dl}/{ul:?}");
    assert_eq!(info.alternates, alts, "alternates for {dl}/{ul:?}");
    info
}

#[test]
fn resolves_known_arfcns() {
    let i = check(627264, Some(627264), 78, &[77]);
    assert!((i.dl_mhz - 3408.96).abs() < 1e-6);
    assert_eq!(i.duplex, Duplex::Tdd);
    check(620000, Some(620000), 78, &[77]);
    check(504990, Some(504990), 41, &[90]);
    let i = check(528980, Some(528980), 41, &[90]);
    assert!((i.dl_mhz - 2644.9).abs() < 1e-6);
    check(519000, Some(519000), 38, &[41, 90]);
    let i = check(531000, Some(507000), 7, &[]);
    assert_eq!(i.duplex, Duplex::Fdd);
    check(428000, Some(390000), 1, &[65]);
    check(154600, Some(143600), 28, &[]);
    check(2079167, Some(2079167), 261, &[257]);
    assert_eq!(resolve_nr_band(627264, None).unwrap().primary, 78);
    assert_eq!(resolve_nr_band(627264, Some(0)).unwrap().primary, 78);
}

#[test]
fn arfcn_range_limits() {
    assert_eq!(resolve_nr_band(3_300_000, None), None);
    assert_eq!(nr_arfcn_to_mhz(3_279_166), None);
    assert!(nr_arfcn_to_mhz(3_279_165).is_some());
    assert_eq!(nr_arfcn_to_mhz(0), Some(0.0));
    assert_eq!(nr_arfcn_to_mhz(600000), Some(3000.0));
    assert!((nr_arfcn_to_mhz(2016667).unwrap() - 24250.08).abs() < 1e-9);
}

#[test]
fn sdl_bands_only_match_without_ul() {
    // 1500 MHz on the 5 kHz raster: n74 (43 MHz wide) wins, SDL n75 is an alternate.
    let i = resolve_nr_band(300_000, None).unwrap();
    assert_eq!((i.primary, i.alternates), (74, vec![50, 75, 92, 94]));
    // With a UL ARFCN, SDL n75 and TDD n50 must drop out.
    let i = resolve_nr_band(300_000, Some(290_000)).unwrap();
    assert_eq!(i.duplex, Duplex::Fdd);
    assert!(!i.alternates.contains(&75) && !i.alternates.contains(&50));
    // n75-only region: 1432-1474 MHz is covered by n50/n75/n92/n94 (no n74).
    let i = resolve_nr_band(288_000, None).unwrap(); // 1440 MHz
    assert_eq!((i.primary, i.alternates), (50, vec![75, 92, 94]));
}

#[test]
fn classifies() {
    assert_eq!(classify(Metric::Rsrp, -80.0), Quality::Excellent);
    assert_eq!(classify(Metric::Rsrp, -100.0), Quality::Fair);
    assert_eq!(classify(Metric::Rsrp, -100.1), Quality::Poor);
    assert_eq!(classify(Metric::Sinr, 13.0), Quality::Good);
    assert_eq!(classify(Metric::Sinr, -0.5), Quality::Poor);
    assert_eq!(classify(Metric::Rsrq, -15.0), Quality::Good);
    assert_eq!(classify(Metric::Sinr, 20.0), Quality::Excellent);
    assert_eq!(classify(Metric::Rsrq, -20.0), Quality::Fair);
}
