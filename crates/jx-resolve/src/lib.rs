//! The dependency-graph resolver (M3, ADR 0004).
//!
//! What it does: turn the manifest's dependency tables into the *concrete*
//! artifact set the build consumes — wanted prefixes expanded against the
//! repositories' own version listings, versionless entries filled from
//! platform (BOM) management, transitive POM dependencies walked nearest-
//! wins with every fork kept visible, `[overrides]` applied everywhere they
//! match.
//!
//! What it deliberately does not do: build classpaths (M4), serve from the
//! lockfile (the lock fast path is a CLI concern), or resolve snapshots
//! (rejected everywhere for V1 — see KNOWN_LIMITATIONS).

pub mod graph;
pub mod selection;

pub use graph::{
    Conflict, Edge, OverrideApplication, Resolution, Skipped, SolvedNode, node_key, resolve,
};
pub use selection::{select_latest_stable, select_version};
