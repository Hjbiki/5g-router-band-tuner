use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

use super::Duplex;
use crate::RouterError;

/// Set of LTE bands as a bitmask (bit n-1 = band n). Serialized as the Huawei hex string.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, TS)]
#[ts(export, type = "string")]
pub struct LteBandSet(u128);

impl LteBandSet {
    /// Firmware "all bands" value.
    pub const AUTO: LteBandSet = LteBandSet(0x7FFF_FFFF_FFFF_FFFF);

    pub const fn from_bits(bits: u128) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u128 {
        self.0
    }

    /// Builds a set from band numbers 1..=128.
    pub fn from_bands(bands: &[u8]) -> Result<Self, RouterError> {
        let mut bits = 0u128;
        for &band in bands {
            if !(1..=128).contains(&band) {
                return Err(RouterError::Validation {
                    message: format!("LTE band {band} is outside 1..=128"),
                });
            }
            bits |= 1u128 << (band - 1);
        }
        Ok(Self(bits))
    }

    /// Parses 1..=32 hex digits (any case, surrounding whitespace allowed).
    pub fn from_huawei_hex(s: &str) -> Result<Self, RouterError> {
        let t = s.trim();
        if t.is_empty() || t.len() > 32 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(RouterError::Parse {
                message: "LTE band mask must be 1-32 hex digits".to_string(),
            });
        }
        u128::from_str_radix(t, 16)
            .map(Self)
            .map_err(|e| RouterError::Parse {
                message: e.to_string(),
            })
    }

    /// Uppercase hex, no prefix, no leading zeros; "0" when empty.
    pub fn to_huawei_hex(&self) -> String {
        format!("{:X}", self.0)
    }

    pub fn bands(&self) -> Vec<u8> {
        (1..=128u8).filter(|&b| self.contains(b)).collect()
    }

    pub fn contains(&self, band: u8) -> bool {
        (1..=128).contains(&band) && self.0 & (1u128 << (band - 1)) != 0
    }

    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn is_subset_of(&self, other: &LteBandSet) -> bool {
        self.0 & !other.0 == 0
    }

    pub fn union(&self, other: &Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn intersection(&self, other: &Self) -> Self {
        Self(self.0 & other.0)
    }

    /// "B1+B3+B28", "Auto" for [`LteBandSet::AUTO`], "None" when empty.
    pub fn label(&self) -> String {
        if *self == Self::AUTO {
            return "Auto".to_string();
        }
        if self.is_empty() {
            return "None".to_string();
        }
        self.bands()
            .iter()
            .map(|b| format!("B{b}"))
            .collect::<Vec<_>>()
            .join("+")
    }
}

impl Serialize for LteBandSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_huawei_hex())
    }
}

impl<'de> Deserialize<'de> for LteBandSet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::from_huawei_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Static description of one LTE band from TS 36.101 Table 5.5-1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LteBandMeta {
    pub band: u8,
    pub duplex: Duplex,
    /// Downlink frequency range in MHz.
    pub dl_mhz: (f32, f32),
    /// Commonly quoted band label in MHz (e.g. 1800 for B3).
    pub label_mhz: u16,
}

use Duplex::{Fdd, SdlOnly, Tdd};

const LTE_BANDS: &[(u8, Duplex, (f32, f32), u16)] = &[
    (1, Fdd, (2110.0, 2170.0), 2100),
    (2, Fdd, (1930.0, 1990.0), 1900),
    (3, Fdd, (1805.0, 1880.0), 1800),
    (4, Fdd, (2110.0, 2155.0), 1700),
    (5, Fdd, (869.0, 894.0), 850),
    (6, Fdd, (875.0, 885.0), 800),
    (7, Fdd, (2620.0, 2690.0), 2600),
    (8, Fdd, (925.0, 960.0), 900),
    (9, Fdd, (1844.9, 1879.9), 1800),
    (10, Fdd, (2110.0, 2170.0), 1700),
    (11, Fdd, (1475.9, 1495.9), 1500),
    (12, Fdd, (729.0, 746.0), 700),
    (13, Fdd, (746.0, 756.0), 700),
    (14, Fdd, (758.0, 768.0), 700),
    (17, Fdd, (734.0, 746.0), 700),
    (18, Fdd, (860.0, 875.0), 800),
    (19, Fdd, (875.0, 890.0), 800),
    (20, Fdd, (791.0, 821.0), 800),
    (21, Fdd, (1495.9, 1510.9), 1500),
    (22, Fdd, (3510.0, 3590.0), 3500),
    (23, Fdd, (2180.0, 2200.0), 2000),
    (24, Fdd, (1525.0, 1559.0), 1500),
    (25, Fdd, (1930.0, 1995.0), 1900),
    (26, Fdd, (859.0, 894.0), 850),
    (27, Fdd, (852.0, 869.0), 850),
    (28, Fdd, (758.0, 803.0), 700),
    (29, SdlOnly, (717.0, 728.0), 700),
    (30, Fdd, (2350.0, 2360.0), 2300),
    (31, Fdd, (462.5, 467.5), 450),
    (32, SdlOnly, (1452.0, 1496.0), 1500),
    (33, Tdd, (1900.0, 1920.0), 1900),
    (34, Tdd, (2010.0, 2025.0), 2000),
    (35, Tdd, (1850.0, 1910.0), 1900),
    (36, Tdd, (1930.0, 1990.0), 1900),
    (37, Tdd, (1910.0, 1930.0), 1900),
    (38, Tdd, (2570.0, 2620.0), 2600),
    (39, Tdd, (1880.0, 1920.0), 1900),
    (40, Tdd, (2300.0, 2400.0), 2300),
    (41, Tdd, (2496.0, 2690.0), 2500),
    (42, Tdd, (3400.0, 3600.0), 3500),
    (43, Tdd, (3600.0, 3800.0), 3600),
    (44, Tdd, (703.0, 803.0), 700),
    (45, Tdd, (1447.0, 1467.0), 1400),
    (46, Tdd, (5150.0, 5925.0), 5200),
    (47, Tdd, (5855.0, 5925.0), 5900),
    (48, Tdd, (3550.0, 3700.0), 3500),
    (49, Tdd, (3550.0, 3700.0), 3500),
    (50, Tdd, (1432.0, 1517.0), 1500),
    (51, Tdd, (1427.0, 1432.0), 1400),
    (52, Tdd, (3300.0, 3400.0), 3300),
    (53, Tdd, (2483.5, 2495.0), 2500),
    (65, Fdd, (2110.0, 2200.0), 2100),
    (66, Fdd, (2110.0, 2200.0), 2100),
    (67, SdlOnly, (738.0, 758.0), 700),
    (68, Fdd, (753.0, 783.0), 700),
    (69, SdlOnly, (2570.0, 2620.0), 2600),
    (70, Fdd, (1995.0, 2020.0), 2000),
    (71, Fdd, (617.0, 652.0), 600),
    (72, Fdd, (461.0, 466.0), 450),
    (73, Fdd, (460.0, 465.0), 450),
    (74, Fdd, (1475.0, 1518.0), 1500),
    (75, SdlOnly, (1432.0, 1517.0), 1500),
    (76, SdlOnly, (1427.0, 1432.0), 1400),
    (85, Fdd, (728.0, 746.0), 700),
    (87, Fdd, (420.0, 425.0), 400),
    (88, Fdd, (422.0, 427.0), 400),
];

pub fn lte_band_meta(band: u8) -> Option<LteBandMeta> {
    LTE_BANDS
        .iter()
        .find(|(b, ..)| *b == band)
        .map(|&(band, duplex, dl_mhz, label_mhz)| LteBandMeta {
            band,
            duplex,
            dl_mhz,
            label_mhz,
        })
}
