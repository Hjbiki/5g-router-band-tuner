use proptest::prelude::*;
use router_core::bands::lte::{lte_band_meta, LteBandSet};
use router_core::bands::Duplex;
use router_core::RouterError;

fn set(bands: &[u8]) -> LteBandSet {
    LteBandSet::from_bands(bands).unwrap()
}

#[test]
fn bookmarklet_regressions_encode_to_huawei_hex() {
    assert_eq!(set(&[2, 4]).to_huawei_hex(), "A");
    assert_eq!(set(&[3, 4]).to_huawei_hex(), "C");
    assert_eq!(set(&[1, 2, 3, 4]).to_huawei_hex(), "F");
    assert_eq!(set(&[38, 40]).to_huawei_hex(), "A000000000");
    assert_eq!(set(&[1, 3, 20]).to_huawei_hex(), "80005");
    assert_eq!(set(&[1, 3, 7, 8, 20, 28]).to_huawei_hex(), "80800C5");
}

#[test]
fn auto_is_all_firmware_bands() {
    assert_eq!(
        LteBandSet::from_huawei_hex("7fffffffffffffff").unwrap(),
        LteBandSet::AUTO
    );
    assert_eq!(LteBandSet::AUTO.label(), "Auto");
    assert_eq!(LteBandSet::AUTO.to_huawei_hex(), "7FFFFFFFFFFFFFFF");
}

#[test]
fn zero_encodes_as_single_zero_digit() {
    assert_eq!(LteBandSet::default().to_huawei_hex(), "0");
    assert_eq!(LteBandSet::default().label(), "None");
    assert!(LteBandSet::default().is_empty());
}

#[test]
fn parse_rejects_empty_non_hex_and_overlong() {
    assert!(matches!(
        LteBandSet::from_huawei_hex(""),
        Err(RouterError::Parse { .. })
    ));
    assert!(matches!(
        LteBandSet::from_huawei_hex("   "),
        Err(RouterError::Parse { .. })
    ));
    assert!(matches!(
        LteBandSet::from_huawei_hex("G1"),
        Err(RouterError::Parse { .. })
    ));
    assert!(matches!(
        LteBandSet::from_huawei_hex(&"F".repeat(33)),
        Err(RouterError::Parse { .. })
    ));
}

#[test]
fn parse_accepts_any_case_whitespace_and_32_digits() {
    assert_eq!(LteBandSet::from_huawei_hex("  a\n").unwrap(), set(&[2, 4]));
    assert!(LteBandSet::from_huawei_hex(&"F".repeat(32)).is_ok());
}

#[test]
fn from_bands_rejects_out_of_range() {
    assert!(matches!(
        LteBandSet::from_bands(&[0]),
        Err(RouterError::Validation { .. })
    ));
    assert!(matches!(
        LteBandSet::from_bands(&[129]),
        Err(RouterError::Validation { .. })
    ));
    assert!(LteBandSet::from_bands(&[1, 128]).is_ok());
}

#[test]
fn bands_and_contains_list_members_in_order() {
    let s = set(&[20, 1, 3]);
    assert_eq!(s.bands(), vec![1, 3, 20]);
    assert!(s.contains(3));
    assert!(!s.contains(2));
    assert!(!s.contains(0));
    assert!(!s.contains(129));
}

#[test]
fn subset() {
    assert!(set(&[3]).is_subset_of(&set(&[1, 3])));
    assert!(!set(&[1, 3]).is_subset_of(&set(&[3])));
}

#[test]
fn union_and_intersection() {
    assert_eq!(set(&[1, 3]).union(&set(&[3, 7])), set(&[1, 3, 7]));
    assert_eq!(set(&[1, 3]).intersection(&set(&[3, 7])), set(&[3]));
}

#[test]
fn label_joins_bands() {
    assert_eq!(set(&[1, 3, 28]).label(), "B1+B3+B28");
}

#[test]
fn serde_uses_huawei_hex_string() {
    assert_eq!(serde_json::to_string(&set(&[2, 4])).unwrap(), "\"A\"");
    assert_eq!(
        serde_json::from_str::<LteBandSet>("\"a\"").unwrap(),
        set(&[2, 4])
    );
    assert!(serde_json::from_str::<LteBandSet>("\"zz\"").is_err());
}

#[test]
fn band_meta_lookup() {
    let b3 = lte_band_meta(3).unwrap();
    assert_eq!(b3.duplex, Duplex::Fdd);
    assert_eq!(b3.dl_mhz, (1805.0, 1880.0));
    assert_eq!(b3.label_mhz, 1800);
    assert_eq!(lte_band_meta(42).unwrap().duplex, Duplex::Tdd);
    assert_eq!(lte_band_meta(29).unwrap().duplex, Duplex::SdlOnly);
    assert!(lte_band_meta(65).is_some());
    assert!(lte_band_meta(85).is_some());
    assert!(lte_band_meta(16).is_none());
    assert!(lte_band_meta(54).is_none());
    assert!(lte_band_meta(0).is_none());
}

#[test]
fn duplex_serializes_camel_case() {
    assert_eq!(
        serde_json::to_string(&Duplex::SdlOnly).unwrap(),
        "\"sdlOnly\""
    );
}

proptest! {
    #[test]
    fn hex_roundtrip(bits in any::<u128>()) {
        let s = LteBandSet::from_bits(bits);
        let hex = s.to_huawei_hex();
        prop_assert_eq!(LteBandSet::from_huawei_hex(&hex).unwrap(), s);
        prop_assert_eq!(LteBandSet::from_huawei_hex(&hex.to_lowercase()).unwrap(), s);
    }
}
