//! `RouterDriver` for the Huawei H122-373, assembled from the session and the parsers
//! (spec §5.5). Reboot (`Control` = 1) is the only device control that exists.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use quick_xml::escape::escape;
use tokio::time::Instant;

use super::session::{wrap_request, HuaweiSession};
use super::xml;
use crate::apn::{ApnDraft, ApnProfiles, ApnTarget};
use crate::diagnostics::EndpointRead;
use crate::{
    ApplyPreview, Capabilities, Credentials, DeviceInfo, DriverInfo, IpMethod, LoginGate,
    NetConfig, NetworkSearch, Operator, RouterDriver, RouterError, RouterIdentity, SessionState,
    Signal, Status, WanAddress,
};

const PLMN_TTL: Duration = Duration::from_secs(60);

const PATH_HEARTBEAT: &str = "/api/user/heartbeat";
const PATH_STATE_LOGIN: &str = "/api/user/state-login";
const PATH_SIGNAL: &str = "/api/device/signal";
const PATH_STATUS: &str = "/api/monitoring/status";
const PATH_PLMN: &str = "/api/net/current-plmn";
const PATH_NET_MODE: &str = "/api/net/net-mode";
const PATH_NET_MODE_LIST: &str = "/api/net/net-mode-list";
const PATH_DEVICE_INFO: &str = "/api/device/information";
const PATH_CONTROL: &str = "/api/device/control";
/// Ends the session (huawei-lte-api `user.logout`, `<Logout>1</Logout>`).
const PATH_LOGOUT: &str = "/api/user/logout";
/// The mobile data switch (`<dataswitch>1</dataswitch>` in huawei-lte-api). A GET reads it and a
/// POST of 0 or 1 turns data off or on; Refresh IP is offered only when the GET answers.
const PATH_DATASWITCH: &str = "/api/dialup/mobile-dataswitch";
/// The manual network search (huawei-lte-api `net.plmn_list`). The router drops its data session
/// while it searches and reconnects by itself, usually with a new address: the H122's Refresh IP,
/// as the user's bookmarklet does it. Only the number of networks is kept; the body (operator
/// names and codes) is dropped here, never logged, and must never be in `DIAGNOSTIC_PATHS`.
pub const PATH_PLMN_LIST: &str = "/api/net/plmn-list";
/// Huawei manual searches usually answer in 30 to 120 s, longer in NSA.
pub const SEARCH_TIMEOUT: Duration = Duration::from_secs(180);
/// The APN profiles (huawei-lte-api `dial_up.profiles`). The body holds the saved passwords, so
/// it is parsed and dropped here, never logged, and not in `DIAGNOSTIC_PATHS`.
const PATH_PROFILES: &str = "/api/dialup/profiles";
/// Whether the router picks the APN by itself (`<AutoAPN>`).
const PATH_AUTO_APN: &str = "/api/dialup/auto-apn";
/// The provider's remote management, TR-069 (huawei-lte-api `cwmp.basic_info`). The body can hold
/// the provider's account for this router, so it is parsed and dropped, never logged, and must
/// never be in `DIAGNOSTIC_PATHS`. Read only for the privacy check, when the user asks.
/// Answered on the H122 on 2026-11-13; its element names are in `docs/API_NOTES.md` §9.
const PATH_CWMP: &str = "/api/cwmp/basic-info";
/// Every device the router remembers, online or not (huawei-lte-api `lan.host_info`). The body
/// holds names and MAC addresses: privacy check only, never logged or in `DIAGNOSTIC_PATHS`.
/// Answered on the H122 on 2026-11-13.
const PATH_LAN_HOSTS: &str = "/api/lan/HostInfo";
/// The devices on Wi-Fi now (huawei-lte-api `wlan.host_list`), which `privacy::check` asks for
/// only when the router won't give `PATH_LAN_HOSTS`. Same rules.
const PATH_WLAN_HOSTS: &str = "/api/wlan/host-list";

/// Read-only endpoints for the diagnostics export. An error code from an endpoint the router
/// lacks is useful too: it tells a driver author what exists.
pub const DIAGNOSTIC_PATHS: &[&str] = &[
    PATH_DEVICE_INFO,
    "/api/device/basic_information",
    PATH_SIGNAL,
    PATH_STATUS,
    "/api/monitoring/converged-status",
    PATH_NET_MODE,
    PATH_NET_MODE_LIST,
    PATH_PLMN,
    PATH_STATE_LOGIN,
    PATH_DATASWITCH,
];

/// Cached `current-plmn` result and when it was fetched.
type PlmnCache = Option<(Instant, Option<Operator>)>;

pub struct HuaweiH122Driver {
    session: HuaweiSession,
    plmn: Mutex<PlmnCache>,
}

impl HuaweiH122Driver {
    /// `host` is a bare host (optionally with port) or `http://host`; paths are rejected.
    pub fn new(host: &str) -> Result<Self, RouterError> {
        let invalid = || RouterError::Validation {
            message: "invalid router address".to_string(),
        };
        let host = host.trim();
        let rest = host.strip_prefix("http://").unwrap_or(host);
        if rest.is_empty() || rest.contains(['/', '?', '#', '@']) || rest.contains("://") {
            return Err(invalid());
        }
        Ok(Self {
            session: HuaweiSession::new(&format!("http://{rest}"))?,
            plmn: Mutex::new(None),
        })
    }

    fn cached_operator(&self) -> Option<Option<Operator>> {
        let guard = self.plmn.lock().unwrap_or_else(|p| p.into_inner());
        match &*guard {
            Some((at, op)) if at.elapsed() < PLMN_TTL => Some(op.clone()),
            _ => None,
        }
    }

    async fn read_profiles(&self) -> Result<ApnProfiles, RouterError> {
        xml::parse_apn_profiles(&self.session.get(PATH_PROFILES).await?)
    }

    /// A profile add, edit or delete. huawei-lte-api sends these encrypted; the app sends plain
    /// XML, so a refusal other than "busy" is reported as possibly needing encryption.
    async fn post_profiles(&self, inner: &str) -> Result<(), RouterError> {
        match self.session.post(PATH_PROFILES, inner).await {
            Ok(_) => Ok(()),
            Err(RouterError::Api { kind, .. }) if kind != crate::ApiErrorKind::Busy => {
                Err(RouterError::Validation {
                    message: crate::apn::APN_NEEDS_ENCRYPTION.to_string(),
                })
            }
            Err(e) => Err(e),
        }
    }

    fn store_operator(&self, op: Option<Operator>) {
        *self.plmn.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), op));
    }
}

pub(crate) fn apply_fields(cfg: &NetConfig) -> Vec<(String, String)> {
    let mut f = vec![
        (
            "NetworkMode".to_string(),
            cfg.network_mode.code().to_string(),
        ),
        ("NetworkBand".to_string(), cfg.legacy_band_raw.clone()),
        ("LTEBand".to_string(), cfg.lte_bands.to_huawei_hex()),
    ];
    if let Some(o) = cfg.nr_option {
        f.push(("networkOption".to_string(), o.code().to_string()));
    }
    f
}

/// `Delete` 0, `SetDefault` the index, `Modify` 0: only the default profile changes.
pub(crate) fn set_default_fields(target: &ApnTarget) -> Vec<(String, String)> {
    vec![
        ("Delete".to_string(), "0".to_string()),
        ("SetDefault".to_string(), target.index.to_string()),
        ("Modify".to_string(), "0".to_string()),
    ]
}

/// `Delete` 0, `SetDefault` the profile in use, `Modify` 1 and the new profile, its tags in the
/// order huawei-lte-api's `create_profile` sends them. `SetDefault` names the profile already in
/// use, so the default doesn't move.
pub(crate) fn add_profile_inner(current: u32, draft: &ApnDraft, password: &str) -> String {
    let set: std::collections::HashMap<&str, String> = draft.tags(password).into_iter().collect();
    let v = |k: &str| set.get(k).cloned().unwrap_or_default();
    let profile: Vec<(String, String)> = [
        ("Index", String::new()),
        ("IsValid", "1".to_string()),
        ("Name", v("Name")),
        ("ApnIsStatic", v("ApnIsStatic")),
        ("ApnName", v("ApnName")),
        ("DialupNum", "*99#".to_string()),
        ("Username", v("Username")),
        ("Password", v("Password")),
        ("AuthMode", v("AuthMode")),
        ("IpIsStatic", String::new()),
        ("IpAddress", String::new()),
        ("DnsIsStatic", String::new()),
        ("PrimaryDns", String::new()),
        ("SecondaryDns", String::new()),
        ("ReadOnly", "0".to_string()),
        ("iptype", v("iptype")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    format!(
        "<Delete>0</Delete><SetDefault>{current}</SetDefault><Modify>1</Modify><Profile>{}</Profile>",
        apply_inner_xml(&profile)
    )
}

/// `Modify` 2 with the profile as the router sent it (`raw`, from `xml::parse_profile_tags`),
/// the draft's tags replaced and every other tag passed through.
pub(crate) fn edit_profile_inner(
    current: u32,
    mut raw: Vec<(String, String)>,
    draft: &ApnDraft,
    password: &str,
) -> String {
    for (k, v) in draft.tags(password) {
        match raw.iter_mut().find(|(t, _)| t == k) {
            Some(slot) => slot.1 = v,
            None => raw.push((k.to_string(), v)),
        }
    }
    format!(
        "<Delete>0</Delete><SetDefault>{current}</SetDefault><Modify>2</Modify><Profile>{}</Profile>",
        apply_inner_xml(&raw)
    )
}

/// `Delete` the index, `SetDefault` the profile in use, `Modify` 0 (huawei-lte-api
/// `delete_profile`).
pub(crate) fn delete_profile_inner(current: u32, index: u32) -> String {
    format!("<Delete>{index}</Delete><SetDefault>{current}</SetDefault><Modify>0</Modify>")
}

pub(crate) fn apply_inner_xml(fields: &[(String, String)]) -> String {
    fields
        .iter()
        .map(|(k, v)| format!("<{k}>{}</{k}>", escape(v.as_str())))
        .collect()
}

#[async_trait]
impl RouterDriver for HuaweiH122Driver {
    fn info(&self) -> DriverInfo {
        DriverInfo {
            id: "huawei_h122",
            name: "Huawei H122-373",
        }
    }

    fn apply_preview(&self, cfg: &NetConfig) -> ApplyPreview {
        let fields = apply_fields(cfg);
        let body = wrap_request(&apply_inner_xml(&fields));
        ApplyPreview { fields, body }
    }

    async fn login(&self, creds: &Credentials) -> Result<(), RouterError> {
        self.session.login(creds).await
    }

    async fn login_gated(
        &self,
        creds: &Credentials,
        gate: &LoginGate,
    ) -> Result<Option<RouterIdentity>, RouterError> {
        self.session.login_gated(creds, gate).await
    }

    /// "No rights" (100003) and a token error that survives the retry (125002 once the session
    /// expired) both reach here as `SessionLost`: the router has no session for us either way.
    async fn logout(&self) -> Result<(), RouterError> {
        match self.session.post(PATH_LOGOUT, "<Logout>1</Logout>").await {
            Ok(_) | Err(RouterError::SessionLost { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn forget_session(&self) {
        self.session.forget();
    }

    async fn session_state(&self) -> Result<SessionState, RouterError> {
        xml::parse_state_login(&self.session.get(PATH_STATE_LOGIN).await?)
    }

    async fn heartbeat(&self) -> Result<(), RouterError> {
        self.session.get(PATH_HEARTBEAT).await.map(|_| ())
    }

    async fn capabilities(&self) -> Result<Capabilities, RouterError> {
        let net_mode = self.net_config().await?;
        let list = self.session.get(PATH_NET_MODE_LIST).await?;
        let mut caps = xml::parse_capabilities(&list, &net_mode);
        // Any error (a missing endpoint, a busy router) leaves Refresh IP hidden until the next
        // capabilities read.
        caps.data_switch = self.data_switch().await.is_ok();
        // The data-switch POST was refused on the real H122 (2026-11-07); the search works.
        caps.ip_methods = vec![IpMethod::NetworkSearch];
        Ok(caps)
    }

    async fn device_info(&self) -> Result<DeviceInfo, RouterError> {
        // The raw body holds identifiers; it is parsed and dropped here, never logged.
        let body = self.session.get(PATH_DEVICE_INFO).await?;
        xml::parse_device_info(&body)
    }

    async fn status(&self) -> Result<Status, RouterError> {
        let mut status = xml::parse_status(&self.session.get(PATH_STATUS).await?)?;
        status.operator = match self.cached_operator() {
            Some(op) => op,
            None => {
                let op = match self.session.get(PATH_PLMN).await {
                    Ok(body) => xml::parse_operator(&body).unwrap_or(None),
                    Err(RouterError::Parse { .. }) => None,
                    Err(e) => return Err(e),
                };
                self.store_operator(op.clone());
                op
            }
        };
        Ok(status)
    }

    async fn signal(&self) -> Result<Signal, RouterError> {
        xml::parse_signal(&self.session.get(PATH_SIGNAL).await?)
    }

    async fn net_config(&self) -> Result<NetConfig, RouterError> {
        xml::parse_net_mode(&self.session.get(PATH_NET_MODE).await?)
    }

    async fn wan_address(&self) -> Result<WanAddress, RouterError> {
        xml::parse_wan(&self.session.get(PATH_DEVICE_INFO).await?)
    }

    async fn data_switch(&self) -> Result<bool, RouterError> {
        xml::parse_dataswitch(&self.session.get(PATH_DATASWITCH).await?)
    }

    async fn set_data_switch(&self, on: bool) -> Result<(), RouterError> {
        let inner = if on {
            "<dataswitch>1</dataswitch>"
        } else {
            "<dataswitch>0</dataswitch>"
        };
        self.session.post(PATH_DATASWITCH, inner).await.map(|_| ())
    }

    async fn network_search(&self) -> Result<NetworkSearch, RouterError> {
        let body = self
            .session
            .get_with_timeout(PATH_PLMN_LIST, SEARCH_TIMEOUT)
            .await?;
        Ok(NetworkSearch {
            networks: xml::count_plmn_networks(&body),
        })
    }

    async fn apn_profiles(&self) -> Result<ApnProfiles, RouterError> {
        let mut profiles = xml::parse_apn_profiles(&self.session.get(PATH_PROFILES).await?)?;
        // Any failure leaves Auto APN unknown; the list is still shown.
        profiles.auto_apn = match self.session.get(PATH_AUTO_APN).await {
            Ok(body) => xml::parse_auto_apn(&body).ok(),
            Err(e @ RouterError::SessionLost { .. }) => return Err(e),
            Err(_) => None,
        };
        Ok(profiles)
    }

    fn apn_preview(&self, target: &ApnTarget) -> ApplyPreview {
        let fields = set_default_fields(target);
        let body = wrap_request(&apply_inner_xml(&fields));
        ApplyPreview { fields, body }
    }

    /// Reads the profiles first and sends nothing unless `target` is still there, complete and
    /// unchanged. The POST is plain XML (huawei-lte-api `dial_up.set_default_profile`); whether
    /// the H122 takes it while data is connected isn't confirmed yet.
    async fn set_default_apn(&self, target: &ApnTarget) -> Result<(), RouterError> {
        let profiles = xml::parse_apn_profiles(&self.session.get(PATH_PROFILES).await?)?;
        crate::apn::check_target(&profiles, target)?;
        let inner = apply_inner_xml(&set_default_fields(target));
        self.session.post(PATH_PROFILES, &inner).await.map(|_| ())
    }

    async fn add_apn_profile(
        &self,
        expect_current: &ApnTarget,
        draft: &ApnDraft,
    ) -> Result<u32, RouterError> {
        let draft = draft.checked(true)?;
        let before = self.read_profiles().await?;
        crate::apn::check_write(&before, expect_current, None)?;
        let password = draft.password_text(None)?;
        let inner = add_profile_inner(expect_current.index, &draft, &password);
        drop(password);
        self.post_profiles(&inner).await?;
        let after = self.read_profiles().await?;
        crate::apn::check_after_write(&after, expect_current)?;
        crate::apn::added_index(&before, &after, &draft)
    }

    /// Built from a fresh read of the profile, so tags the app doesn't know go back unchanged.
    async fn edit_apn_profile(
        &self,
        expect_current: &ApnTarget,
        target: &ApnTarget,
        draft: &ApnDraft,
    ) -> Result<(), RouterError> {
        let draft = draft.checked(false)?;
        let body = self.session.get(PATH_PROFILES).await?;
        let before = xml::parse_apn_profiles(&body)?;
        crate::apn::check_write(&before, expect_current, Some(target))?;
        let raw = xml::parse_profile_tags(&body, target.index)?.ok_or_else(|| {
            RouterError::Validation {
                message: crate::apn::APN_PROFILE_MISSING.to_string(),
            }
        })?;
        drop(body);
        let saved = raw
            .iter()
            .find(|(k, _)| k == "Password")
            .map(|(_, v)| v.as_str());
        let password = draft.password_text(saved)?;
        let inner = edit_profile_inner(expect_current.index, raw, &draft, &password);
        drop(password);
        self.post_profiles(&inner).await?;
        let after = self.read_profiles().await?;
        crate::apn::check_after_write(&after, expect_current)?;
        crate::apn::check_edited(&after, target.index, &draft)
    }

    async fn delete_apn_profile(
        &self,
        expect_current: &ApnTarget,
        target: &ApnTarget,
    ) -> Result<(), RouterError> {
        let before = self.read_profiles().await?;
        crate::apn::check_write(&before, expect_current, Some(target))?;
        self.post_profiles(&delete_profile_inner(expect_current.index, target.index))
            .await?;
        let after = self.read_profiles().await?;
        crate::apn::check_after_write(&after, expect_current)?;
        crate::apn::check_deleted(&after, target.index)
    }

    /// One GET; the raw body is dropped here.
    async fn remote_management(
        &self,
    ) -> Result<crate::privacy::Seen<crate::privacy::RemoteManagement>, RouterError> {
        xml::parse_cwmp(&self.session.get_optional(PATH_CWMP).await?)
    }

    /// One GET; the raw body is dropped here.
    async fn lan_devices(
        &self,
    ) -> Result<crate::privacy::Seen<Vec<crate::privacy::LanDevice>>, RouterError> {
        xml::parse_hosts(&self.session.get_optional(PATH_LAN_HOSTS).await?)
    }

    /// One GET; the raw body is dropped here.
    async fn wifi_devices(
        &self,
    ) -> Result<crate::privacy::Seen<Vec<crate::privacy::LanDevice>>, RouterError> {
        xml::parse_hosts(&self.session.get_optional(PATH_WLAN_HOSTS).await?)
    }

    async fn apply_net_config(&self, cfg: &NetConfig) -> Result<(), RouterError> {
        let inner = apply_inner_xml(&apply_fields(cfg));
        self.session.post(PATH_NET_MODE, &inner).await.map(|_| ())
    }

    async fn reboot(&self) -> Result<(), RouterError> {
        self.session
            .post(PATH_CONTROL, "<Control>1</Control>")
            .await
            .map(|_| ())
    }

    /// `Control` = 2 on the same endpoint as reboot. The user's own bookmarklet sends exactly
    /// this and has reset this H122 (fw 11.0.2.21) successfully; see docs/API_NOTES.md §7.
    async fn factory_reset(&self) -> Result<(), RouterError> {
        self.session
            .post(PATH_CONTROL, "<Control>2</Control>")
            .await
            .map(|_| ())
    }

    /// GETs only, and only for a path `page_scripts::is_allowed_path` accepts; anything else is
    /// refused here without a request.
    async fn web_page(&self, path: &str) -> Result<Vec<u8>, RouterError> {
        if !crate::page_scripts::is_allowed_path(path) {
            return Err(RouterError::Validation {
                message: crate::page_scripts::PAGE_PATH_REFUSED.to_string(),
            });
        }
        self.session.get_page(path).await
    }

    async fn diagnostic_reads(&self) -> Vec<EndpointRead> {
        let mut out = Vec::with_capacity(DIAGNOSTIC_PATHS.len());
        for path in DIAGNOSTIC_PATHS {
            out.push(EndpointRead {
                path: path.to_string(),
                result: self.session.get(path).await,
            });
        }
        out
    }
}
