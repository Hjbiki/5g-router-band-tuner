use crate::ApiErrorKind;

/// Maps a Huawei `<error><code>` value to a coarse kind.
pub fn kind(code: u32) -> ApiErrorKind {
    match code {
        125001..=125003 => ApiErrorKind::Token,
        100003 => ApiErrorKind::NoRights,
        100004 => ApiErrorKind::Busy,
        100005 | 100006 => ApiErrorKind::BadParam,
        100002 => ApiErrorKind::NotSupported,
        108001 | 108002 | 108006 => ApiErrorKind::BadCredentials,
        108003 => ApiErrorKind::AlreadyLoggedIn,
        108007 => ApiErrorKind::TooManyAttempts,
        _ => ApiErrorKind::Unknown,
    }
}
