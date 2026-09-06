//! Opaque identifiers. Paths are never used as identity because files move.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Creates a fresh random identifier.
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID.
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Returns the underlying UUID.
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0.simple())
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self)
            }
        }
    };
}

define_id!(
    /// Identifies a logical asset (file or directory) across renames and moves.
    AssetId
);
define_id!(
    /// Identifies a group of assets that should be organised together.
    BundleId
);
define_id!(
    /// Identifies an immutable proposal revision.
    RevisionId
);
define_id!(
    /// Identifies a scanning/review session rooted at one folder.
    SessionId
);
define_id!(
    /// Identifies a validated operation plan.
    PlanId
);
define_id!(
    /// Identifies an apply journal.
    JournalId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_serde_and_display() {
        let id = AssetId::new();
        let json = serde_json::to_string(&id).unwrap_or_default();
        let parsed: AssetId = serde_json::from_str(&json).unwrap_or_default();
        assert_eq!(id, parsed);
        let displayed = id.to_string();
        assert_eq!(displayed.len(), 32);
        assert_eq!(displayed.parse::<AssetId>().ok(), Some(id));
    }
}
