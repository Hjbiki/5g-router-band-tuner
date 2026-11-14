//! Router-independent domain model, errors and the driver trait.

pub mod actor;
pub mod apn;
pub mod bands;
pub mod capture;
pub mod diagnostics;
pub mod driver;
pub mod drivers;
pub mod error;
pub mod event_log;
pub mod event_text;
pub mod identity_file;
pub mod ip_refresh;
pub mod ip_refresh_file;
pub mod journal;
pub mod legacy;
pub mod logout_marker;
pub mod model;
pub mod page_scripts;
pub mod password_marker;
pub mod privacy;
pub mod probe;
pub mod safe_apply;
pub mod scan;
pub mod sequence_file;

pub use actor::{
    reattach_step1, spawn_actor, ActorConfig, ActorHandle, Alert, AppSnapshot, AutomationConfig,
    AutomationEvent, CellKey, Command, IpReattachOutcome, IpReattachResult, IpReattachView,
    IpRefreshAlert, LogoutOutcome, RebootTicket, RelockView, SessionView, WidenFallback,
    APN_CHANGE_RUNNING, APN_JOURNAL_BLOCKED, BANDS_NOT_READ, BAND_EDIT_NOT_ALLOWED,
    CHANGE_IN_PROGRESS, DATA_JOURNAL_BLOCKED, DATA_STILL_OFF, DATA_SWITCH_OFF, IP_NOT_IN_SERVICE,
    IP_REATTACH_COOLDOWN, IP_REATTACH_NO_LTE, IP_REATTACH_NO_OTHER_BANDS, IP_REATTACH_UNDO_OFF,
    IP_REFRESH_RUNNING, NOT_CONNECTED, PAGE_SCRIPTS_RUNNING, REBOOT_PENDING, SPEED_TEST_RUNNING,
};
pub use apn::{
    ApnApplyView, ApnAuth, ApnDraft, ApnIpType, ApnProfile, ApnProfiles, ApnSupport, ApnTarget,
    PasswordChange, Secret, APN_AUTO_ON, APN_CURRENT_MOVED, APN_EDIT_NOT_SUPPORTED,
    APN_NEEDS_ENCRYPTION, APN_NOT_SUPPORTED, APN_PASSWORD_HIDDEN, APN_PROFILES_UNKNOWN,
    APN_PROFILE_CHANGED, APN_PROFILE_INVALID, APN_PROFILE_IN_USE, APN_PROFILE_MISSING,
    APN_PROFILE_READ_ONLY, APN_ROUTER_SWITCHED, APN_SAVE_MISMATCH, APN_SWITCH_NOT_SUPPORTED,
    APN_TARGET_INVALID, APN_TIMERS,
};
pub use bands::lte::LteBandSet;
pub use bands::nr::NrBandInfo;
pub use bands::quality::{classify, Metric, Quality};
pub use bands::Duplex;
pub use driver::{
    ApplyPreview, Credentials, DriverInfo, LoginGate, NetworkSearch, RouterDriver, RouterIdentity,
    SessionState, WanAddress, DATA_SWITCH_NOT_SUPPORTED, FACTORY_RESET_NOT_VERIFIED,
    NETWORK_SEARCH_NOT_SUPPORTED, WAN_NOT_SUPPORTED,
};
pub use error::{ApiErrorKind, RouterError};
pub use event_log::{sanitize_detail, set_event_sink, EventLog, EventRecord, EventSink};
pub use event_text::{
    describe, group_changes, ChangeGroup, ChangeOutcome, EventCategory, EventEntry, EventItem,
    EventParam, Severity,
};
pub use ip_refresh::{
    IpRefreshOutcome, IpRefreshPhase, IpRefreshResult, IpRefreshView, IpTarget, IP_ALREADY_MATCHES,
    IP_NO_METHOD, IP_REFRESH_COOLDOWN, IP_TARGET_INVALID, IP_TRIES_INVALID, MAX_TRIES,
};
pub use journal::{Journal, JournalPhase, JournalRecord};
pub use model::{
    in_service, ipv4_range, wan_kind, Capabilities, ConnState, DeviceInfo, IpMethod, LteSignal,
    NetConfig, NetworkMode, NrOption, NrSignal, Operator, Rat, Signal, Status, WanKind, WanView,
};
pub use safe_apply::{
    ApplySequenceView, ChangeTarget, Effect, Evidence, ForeignJournal, Input, Phase, RevertReason,
    SafeApply, SafeApplyEvent, SafeApplyPolicyView, SafeApplyTimers, SafeApplyView, SequenceOrigin,
};
pub use sequence_file::{SequenceRecord, SequenceStore, SEQUENCE_VERSION};
