#[doc(hidden)]
pub mod errors;
#[doc(hidden)]
pub mod scram;
#[doc(hidden)]
pub mod session;
#[doc(hidden)]
pub mod xml;

mod driver;
pub use driver::HuaweiH122Driver;
pub(crate) use driver::{apply_fields, apply_inner_xml, set_default_fields};
#[doc(hidden)]
pub use driver::{DIAGNOSTIC_PATHS, PATH_PLMN_LIST, SEARCH_TIMEOUT};
