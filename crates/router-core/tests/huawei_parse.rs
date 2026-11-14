use router_core::drivers::huawei::{errors, xml};
use router_core::{
    ApiErrorKind, ConnState, LteBandSet, NetworkMode, NrOption, Rat, RouterError, SessionState,
};

fn fx(name: &str) -> String {
    let p = format!(
        "{}/tests/fixtures/huawei_h122/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).unwrap()
}

fn tag(text: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let s = text.find(&open).unwrap() + open.len();
    let e = text[s..].find('<').unwrap();
    text[s..s + e].to_string()
}

#[test]
fn parses_real_signal() {
    let s = xml::parse_signal(&fx("signal.xml")).unwrap();
    assert_eq!(s.serving, Some(Rat::Lte));
    let l = s.lte.unwrap();
    assert_eq!(l.band, Some(3));
    assert_eq!(l.earfcn_dl, Some(1650));
    assert_eq!(l.earfcn_ul, Some(19650));
    assert_eq!(l.pci, Some(101));
    assert_eq!(l.enb_id, Some(123456));
    assert_eq!(l.sector, Some(1));
    assert_eq!(l.cell_id, Some(31604737));
    assert_eq!(l.rsrp, Some(-100.0));
    assert_eq!(l.rsrq, Some(-8.0));
    assert_eq!(l.sinr, Some(0.0));
    assert_eq!(l.rssi, Some(-72.0));
    assert_eq!(l.tx_power, Some(2.0));
    assert_eq!(l.dl_bw_mhz, Some(20.0));
    assert_eq!(l.dl_mcs, [Some(1), Some(1)]);
    assert_eq!(l.ul_mcs, Some(0));
    assert_eq!(l.cqi, [Some(6), Some(6)]);
    assert_eq!(l.scc_pci, Some(202));
    let n = s.nr.unwrap();
    assert_eq!(n.arfcn_dl, Some(528980));
    assert_eq!(n.arfcn_ul, Some(528980));
    let b = n.band.unwrap();
    assert_eq!(b.primary, 41);
    assert_eq!(b.alternates, vec![90]);
    assert_eq!(n.rsrp, Some(-93.0));
    assert_eq!(n.rsrq, Some(-10.0));
    assert_eq!(n.sinr, Some(14.0));
    assert_eq!(n.tx_power, Some(19.0));
    assert_eq!(n.dl_bw_mhz, Some(90.0));
    assert_eq!(n.dl_mcs, [Some(14), Some(0)]);
    assert_eq!(n.ul_mcs, Some(23));
    assert_eq!(n.cqi, [None, None]);
}

#[test]
fn parses_decimal_cell_id() {
    let s = xml::parse_signal("<response><mode>7</mode><cell_id>25600002</cell_id></response>")
        .unwrap();
    let l = s.lte.unwrap();
    assert_eq!(l.enb_id, Some(100000));
    assert_eq!(l.sector, Some(2));
    assert_eq!(l.cell_id, Some(25600002));
}

#[test]
fn parses_signal_during_reattach() {
    let s = xml::parse_signal(&fx("signal_reattach.xml")).unwrap();
    assert_eq!(s.serving, None);
    assert_eq!(s.lte.unwrap().rsrp, None);
    assert!(s.nr.is_none());
}

#[test]
fn parses_bounds() {
    let s = xml::parse_signal(&fx("signal_bounds.xml")).unwrap();
    let l = s.lte.unwrap();
    assert_eq!(l.rsrp, Some(-44.0));
    assert_eq!(l.rsrq, Some(-19.5));
}

#[test]
fn parse_number_cases() {
    assert_eq!(xml::parse_number("-8.0dB"), Some(-8.0));
    assert_eq!(xml::parse_number("-100dBm"), Some(-100.0));
    assert_eq!(xml::parse_number(">=-44dBm"), Some(-44.0));
    assert_eq!(xml::parse_number("<-19.5dB"), Some(-19.5));
    assert_eq!(xml::parse_number("0dB"), Some(0.0));
    assert_eq!(xml::parse_number("Unknown"), None);
    assert_eq!(xml::parse_number(""), None);
    assert_eq!(xml::parse_number("NaN"), None);
}

#[test]
fn bandwidth_units() {
    let s = xml::parse_signal(
        "<response><mode>7</mode><dlbandwidth>1400KHz</dlbandwidth><ulbandwidth>20MHz</ulbandwidth></response>",
    )
    .unwrap();
    let l = s.lte.unwrap();
    assert_eq!(l.dl_bw_mhz, Some(1.4));
    assert_eq!(l.ul_bw_mhz, Some(20.0));
}

#[test]
fn parses_net_mode_fixture() {
    let c = xml::parse_net_mode(&fx("net_mode.xml")).unwrap();
    assert_eq!(c.network_mode, NetworkMode::Auto);
    assert_eq!(c.legacy_band_raw, "0");
    assert_eq!(c.nr_option, Some(NrOption::Nsa));
}

#[test]
fn parses_status_fixture() {
    let body = fx("monitoring_status.xml");
    let s = xml::parse_status(&body).unwrap();
    assert_eq!(s.connection, ConnState::Connected);
    let expected: u32 = tag(&body, "CurrentNetworkTypeEx").parse().unwrap();
    assert_eq!(s.network_type, expected);
    assert_eq!(s.bars, Some(4));
    assert_eq!(s.nr_bars, Some(5));
    assert_eq!(s.endc, Some(true));
    assert_eq!(s.endc_restricted, Some(false));
    assert!(s.operator.is_none());
}

#[test]
fn parses_operator() {
    let o = xml::parse_operator(&fx("current_plmn.xml"))
        .unwrap()
        .unwrap();
    assert_eq!(o.name, "Zain KSA");
    assert_eq!(o.mcc_mnc, "42004");
}

#[test]
fn parses_real_capabilities() {
    let cfg = xml::parse_net_mode(&fx("net_mode.xml")).unwrap();
    let c = xml::parse_capabilities(&fx("net_mode_list.xml"), &cfg);
    let expected = LteBandSet::from_huawei_hex("7e2880800d5").unwrap();
    assert_eq!(c.supported_lte, expected);
    assert_eq!(
        expected.bands(),
        vec![1, 3, 5, 7, 8, 20, 28, 32, 34, 38, 39, 40, 41, 42, 43]
    );
    assert_eq!(
        c.modes,
        vec![NetworkMode::Auto, NetworkMode::NrOnly, NetworkMode::LteOnly]
    );
    assert_eq!(
        c.nr_options,
        vec![NrOption::Nsa, NrOption::Sa, NrOption::SaNsa]
    );
    assert!(c.band_edit_allowed);
}

#[test]
fn capabilities_garbage_disables_editing() {
    let cfg = xml::parse_net_mode(&fx("net_mode.xml")).unwrap();
    assert!(!xml::parse_capabilities("<response></response>", &cfg).band_edit_allowed);
    let only_all = "<response><LTEBandList><LTEBand><Name>All bands</Name><Value>7fffffffffffffff</Value></LTEBand></LTEBandList></response>";
    assert!(!xml::parse_capabilities(only_all, &cfg).band_edit_allowed);
}

#[test]
fn error_in_200_is_detected() {
    let body = fx("error_lan_wan_config.xml");
    let expected: u32 = tag(&body, "code").parse().unwrap();
    match xml::check_error(&body) {
        Err(RouterError::Api { code, .. }) => assert_eq!(code, expected),
        other => panic!("expected Api error, got {other:?}"),
    }
    assert!(xml::check_error(&fx("host_info_ok.xml")).is_ok());
}

#[test]
fn error_kinds() {
    assert_eq!(errors::kind(125002), ApiErrorKind::Token);
    assert_eq!(errors::kind(108002), ApiErrorKind::BadCredentials);
    assert_eq!(errors::kind(100002), ApiErrorKind::NotSupported);
    assert_eq!(errors::kind(1), ApiErrorKind::Unknown);
}

#[test]
fn device_info_reads_only_safe_fields() {
    let body = fx("device_information.xml");
    let d = xml::parse_device_info(&body).unwrap();
    assert_eq!(d.model, "H122-373");
    // The fixture sends `11.0.2.21&#40;H329SP1C7002&#41;`: character references must be kept,
    // not cut off at the first one (quick-xml >= 0.38 reports them as separate events).
    assert_eq!(d.firmware, "11.0.2.21(H329SP1C7002)");
    let imei = tag(&body, "Imei");
    assert!(!imei.is_empty());
    assert!(!format!("{d:?}").contains(&imei));
}

#[test]
fn state_login_fixtures() {
    assert_eq!(
        xml::parse_state_login(&fx("state_login_logged_out.xml")).unwrap(),
        SessionState::LoggedOut
    );
    assert_eq!(
        xml::parse_state_login(&fx("state_login_logged_in.xml")).unwrap(),
        SessionState::LoggedIn
    );
    assert_eq!(
        xml::parse_state_login(
            "<response><State>-1</State><lockstatus>1</lockstatus><remainwaittime>120</remainwaittime></response>"
        )
        .unwrap(),
        SessionState::Locked { wait_s: 120 }
    );
}

#[test]
fn lte_absent_when_no_lte_fields() {
    let s = xml::parse_signal(
        "<response><mode>2</mode><rscp>-85dBm</rscp><ecio>-6dB</ecio></response>",
    )
    .unwrap();
    assert!(s.lte.is_none());
    assert_eq!(s.serving, Some(Rat::Umts));
}

#[test]
fn lte_absent_for_empty_body() {
    let s = xml::parse_signal("<response></response>").unwrap();
    assert!(s.lte.is_none());
    assert!(s.nr.is_none());
    assert!(s.serving.is_none());
}

#[test]
fn text_with_entity_references_is_decoded_whole() {
    let d = xml::parse_device_info(
        "<response><DeviceName>A &amp; B&#32;&lt;x&gt;</DeviceName><SoftwareVersion>1.2&#40;C&#x29;</SoftwareVersion></response>",
    )
    .unwrap();
    assert_eq!(d.model, "A & B <x>");
    assert_eq!(d.firmware, "1.2(C)");
}

#[test]
fn unknown_entity_is_a_parse_error() {
    assert!(xml::parse_device_info(
        "<response><DeviceName>&bogus;</DeviceName><SoftwareVersion>1</SoftwareVersion></response>"
    )
    .is_err());
}

#[test]
fn only_the_home_network_of_an_imsi_gets_out() {
    let imsi = |prefix: &str| format!("{prefix}{}", "0".repeat(15 - prefix.len()));
    assert_eq!(xml::home_plmn(&imsi("42009")).as_deref(), Some("42009"));
    assert_eq!(
        xml::home_plmn(&format!(" {} ", imsi("42004"))).as_deref(),
        Some("42004")
    );
    // Elsewhere the MNC length varies, so nothing is guessed.
    assert_eq!(xml::home_plmn(&imsi("31026")), None);
    assert_eq!(xml::home_plmn(""), None);
    assert_eq!(xml::home_plmn("42009"), None);
    assert_eq!(xml::home_plmn("42009abc0000000"), None);

    // Through the real parser: the snapshot's device info holds the code, not the IMSI.
    let full = imsi("42009");
    let body = format!(
        "<response><DeviceName>H122-373</DeviceName><SoftwareVersion>1</SoftwareVersion><WebUIVersion>1</WebUIVersion><Imsi>{full}</Imsi></response>"
    );
    let d = xml::parse_device_info(&body).unwrap();
    assert_eq!(d.home_plmn.as_deref(), Some("42009"));
    let as_json = serde_json::to_string(&d).unwrap();
    assert!(!as_json.contains(&full), "{as_json}");
    assert!(!format!("{d:?}").contains(&full));
}

#[test]
fn the_sims_name_is_read_when_it_says_something() {
    let body = |spn: &str| {
        format!("<response><State>0</State><FullName>Zain KSA</FullName><ShortName>Zain KSA</ShortName><Numeric>42004</Numeric><Rat>7</Rat><Spn>{spn}</Spn></response>")
    };
    let spn = |s: &str| xml::parse_operator(&body(s)).unwrap().unwrap().spn;
    assert_eq!(spn(""), None);
    assert_eq!(spn("  "), None);
    assert_eq!(spn(" Salam ").as_deref(), Some("Salam"));
    assert_eq!(spn(&"x".repeat(100)).map(|s| s.len()), Some(40));
}

#[test]
fn parses_wan_from_device_information() {
    let w = xml::parse_wan(&fx("device_information.xml")).unwrap();
    assert_eq!(w.ipv4, Some(std::net::Ipv4Addr::new(192, 0, 2, 17)));
    assert!(w.ipv6_present);
}

#[test]
fn wan_without_a_data_connection_reads_as_none() {
    for v4 in ["", "0.0.0.0", "not-an-ip", "10.45.1"] {
        let body = format!(
            "<response><WanIPAddress>{v4}</WanIPAddress><WanIPv6Address></WanIPv6Address></response>"
        );
        let w = xml::parse_wan(&body).unwrap();
        assert_eq!(w.ipv4, None, "{v4:?}");
        assert!(!w.ipv6_present);
    }
    let w = xml::parse_wan("<response><DeviceName>x</DeviceName></response>").unwrap();
    assert_eq!(w.ipv4, None);
    let w = xml::parse_wan("<response><WanIPv6Address>::</WanIPv6Address></response>").unwrap();
    assert!(!w.ipv6_present);
}

#[test]
fn dataswitch_is_0_or_1() {
    let body = |v: &str| format!("<response><dataswitch>{v}</dataswitch></response>");
    assert!(xml::parse_dataswitch(&body("1")).unwrap());
    assert!(!xml::parse_dataswitch(&body("0")).unwrap());
    for bad in ["", "2", "on"] {
        assert!(matches!(
            xml::parse_dataswitch(&body(bad)),
            Err(RouterError::Parse { .. })
        ));
    }
    assert!(xml::parse_dataswitch("<response></response>").is_err());
}

#[test]
fn plmn_list_is_counted_and_nothing_else_is_read() {
    assert_eq!(xml::count_plmn_networks(&fx("plmn_list.xml")), Some(3));
    assert_eq!(
        xml::count_plmn_networks("<response><Networks></Networks></response>"),
        Some(0)
    );
    assert_eq!(xml::count_plmn_networks("<response/>"), Some(0));
    // Not the router's answer: unknown, not an error.
    assert_eq!(xml::count_plmn_networks("not xml <<"), None);
    assert_eq!(xml::count_plmn_networks(""), None);
    assert_eq!(
        xml::count_plmn_networks("<error><code>100002</code></error>"),
        None
    );
}
