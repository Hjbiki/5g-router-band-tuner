//! APN profiles: the Huawei parser, the fingerprint and what never leaves the parser.

use router_core::apn::{fingerprint, is_fingerprint};
use router_core::drivers::huawei::xml;
use router_core::{ApnAuth, ApnIpType};

fn fx(name: &str) -> String {
    let p = format!(
        "{}/tests/fixtures/huawei_h122/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).unwrap()
}

const SECRET: &str = "synthetic-secret-1234";

#[test]
fn parses_the_profile_list() {
    let p = xml::parse_apn_profiles(&fx("dialup_profiles.xml")).unwrap();
    assert_eq!(p.current, Some(1));
    assert_eq!(p.auto_apn, None, "read from its own endpoint");
    assert_eq!(p.profiles.len(), 3, "a profile without an index is skipped");

    let op = &p.profiles[0];
    assert_eq!(op.index, 1);
    assert_eq!(op.name, "Operator internet");
    assert_eq!(op.apn.as_deref(), Some("internet.example"));
    assert!(op.apn_static && op.read_only && op.valid && !op.has_password);
    assert_eq!(op.auth, ApnAuth::Auto);
    assert_eq!(op.ip_type, ApnIpType::Ipv4v6);

    let work = &p.profiles[1];
    assert_eq!(work.name, "Work & travel");
    assert_eq!(work.username, "demo-user");
    assert!(work.has_password && !work.read_only);
    assert_eq!(work.auth, ApnAuth::Chap);
    assert_eq!(work.ip_type, ApnIpType::Ipv4);

    let net = &p.profiles[2];
    assert_eq!(net.apn, None, "the network chooses it");
    assert!(!net.apn_static && !net.valid);
    assert!(net.has_password, "a masked password still counts as set");
    assert_eq!(net.auth, ApnAuth::Pap);
    assert_eq!(net.ip_type, ApnIpType::Ipv6);
    assert_eq!(p.get(2), Some(work));
    assert_eq!(p.current_target().unwrap().index, 1);
}

#[test]
fn the_password_never_leaves_the_parser() {
    let p = xml::parse_apn_profiles(&fx("dialup_profiles.xml")).unwrap();
    let json = serde_json::to_string(&p).unwrap();
    let debug = format!("{p:?}");
    for text in [&json, &debug] {
        assert!(!text.contains(SECRET), "{text}");
        assert!(!text.contains("******"), "{text}");
        assert!(!text.to_lowercase().contains("\"password\""), "{text}");
    }
}

#[test]
fn the_fingerprint_is_stable_and_leaves_out_the_password() {
    let a = xml::parse_apn_profiles(&fx("dialup_profiles.xml")).unwrap();
    let other_pw = fx("dialup_profiles.xml").replace(SECRET, "another-made-up-one");
    let b = xml::parse_apn_profiles(&other_pw).unwrap();
    assert_eq!(a.profiles[1].fingerprint, b.profiles[1].fingerprint);
    for p in &a.profiles {
        assert!(is_fingerprint(&p.fingerprint), "{}", p.fingerprint);
    }
    assert_eq!(
        fingerprint("Work & travel", "work.example", "demo-user", "2", "0"),
        a.profiles[1].fingerprint
    );
    // Every field counts, and the separator keeps fields apart.
    let base = fingerprint("a", "b", "c", "0", "2");
    assert_eq!(base, fingerprint("a", "b", "c", "0", "2"));
    for other in [
        fingerprint("x", "b", "c", "0", "2"),
        fingerprint("a", "x", "c", "0", "2"),
        fingerprint("a", "b", "x", "0", "2"),
        fingerprint("a", "b", "c", "1", "2"),
        fingerprint("a", "b", "c", "0", "0"),
        fingerprint("ab", "", "c", "0", "2"),
    ] {
        assert_ne!(base, other);
    }
    let renamed = fx("dialup_profiles.xml").replace("work.example", "work2.example");
    let c = xml::parse_apn_profiles(&renamed).unwrap();
    assert_ne!(a.profiles[1].fingerprint, c.profiles[1].fingerprint);
}

#[test]
fn router_text_is_bounded_and_cleaned() {
    let long = "N".repeat(200);
    let body = format!(
        "<response><CurrentProfile>4</CurrentProfile><Profiles><Profile><Index>4</Index>\
         <IsValid>1</IsValid><Name>{long}&#10;x</Name><ApnIsStatic>1</ApnIsStatic>\
         <ApnName>{long}</ApnName><ReadOnly>0</ReadOnly></Profile></Profiles></response>"
    );
    let p = xml::parse_apn_profiles(&body).unwrap();
    let pr = &p.profiles[0];
    assert_eq!(pr.name.chars().count(), 64);
    assert!(!pr.name.contains('\n'));
    assert_eq!(pr.apn.as_ref().unwrap().len(), 100);
    // A missing ReadOnly or unknown codes are the safe reading.
    let bare = "<response><Profiles><Profile><Index>5</Index><AuthMode>9</AuthMode>\
                <iptype>7</iptype></Profile></Profiles></response>";
    let q = xml::parse_apn_profiles(bare).unwrap();
    assert_eq!(q.current, None);
    let pr = &q.profiles[0];
    assert!(pr.read_only && !pr.valid);
    assert_eq!(pr.auth, ApnAuth::Other);
    assert_eq!(pr.ip_type, ApnIpType::Other);
}

#[test]
fn auto_apn_and_errors() {
    assert!(xml::parse_auto_apn("<response><AutoAPN>1</AutoAPN></response>").unwrap());
    assert!(!xml::parse_auto_apn("<response><AutoAPN>0</AutoAPN></response>").unwrap());
    assert!(xml::parse_auto_apn("<response></response>").is_err());
    assert!(xml::parse_apn_profiles("<error><code>100002</code></error>").is_err());
    assert!(xml::parse_apn_profiles("<html></html>").is_err());
}
