use std::fmt;
use std::net::Ipv4Addr;

use async_trait::async_trait;

use crate::{Capabilities, DeviceInfo, NetConfig, RouterError, Signal, Status};

/// The refusal when no factory-reset call is confirmed for this router model.
pub const FACTORY_RESET_NOT_VERIFIED: &str = "factory reset is not verified on this router model";

/// The refusal from a driver that can't read the WAN address.
pub const WAN_NOT_SUPPORTED: &str = "reading the internet address is not supported";

/// The refusal from a driver that has no mobile data switch.
pub const DATA_SWITCH_NOT_SUPPORTED: &str = "this router has no mobile data switch";

/// The refusal from a driver that has no manual network search.
pub const NETWORK_SEARCH_NOT_SUPPORTED: &str = "this router has no network search";

/// What a manual network search found. Only the count is kept: the operators' names and codes
/// are dropped in the driver and never logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkSearch {
    /// Networks the router listed; `None` when the answer couldn't be read.
    pub networks: Option<u8>,
}

/// The router's internet address as the driver read it. The full IPv4 address stays in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WanAddress {
    /// `None` while there is no data connection (empty or 0.0.0.0).
    pub ipv4: Option<Ipv4Addr>,
    pub ipv6_present: bool,
}

/// Login credentials. Deliberately not `Serialize`; `Debug` hides the password.
#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    password: String,
}

impl Credentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username)
            .field("password", &"***")
            .finish()
    }
}

/// What a router tells anyone before a login, and a device without the password can't guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouterIdentity {
    /// Lower-case, since it's compared as text.
    pub salt: String,
    pub iterations: u32,
}

impl RouterIdentity {
    pub fn new(salt: &str, iterations: u32) -> Self {
        Self {
            salt: salt.to_ascii_lowercase(),
            iterations,
        }
    }
}

/// Whether a login may go on to send its proof, once the driver has seen the router's identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoginGate {
    Open,
    /// Only to the router recorded at the last verified login.
    Expect(RouterIdentity),
    /// Nothing is recorded and the user hasn't asked for this login.
    Ask,
}

impl LoginGate {
    pub fn check(&self, seen: &RouterIdentity) -> Result<(), RouterError> {
        match self {
            Self::Open => Ok(()),
            Self::Expect(known) if known == seen => Ok(()),
            Self::Expect(_) => Err(RouterError::UnrecognizedRouter { first_time: false }),
            Self::Ask => Err(RouterError::UnrecognizedRouter { first_time: true }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverInfo {
    pub id: &'static str,
    pub name: &'static str,
}

/// Exact request that `apply_net_config` would send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyPreview {
    pub fields: Vec<(String, String)>,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    LoggedIn,
    LoggedOut,
    Locked { wait_s: u32 },
}

#[async_trait]
pub trait RouterDriver: Send + Sync {
    fn info(&self) -> DriverInfo;
    fn apply_preview(&self, cfg: &NetConfig) -> ApplyPreview;
    async fn login(&self, creds: &Credentials) -> Result<(), RouterError>;
    /// `login`, stopping before the proof when `gate` refuses the router it finds. Returns the
    /// identity once the server signature has verified, and `None` when no proof was exchanged
    /// (already logged in) or the driver has no such check.
    async fn login_gated(
        &self,
        creds: &Credentials,
        _gate: &LoginGate,
    ) -> Result<Option<RouterIdentity>, RouterError> {
        self.login(creds).await.map(|()| None)
    }
    /// Ends the app's own session on the router. A session the router already dropped counts as
    /// logged out. Drivers without a logout call send nothing.
    async fn logout(&self) -> Result<(), RouterError> {
        Ok(())
    }
    /// Drops everything the driver holds for the session (cookies, tokens), so the next login
    /// starts from scratch. Sends nothing.
    fn forget_session(&self) {}
    async fn session_state(&self) -> Result<SessionState, RouterError>;
    async fn heartbeat(&self) -> Result<(), RouterError>;
    async fn capabilities(&self) -> Result<Capabilities, RouterError>;
    async fn device_info(&self) -> Result<DeviceInfo, RouterError>;
    async fn status(&self) -> Result<Status, RouterError>;
    async fn signal(&self) -> Result<Signal, RouterError>;
    async fn net_config(&self) -> Result<NetConfig, RouterError>;
    /// The WAN address. A read only; drivers that can't read it refuse.
    async fn wan_address(&self) -> Result<WanAddress, RouterError> {
        Err(RouterError::Validation {
            message: WAN_NOT_SUPPORTED.to_string(),
        })
    }
    /// Whether the mobile data switch is on. A read only; drivers without the switch refuse.
    async fn data_switch(&self) -> Result<bool, RouterError> {
        Err(RouterError::Validation {
            message: DATA_SWITCH_NOT_SUPPORTED.to_string(),
        })
    }
    /// Turns mobile data off or on. Only the IP refresh in `actor` calls this, with the
    /// `pending-data-switch.json` journal on disk before any "off". Drivers without the switch
    /// refuse and send nothing.
    async fn set_data_switch(&self, _on: bool) -> Result<(), RouterError> {
        Err(RouterError::Validation {
            message: DATA_SWITCH_NOT_SUPPORTED.to_string(),
        })
    }
    /// A manual network search, for Refresh IP (`IpMethod::NetworkSearch`). The router drops its
    /// data session while it searches and reconnects by itself; the call can take minutes.
    /// Only the IP refresh in `actor` calls this, from a task of its own. Drivers without one
    /// refuse and send nothing.
    async fn network_search(&self) -> Result<NetworkSearch, RouterError> {
        Err(RouterError::Validation {
            message: NETWORK_SEARCH_NOT_SUPPORTED.to_string(),
        })
    }
    /// The APN profiles and which one is in use. A read only; drivers that can't read them
    /// refuse. The profiles' passwords never leave the driver.
    async fn apn_profiles(&self) -> Result<crate::apn::ApnProfiles, RouterError> {
        Err(RouterError::Validation {
            message: crate::apn::APN_NOT_SUPPORTED.to_string(),
        })
    }
    /// Exact request that `set_default_apn` would send. Drivers that can't switch show none.
    fn apn_preview(&self, _target: &crate::apn::ApnTarget) -> ApplyPreview {
        ApplyPreview {
            fields: Vec::new(),
            body: String::new(),
        }
    }
    /// Makes `target` the profile the router dials with. Only an APN safe-apply `Post` calls
    /// this. A driver reads the profiles first and refuses, sending nothing, when the index is
    /// missing, the profile is incomplete or its fingerprint differs. Drivers that can't switch
    /// refuse.
    async fn set_default_apn(&self, _target: &crate::apn::ApnTarget) -> Result<(), RouterError> {
        Err(RouterError::Validation {
            message: crate::apn::APN_SWITCH_NOT_SUPPORTED.to_string(),
        })
    }
    /// Adds a profile and returns its index. Like the edit and delete below, a driver reads the
    /// profiles first and sends nothing unless the profile in use is still `expect_current`
    /// (`apn::check_write`), then reads again after its POST and refuses with
    /// `APN_ROUTER_SWITCHED` when the router now dials with another profile. The draft's
    /// password is in the request body only. Drivers that can't edit refuse.
    async fn add_apn_profile(
        &self,
        _expect_current: &crate::apn::ApnTarget,
        _draft: &crate::apn::ApnDraft,
    ) -> Result<u32, RouterError> {
        Err(RouterError::Validation {
            message: crate::apn::APN_EDIT_NOT_SUPPORTED.to_string(),
        })
    }
    /// Rewrites profile `target`, which must not be in use or the operator's. Tags the draft
    /// doesn't set are sent back as the router had them. `PasswordChange::Keep` copies the
    /// router's own saved password inside this call, and refuses when it reads empty or as
    /// stars.
    async fn edit_apn_profile(
        &self,
        _expect_current: &crate::apn::ApnTarget,
        _target: &crate::apn::ApnTarget,
        _draft: &crate::apn::ApnDraft,
    ) -> Result<(), RouterError> {
        Err(RouterError::Validation {
            message: crate::apn::APN_EDIT_NOT_SUPPORTED.to_string(),
        })
    }
    /// Deletes profile `target`, which must not be in use or the operator's.
    async fn delete_apn_profile(
        &self,
        _expect_current: &crate::apn::ApnTarget,
        _target: &crate::apn::ApnTarget,
    ) -> Result<(), RouterError> {
        Err(RouterError::Validation {
            message: crate::apn::APN_EDIT_NOT_SUPPORTED.to_string(),
        })
    }
    /// The provider's remote-management (TR-069) settings, for the privacy check. A read only,
    /// sent when the user asks; the answer can hold the provider's account for this router, so
    /// a driver parses it and drops it. Drivers without it refuse and send nothing.
    async fn remote_management(
        &self,
    ) -> Result<crate::privacy::Seen<crate::privacy::RemoteManagement>, RouterError> {
        Err(RouterError::Validation {
            message: crate::privacy::REMOTE_MANAGEMENT_NOT_SUPPORTED.to_string(),
        })
    }
    /// Every device the router remembers, online or not, for the privacy check. A read only,
    /// sent when the user asks, and never part of `diagnostic_reads`. Drivers without it refuse
    /// and send nothing.
    async fn lan_devices(
        &self,
    ) -> Result<crate::privacy::Seen<Vec<crate::privacy::LanDevice>>, RouterError> {
        Err(RouterError::Validation {
            message: crate::privacy::DEVICES_NOT_SUPPORTED.to_string(),
        })
    }
    /// Only the devices on Wi-Fi right now, which `privacy::check` asks for when the router
    /// won't give `lan_devices`. Same rules.
    async fn wifi_devices(
        &self,
    ) -> Result<crate::privacy::Seen<Vec<crate::privacy::LanDevice>>, RouterError> {
        Err(RouterError::Validation {
            message: crate::privacy::DEVICES_NOT_SUPPORTED.to_string(),
        })
    }
    async fn apply_net_config(&self, cfg: &NetConfig) -> Result<(), RouterError>;
    async fn reboot(&self) -> Result<(), RouterError>;
    /// Erases the router's settings. Only a driver whose call is confirmed for its model overrides
    /// this and sets `Capabilities::factory_reset_verified`; every other driver sends nothing.
    async fn factory_reset(&self) -> Result<(), RouterError> {
        Err(RouterError::Validation {
            message: FACTORY_RESET_NOT_VERIFIED.to_string(),
        })
    }
    /// Raw responses of the read-only endpoints a driver author needs, for the redacted
    /// diagnostics export. GETs only: this must never change anything on the router.
    async fn diagnostic_reads(&self) -> Vec<crate::diagnostics::EndpointRead> {
        Vec::new()
    }
    /// One file of the router's own web UI (`/html/…`, `/js/…`, `/lib/…`) as raw bytes, for the
    /// read-only "Save router page scripts" (`page_scripts::collect`). A GET with the app's
    /// session and nothing else: a driver checks `page_scripts::is_allowed_path` again and sends
    /// nothing for any other path. Drivers without it refuse.
    async fn web_page(&self, _path: &str) -> Result<Vec<u8>, RouterError> {
        Err(RouterError::Validation {
            message: crate::page_scripts::PAGES_NOT_SUPPORTED.to_string(),
        })
    }
}
