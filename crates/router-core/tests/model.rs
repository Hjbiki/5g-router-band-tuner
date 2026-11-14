use std::net::Ipv4Addr;

use router_core::{
    in_service, ipv4_range, wan_kind, ConnState, Credentials, NetworkMode, NrOption, Rat, Signal,
    Status, WanKind, WanView,
};

#[test]
fn network_mode_codes_roundtrip() {
    assert_eq!(NetworkMode::from_code("03"), NetworkMode::LteOnly);
    assert_eq!(NetworkMode::from_code("00"), NetworkMode::Auto);
    assert_eq!(NetworkMode::from_code("08"), NetworkMode::NrOnly);
    assert_eq!(NetworkMode::from_code("0A").code(), "0A");
}

#[test]
fn nr_option_codes() {
    assert_eq!(NrOption::from_code(2), Some(NrOption::SaNsa));
    assert_eq!(NrOption::from_code(3), None);
    assert_eq!(NrOption::Sa.code(), 1);
}

#[test]
fn conn_state_codes() {
    assert_eq!(ConnState::from_code(901), ConnState::Connected);
    assert_eq!(ConnState::from_code(904), ConnState::Failed);
    assert_eq!(ConnState::from_code(905), ConnState::NoService);
    assert_eq!(ConnState::from_code(906), ConnState::Failed);
    assert_eq!(ConnState::from_code(42), ConnState::Other { code: 42 });
}

#[test]
fn credentials_debug_redacts() {
    let c = Credentials::new("admin", "s3cret");
    assert!(!format!("{c:?}").contains("s3cret"));
    assert_eq!(c.password(), "s3cret");
}

fn status(connection: ConnState) -> Status {
    Status {
        connection,
        network_type: 0,
        operator: None,
        bars: None,
        nr_bars: None,
        endc: None,
        endc_restricted: None,
    }
}

fn signal(serving: Option<Rat>) -> Signal {
    Signal {
        serving,
        lte: None,
        nr: None,
    }
}

#[test]
fn in_service_requires_both() {
    assert!(!in_service(&status(ConnState::Connected), &signal(None)));
    assert!(!in_service(
        &status(ConnState::Disconnected),
        &signal(Some(Rat::Lte))
    ));
    assert!(in_service(
        &status(ConnState::Connected),
        &signal(Some(Rat::Lte))
    ));
}

#[test]
fn error_json_shape() {
    use router_core::{ApiErrorKind, RouterError};
    let e = RouterError::Api {
        code: 125002,
        kind: ApiErrorKind::Token,
    };
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"kind":"api","code":125002,"errorKind":"token"}"#
    );
    let e = RouterError::LockedOut { wait_s: 120 };
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"kind":"lockedOut","waitS":120}"#
    );
    let m = NetworkMode::Raw { value: "0A".into() };
    assert_eq!(
        serde_json::to_string(&m).unwrap(),
        r#"{"kind":"raw","value":"0A"}"#
    );
}

#[test]
fn ipv4_range_keeps_the_first_two_numbers() {
    assert_eq!(ipv4_range(Ipv4Addr::new(10, 45, 23, 7)), "10.45");
    assert_eq!(ipv4_range(Ipv4Addr::new(203, 0, 113, 9)), "203.0");
}

#[test]
fn wan_kind_by_range() {
    let k = |a, b, c, d| wan_kind(Ipv4Addr::new(a, b, c, d));
    assert_eq!(k(100, 64, 0, 1), WanKind::Cgnat);
    assert_eq!(k(100, 127, 255, 254), WanKind::Cgnat);
    assert_eq!(k(100, 63, 0, 1), WanKind::Public);
    assert_eq!(k(100, 128, 0, 1), WanKind::Public);
    assert_eq!(k(10, 45, 1, 2), WanKind::Private);
    assert_eq!(k(172, 16, 0, 1), WanKind::Private);
    assert_eq!(k(172, 31, 255, 1), WanKind::Private);
    assert_eq!(k(172, 32, 0, 1), WanKind::Public);
    assert_eq!(k(192, 168, 8, 1), WanKind::Private);
    assert_eq!(k(192, 169, 0, 1), WanKind::Public);
    assert_eq!(k(203, 0, 113, 9), WanKind::Public);
}

#[test]
fn wan_view_without_an_address() {
    let v = WanView::new(None, true);
    assert_eq!(v.ipv4, None);
    assert_eq!(v.range, None);
    assert_eq!(v.kind, None);
    assert!(v.ipv6);
    let v = WanView::new(Some(Ipv4Addr::new(100, 70, 3, 4)), false);
    assert_eq!(v.ipv4.as_deref(), Some("100.70.3.4"));
    assert_eq!(v.range.as_deref(), Some("100.70"));
    assert_eq!(v.kind, Some(WanKind::Cgnat));
}
