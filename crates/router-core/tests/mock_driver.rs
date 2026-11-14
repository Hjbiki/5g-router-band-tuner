use std::time::Duration;

use router_core::drivers::huawei::HuaweiH122Driver;
use router_core::drivers::mock::{Fault, MockDriver, MockScenario};
use router_core::{
    Credentials, LteBandSet, NetConfig, NetworkMode, NrOption, Rat, RouterDriver, RouterError,
    SessionState,
};

async fn mock() -> MockDriver {
    let m = MockDriver::new(MockScenario::zain_like());
    m.login(&Credentials::new("admin", "mock")).await.unwrap();
    m
}

fn cfg(bands: &[u8]) -> NetConfig {
    NetConfig {
        network_mode: NetworkMode::Auto,
        lte_bands: LteBandSet::from_bands(bands).unwrap(),
        legacy_band_raw: "0".into(),
        nr_option: Some(NrOption::Nsa),
    }
}

#[tokio::test(start_paused = true)]
async fn serves_best_sinr() {
    let m = mock().await;
    let s = m.signal().await.unwrap();
    assert_eq!(s.serving, Some(Rat::Lte));
    assert_eq!(s.lte.unwrap().band, Some(28));
    assert!(s.nr.is_none());
}

#[tokio::test(start_paused = true)]
async fn reattach_gap() {
    let m = mock().await;
    m.apply_net_config(&cfg(&[3])).await.unwrap();
    tokio::time::advance(Duration::from_secs(5)).await;
    let s = m.signal().await.unwrap();
    assert_eq!(s.serving, None);
    assert!(s.lte.is_none() && s.nr.is_none());
    tokio::time::advance(Duration::from_secs(11)).await;
    let s = m.signal().await.unwrap();
    assert_eq!(s.serving, Some(Rat::Lte));
    assert_eq!(s.lte.unwrap().band, Some(3));
    assert_eq!(s.nr.unwrap().arfcn_dl, Some(528980));
}

#[tokio::test(start_paused = true)]
async fn no_cell_means_no_service() {
    let m = mock().await;
    m.apply_net_config(&cfg(&[7])).await.unwrap();
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(m.signal().await.unwrap().serving, None);
    assert_eq!(
        m.status().await.unwrap().connection,
        router_core::ConnState::NoService
    );
}

#[tokio::test(start_paused = true)]
async fn faults_apply() {
    let m = mock().await;
    m.set_fault(Some(Fault::ApplyError(100005)));
    assert_eq!(
        m.apply_net_config(&cfg(&[3])).await,
        Err(RouterError::Api {
            code: 100005,
            kind: router_core::drivers::huawei::errors::kind(100005)
        })
    );
    assert!(m.applied().is_empty());
    m.set_fault(Some(Fault::VerifyMismatch));
    let before = m.net_config().await.unwrap();
    m.apply_net_config(&cfg(&[3])).await.unwrap();
    assert_eq!(m.net_config().await.unwrap(), before);
    assert!(m.applied().is_empty());
}

#[tokio::test(start_paused = true)]
async fn wrong_password() {
    let m = mock().await;
    assert_eq!(
        m.login(&Credentials::new("admin", "nope")).await,
        Err(RouterError::BadCredentials)
    );
    m.login(&Credentials::new("admin", "mock")).await.unwrap();
    m.set_password("x");
    assert!(m.login(&Credentials::new("admin", "mock")).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn reboot_and_faults() {
    let m = mock().await;
    m.reboot().await.unwrap();
    m.login(&Credentials::new("admin", "mock")).await.unwrap();
    assert_eq!(m.signal().await.unwrap().serving, None);
    tokio::time::advance(Duration::from_secs(16)).await;
    assert!(m.signal().await.unwrap().serving.is_some());
    m.set_fault(Some(Fault::Unreachable));
    assert_eq!(m.heartbeat().await, Err(RouterError::Unreachable));
    m.set_fault(Some(Fault::SessionTakeover));
    assert_eq!(m.heartbeat().await, Err(RouterError::SessionTakenOver));
    m.set_fault(None);
    m.login(&Credentials::new("admin", "mock")).await.unwrap();
    m.set_fault(Some(Fault::NoServiceForever));
    assert_eq!(m.signal().await.unwrap().serving, None);
    m.set_fault(None);
    assert!(m.signal().await.unwrap().serving.is_some());
}

#[tokio::test(start_paused = true)]
async fn starts_logged_out() {
    let m = MockDriver::new(MockScenario::zain_like());
    assert_eq!(m.session_state().await, Ok(SessionState::LoggedOut));
    assert_eq!(
        m.signal().await,
        Err(RouterError::SessionLost { code: None })
    );
}

#[tokio::test(start_paused = true)]
async fn takeover_requires_relogin() {
    let m = mock().await;
    m.set_fault(Some(Fault::SessionTakeover));
    assert_eq!(m.signal().await, Err(RouterError::SessionTakenOver));
    m.set_fault(None);
    assert_eq!(
        m.signal().await,
        Err(RouterError::SessionLost { code: None })
    );
    m.login(&Credentials::new("admin", "mock")).await.unwrap();
    assert!(m.signal().await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn bad_password_counts_and_stays_out() {
    let m = MockDriver::new(MockScenario::zain_like());
    m.set_password("other");
    assert_eq!(
        m.login(&Credentials::new("admin", "mock")).await,
        Err(RouterError::BadCredentials)
    );
    assert_eq!(m.login_attempts(), 1);
    assert_eq!(m.session_state().await, Ok(SessionState::LoggedOut));
}

#[tokio::test(start_paused = true)]
async fn reboot_logs_out() {
    let m = mock().await;
    m.reboot().await.unwrap();
    assert_eq!(m.session_state().await, Ok(SessionState::LoggedOut));
    assert_eq!(
        m.signal().await,
        Err(RouterError::SessionLost { code: None })
    );
}

#[tokio::test(start_paused = true)]
async fn preview_matches_huawei() {
    let m = MockDriver::new(MockScenario::zain_like());
    let real = HuaweiH122Driver::new("192.0.2.1").unwrap();
    let c = cfg(&[1, 3]);
    assert_eq!(m.apply_preview(&c), real.apply_preview(&c));
}

/// A search drops service and the address for its duration, then hands out the next queued
/// address; the data switch is never touched.
#[tokio::test(start_paused = true)]
async fn a_network_search_drops_the_link_for_its_duration() {
    use std::sync::Arc;
    let m = Arc::new(mock().await);
    let caps = m.capabilities().await.unwrap();
    assert_eq!(
        caps.ip_methods,
        vec![
            router_core::IpMethod::DataSwitch,
            router_core::IpMethod::NetworkSearch
        ]
    );
    let next = router_core::drivers::mock::mock_wan(1, 7);
    m.queue_wan(&[next]);
    m.set_search_duration(Duration::from_secs(40));
    let searching = tokio::spawn({
        let m = m.clone();
        async move { m.network_search().await }
    });
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(m.signal().await.unwrap().serving, None);
    assert_eq!(m.wan_address().await.unwrap().ipv4, None);
    assert_eq!(m.searches(), 1);
    let found = searching.await.unwrap().unwrap();
    assert_eq!(found.networks, Some(3));
    assert!(m.signal().await.unwrap().serving.is_some());
    assert_eq!(m.wan_address().await.unwrap().ipv4, Some(next));
    assert!(m.switch_posts().is_empty());

    m.set_network_search_supported(false);
    m.set_data_switch_supported(false);
    assert!(m.capabilities().await.unwrap().ip_methods.is_empty());
}

#[tokio::test(start_paused = true)]
async fn search_and_off_refusals_answer_at_once_with_their_code() {
    let m = mock().await;
    let refusal = RouterError::SessionLost { code: Some(100003) };
    m.refuse_search(Some(refusal.clone()));
    assert_eq!(m.network_search().await, Err(refusal.clone()));
    assert_eq!(m.searches(), 0);
    assert!(m.signal().await.unwrap().serving.is_some());

    m.refuse_data_off(Some(refusal.clone()));
    assert_eq!(m.set_data_switch(false).await, Err(refusal));
    assert!(m.data_on());
    m.set_data_switch(true).await.unwrap();
    m.refuse_data_off(None);
    m.set_data_switch(false).await.unwrap();
    assert!(!m.data_on());
}

#[tokio::test(start_paused = true)]
async fn a_search_can_change_the_bands_and_break_service() {
    let m = mock().await;
    m.set_search_duration(Duration::from_secs(1));
    m.search_sets_config(Some(cfg(&[1, 3])));
    m.search_breaks_service(true);
    m.network_search().await.unwrap();
    assert_eq!(m.net_config().await.unwrap(), cfg(&[1, 3]));
    assert!(m.applied().is_empty(), "not an apply");
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(m.signal().await.unwrap().serving, None);
}
