use std::ffi::OsString;

use dualpane_core::{Metadata, VPath};

/// Per-item conflict policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConflictPolicy {
    #[default]
    Ask,
    Overwrite,
    Skip,
    OverwriteIfNewer,
    OverwriteIfDifferentSize,
    Rename,
}

/// Data displayed by the future conflict dialog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conflict {
    pub source: VPath,
    pub destination: VPath,
    pub source_metadata: Metadata,
    pub destination_metadata: Metadata,
}

/// Resolved transfer behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConflictAction {
    /// Publish only if the destination is still absent.
    Create,
    Overwrite,
    Skip,
    Rename(VPath),
    OverwriteIfNewer,
}

/// One response from a conflict UI or headless policy callback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConflictDecision {
    pub action: ConflictAction,
    pub apply_to_all: bool,
}

/// Resolver output. `Pending` lets non-conflicting work continue while UI asks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConflictResolution {
    Resolved(ConflictAction),
    Pending(Box<Conflict>),
}

#[must_use]
pub fn resolve_conflict(
    conflict: Conflict,
    policy: ConflictPolicy,
    renamed_destination: impl FnOnce() -> VPath,
) -> ConflictResolution {
    let action = match policy {
        ConflictPolicy::Ask => return ConflictResolution::Pending(Box::new(conflict)),
        ConflictPolicy::Overwrite => ConflictAction::Overwrite,
        ConflictPolicy::Skip => ConflictAction::Skip,
        ConflictPolicy::OverwriteIfNewer => {
            if conflict.source_metadata.modified > conflict.destination_metadata.modified {
                ConflictAction::Overwrite
            } else {
                ConflictAction::Skip
            }
        }
        ConflictPolicy::OverwriteIfDifferentSize => {
            if conflict.source_metadata.size != conflict.destination_metadata.size {
                ConflictAction::Overwrite
            } else {
                ConflictAction::Skip
            }
        }
        ConflictPolicy::Rename => ConflictAction::Rename(renamed_destination()),
    };
    ConflictResolution::Resolved(action)
}

/// Finds a collision-free `name (N).ext` destination without lossy filename conversion.
#[must_use]
pub fn unique_renamed_path(destination: &VPath, mut exists: impl FnMut(&VPath) -> bool) -> VPath {
    let parent = destination.parent().unwrap_or_else(|| VPath::from("."));
    let native = destination.as_path();
    let stem = native.file_stem().unwrap_or_default();
    let extension = native.extension();
    for suffix in 2_u64.. {
        let mut name = OsString::from(stem);
        name.push(format!(" ({suffix})"));
        if let Some(extension) = extension {
            name.push(".");
            name.push(extension);
        }
        let candidate = parent.join_name(&name);
        if !exists(&candidate) {
            return candidate;
        }
    }
    unreachable!("u64 suffix space exhausted")
}

#[cfg(test)]
mod tests {
    use dualpane_core::{EntryKind, Metadata, Timestamp, VPath};
    use proptest::prelude::*;

    use super::{
        Conflict, ConflictAction, ConflictPolicy, ConflictResolution, resolve_conflict,
        unique_renamed_path,
    };

    #[test]
    fn rename_suffix_stays_before_the_extension() {
        let renamed = unique_renamed_path(&VPath::from("/tmp/archive.tar"), |_| false);
        assert_eq!(renamed.to_string(), "/tmp/archive (2).tar");
    }

    #[test]
    fn conditional_policies_compare_the_requested_attribute() {
        let conflict = Conflict {
            source: VPath::from("source"),
            destination: VPath::from("destination"),
            source_metadata: metadata(20, 200),
            destination_metadata: metadata(10, 100),
        };

        assert_eq!(
            resolve_conflict(
                conflict.clone(),
                ConflictPolicy::OverwriteIfNewer,
                || unreachable!(),
            ),
            ConflictResolution::Resolved(ConflictAction::Overwrite)
        );
        assert_eq!(
            resolve_conflict(
                conflict,
                ConflictPolicy::OverwriteIfDifferentSize,
                || unreachable!(),
            ),
            ConflictResolution::Resolved(ConflictAction::Overwrite)
        );
    }

    fn metadata(size: u64, modified: i64) -> Metadata {
        Metadata {
            kind: EntryKind::File,
            identity: None,
            size,
            allocated_size: size,
            modified: Some(Timestamp {
                seconds: modified,
                nanoseconds: 0,
            }),
            created: None,
            accessed: None,
            mode: None,
            owner: None,
            group: None,
            hard_links: None,
        }
    }

    proptest! {
        #[test]
        fn rename_suffix_always_terminates_for_a_finite_collision_prefix(collisions in 0_u16..500) {
            let mut seen = 0_u16;
            let renamed = unique_renamed_path(&VPath::from("/tmp/name.bin"), |_| {
                let exists = seen < collisions;
                seen = seen.saturating_add(1);
                exists
            });
            prop_assert!(renamed.file_name().is_some());
            prop_assert_eq!(seen, collisions.saturating_add(1));
        }
    }
}
