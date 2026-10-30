//! JDK discovery, download, verification and installation.
//!
//! The provider layer is an extension point ([`provider::JdkProvider`]):
//! Eclipse Temurin is implemented today (verified endpoints recorded in
//! docs/decisions/0002-temurin-provider.md). Everything downstream — download,
//! checksum verification, safe extraction, the install layout under
//! `<JX_HOME>/jdks` — is provider-independent.
//!
//! jx never modifies a system Java installation; `java`/`JAVA_HOME` outside
//! jx stay untouched, and nothing here edits PATH.

// platform/arch helpers live in provider.rs (detect section)
pub mod extract;
pub mod provider;
pub mod store;

pub use store::{
    DEFAULT_FEATURE, InstalledJdk, default_home, ensure_jdk, installed_for_feature, jdks_dir,
    list_installed, remove_installed,
};
