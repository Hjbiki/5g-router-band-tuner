use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::Duplex;

/// Resolved NR band for a DL (and optional UL) NR-ARFCN.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NrBandInfo {
    pub primary: u16,
    pub alternates: Vec<u16>,
    pub duplex: Duplex,
    pub dl_mhz: f64,
}

/// Highest NR-ARFCN of the global raster (TS 38.104 section 5.4.2.1).
const MAX_ARFCN: u32 = 3_279_165;

/// NR-ARFCN to frequency in kHz, integer arithmetic.
fn arfcn_to_khz(arfcn: u32) -> Option<u64> {
    let n = u64::from(arfcn);
    match arfcn {
        0..=599_999 => Some(5 * n),
        600_000..=2_016_666 => Some(3_000_000 + 15 * (n - 600_000)),
        2_016_667..=MAX_ARFCN => Some(24_250_080 + 60 * (n - 2_016_667)),
        _ => None,
    }
}

/// NR-ARFCN to frequency in MHz; `None` above 3_279_165.
pub fn nr_arfcn_to_mhz(arfcn: u32) -> Option<f64> {
    arfcn_to_khz(arfcn).map(|k| k as f64 / 1000.0)
}

struct Band {
    n: u16,
    duplex: Duplex,
    /// UL range in MHz; `None` for SDL.
    ul: Option<(f64, f64)>,
    dl: (f64, f64),
}

const fn fdd(n: u16, ul: (f64, f64), dl: (f64, f64)) -> Band {
    Band {
        n,
        duplex: Duplex::Fdd,
        ul: Some(ul),
        dl,
    }
}
const fn tdd(n: u16, r: (f64, f64)) -> Band {
    Band {
        n,
        duplex: Duplex::Tdd,
        ul: Some(r),
        dl: r,
    }
}
const fn sdl(n: u16, dl: (f64, f64)) -> Band {
    Band {
        n,
        duplex: Duplex::SdlOnly,
        ul: None,
        dl,
    }
}

/// TS 38.104 Table 5.2-1 (FR1) and Table 5.2-2 (FR2 n257-n261), in MHz.
/// Supplementary-uplink-only bands (n80-n84, n86, n89, n95, n97-n99) have no DL and are omitted.
static BANDS: &[Band] = &[
    fdd(1, (1920.0, 1980.0), (2110.0, 2170.0)),
    fdd(2, (1850.0, 1910.0), (1930.0, 1990.0)),
    fdd(3, (1710.0, 1785.0), (1805.0, 1880.0)),
    fdd(5, (824.0, 849.0), (869.0, 894.0)),
    fdd(7, (2500.0, 2570.0), (2620.0, 2690.0)),
    fdd(8, (880.0, 915.0), (925.0, 960.0)),
    fdd(12, (699.0, 716.0), (729.0, 746.0)),
    fdd(13, (777.0, 787.0), (746.0, 756.0)),
    fdd(14, (788.0, 798.0), (758.0, 768.0)),
    fdd(18, (815.0, 830.0), (860.0, 875.0)),
    fdd(20, (832.0, 862.0), (791.0, 821.0)),
    fdd(24, (1626.5, 1660.5), (1525.0, 1559.0)),
    fdd(25, (1850.0, 1915.0), (1930.0, 1995.0)),
    fdd(26, (814.0, 849.0), (859.0, 894.0)),
    fdd(28, (703.0, 748.0), (758.0, 803.0)),
    sdl(29, (717.0, 728.0)),
    fdd(30, (2305.0, 2315.0), (2350.0, 2360.0)),
    tdd(34, (2010.0, 2025.0)),
    tdd(38, (2570.0, 2620.0)),
    tdd(39, (1880.0, 1920.0)),
    tdd(40, (2300.0, 2400.0)),
    tdd(41, (2496.0, 2690.0)),
    tdd(46, (5150.0, 5925.0)),
    tdd(47, (5855.0, 5925.0)),
    tdd(48, (3550.0, 3700.0)),
    tdd(50, (1432.0, 1517.0)),
    tdd(51, (1427.0, 1432.0)),
    tdd(53, (2483.5, 2495.0)),
    tdd(54, (1670.0, 1675.0)),
    fdd(65, (1920.0, 2010.0), (2110.0, 2200.0)),
    fdd(66, (1710.0, 1780.0), (2110.0, 2200.0)),
    sdl(67, (738.0, 758.0)),
    fdd(70, (1695.0, 1710.0), (1995.0, 2020.0)),
    fdd(71, (663.0, 698.0), (617.0, 652.0)),
    fdd(72, (451.0, 456.0), (461.0, 466.0)),
    fdd(74, (1427.0, 1470.0), (1475.0, 1518.0)),
    sdl(75, (1432.0, 1517.0)),
    sdl(76, (1427.0, 1432.0)),
    tdd(77, (3300.0, 4200.0)),
    tdd(78, (3300.0, 3800.0)),
    tdd(79, (4400.0, 5000.0)),
    tdd(90, (2496.0, 2690.0)),
    fdd(91, (832.0, 862.0), (1427.0, 1432.0)),
    fdd(92, (832.0, 862.0), (1432.0, 1517.0)),
    fdd(93, (880.0, 915.0), (1427.0, 1432.0)),
    fdd(94, (880.0, 915.0), (1432.0, 1517.0)),
    tdd(96, (5925.0, 7125.0)),
    fdd(100, (874.4, 880.0), (919.4, 925.0)),
    tdd(101, (1900.0, 1910.0)),
    tdd(102, (5925.0, 6425.0)),
    tdd(104, (6425.0, 7125.0)),
    tdd(257, (26500.0, 29500.0)),
    tdd(258, (24250.0, 27500.0)),
    tdd(259, (39500.0, 43500.0)),
    tdd(260, (37000.0, 40000.0)),
    tdd(261, (27500.0, 28350.0)),
];

fn khz(mhz: f64) -> u64 {
    (mhz * 1000.0).round() as u64
}

fn contains(range: (f64, f64), f_khz: u64) -> bool {
    (khz(range.0)..=khz(range.1)).contains(&f_khz)
}

/// Resolve the NR band for a DL ARFCN and optional UL ARFCN (`None` or 0 = unknown).
pub fn resolve_nr_band(dl: u32, ul: Option<u32>) -> Option<NrBandInfo> {
    let dl_khz = arfcn_to_khz(dl)?;
    let ul = ul.filter(|&u| u != 0);
    let ul_khz = match ul {
        Some(u) => Some(arfcn_to_khz(u)?),
        None => None,
    };

    let mut hits: Vec<&Band> = BANDS
        .iter()
        .filter(|b| contains(b.dl, dl_khz))
        .filter(|b| match (ul, ul_khz) {
            (None, _) => true,
            (Some(u), _) if u == dl => b.duplex == Duplex::Tdd,
            (Some(_), Some(uk)) => b.duplex == Duplex::Fdd && b.ul.is_some_and(|r| contains(r, uk)),
            _ => false,
        })
        .collect();
    hits.sort_by(|a, b| {
        let wa = khz(a.dl.1) - khz(a.dl.0);
        let wb = khz(b.dl.1) - khz(b.dl.0);
        wa.cmp(&wb).then(a.n.cmp(&b.n))
    });
    let (first, rest) = hits.split_first()?;
    let mut alternates: Vec<u16> = rest.iter().map(|b| b.n).collect();
    alternates.sort_unstable();
    Some(NrBandInfo {
        primary: first.n,
        alternates,
        duplex: first.duplex,
        dl_mhz: dl_khz as f64 / 1000.0,
    })
}
