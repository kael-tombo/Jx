//! jx-maven — the Maven repository client and POM model (M2).
//!
//! Modules:
//! - [`version`]: Maven `ComparableVersion` ordering, ported from source and
//!   verified against Maven's own ordering tables.
//! - [`coords`]: `group:artifact:version[:classifier[:extension]]` and the
//!   repository layout paths it maps to.
//! - [`xml`]: POM and `maven-metadata.xml` parsing (data-only models).
//! - [`client`]: the repository client — mirrors, proxies, auth refs,
//!   checksum verification, content cache and parallel fetching.
//!
//! See `docs/decisions/0003-repository-client-and-pom-model.md`.

pub mod client;
pub mod coords;
pub mod version;
pub mod xml;

pub use client::{Client, FetchOutcome};
pub use coords::Coordinate;
pub use version::ComparableVersion;
