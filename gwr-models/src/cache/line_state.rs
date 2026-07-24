// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

#[allow(dead_code)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum LineState {
    #[default]
    Invalid,
    AllocatedShared,
    AllocatedExclusive,
    Shared,
    Exclusive,
    Modified,
}

impl LineState {
    pub(super) fn is_allocated(self) -> bool {
        matches!(self, Self::AllocatedShared | Self::AllocatedExclusive)
    }

    pub(super) fn is_evictable(self) -> bool {
        matches!(self, Self::Invalid | Self::Shared | Self::Exclusive)
    }

    pub(super) fn can_read_hit(self) -> bool {
        matches!(self, Self::Shared | Self::Exclusive | Self::Modified)
    }

    pub(super) fn can_write_hit(self) -> bool {
        matches!(self, Self::Exclusive | Self::Modified)
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "Invalid",
            Self::AllocatedShared => "AllocatedShared",
            Self::AllocatedExclusive => "AllocatedExclusive",
            Self::Shared => "Shared",
            Self::Exclusive => "Exclusive",
            Self::Modified => "Modified",
        }
    }
}

pub(super) trait LineStateTransition {
    const FROM: &'static [LineState];
    const TO: LineState;
}

pub(super) struct AllocateShared;
pub(super) struct AllocateExclusive;
pub(super) struct GrantShared;
pub(super) struct GrantExclusiveClean;
pub(super) struct GrantExclusiveModified;
pub(super) struct LocalWriteModified;
pub(super) struct InvalidateLine;

impl LineStateTransition for AllocateShared {
    const FROM: &'static [LineState] =
        &[LineState::Invalid, LineState::Shared, LineState::Exclusive];
    const TO: LineState = LineState::AllocatedShared;
}

impl LineStateTransition for AllocateExclusive {
    const FROM: &'static [LineState] =
        &[LineState::Invalid, LineState::Shared, LineState::Exclusive];
    const TO: LineState = LineState::AllocatedExclusive;
}

impl LineStateTransition for GrantShared {
    const FROM: &'static [LineState] = &[LineState::AllocatedShared];
    const TO: LineState = LineState::Shared;
}

impl LineStateTransition for GrantExclusiveClean {
    const FROM: &'static [LineState] = &[LineState::AllocatedShared, LineState::AllocatedExclusive];
    const TO: LineState = LineState::Exclusive;
}

impl LineStateTransition for GrantExclusiveModified {
    const FROM: &'static [LineState] = &[LineState::AllocatedShared, LineState::AllocatedExclusive];
    const TO: LineState = LineState::Modified;
}

impl LineStateTransition for LocalWriteModified {
    const FROM: &'static [LineState] = &[LineState::Exclusive, LineState::Modified];
    const TO: LineState = LineState::Modified;
}

impl LineStateTransition for InvalidateLine {
    const FROM: &'static [LineState] = &[
        LineState::AllocatedShared,
        LineState::AllocatedExclusive,
        LineState::Shared,
        LineState::Exclusive,
        LineState::Modified,
    ];
    const TO: LineState = LineState::Invalid;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::contents::CacheEntry;

    #[test]
    fn line_state_predicates_and_labels_cover_all_states() {
        assert!(!LineState::Invalid.is_allocated());
        assert!(LineState::AllocatedShared.is_allocated());
        assert!(LineState::AllocatedExclusive.is_allocated());
        assert!(!LineState::Shared.is_allocated());
        assert!(!LineState::Exclusive.is_allocated());
        assert!(!LineState::Modified.is_allocated());

        assert!(LineState::Invalid.is_evictable());
        assert!(!LineState::AllocatedShared.is_evictable());
        assert!(!LineState::AllocatedExclusive.is_evictable());
        assert!(LineState::Shared.is_evictable());
        assert!(LineState::Exclusive.is_evictable());
        assert!(!LineState::Modified.is_evictable());

        assert!(!LineState::Invalid.can_read_hit());
        assert!(!LineState::AllocatedShared.can_read_hit());
        assert!(!LineState::AllocatedExclusive.can_read_hit());
        assert!(LineState::Shared.can_read_hit());
        assert!(LineState::Exclusive.can_read_hit());
        assert!(LineState::Modified.can_read_hit());

        assert!(!LineState::Invalid.can_write_hit());
        assert!(!LineState::AllocatedShared.can_write_hit());
        assert!(!LineState::AllocatedExclusive.can_write_hit());
        assert!(!LineState::Shared.can_write_hit());
        assert!(LineState::Exclusive.can_write_hit());
        assert!(LineState::Modified.can_write_hit());

        assert_eq!(LineState::Invalid.as_str(), "Invalid");
        assert_eq!(LineState::AllocatedShared.as_str(), "AllocatedShared");
        assert_eq!(LineState::AllocatedExclusive.as_str(), "AllocatedExclusive");
        assert_eq!(LineState::Shared.as_str(), "Shared");
        assert_eq!(LineState::Exclusive.as_str(), "Exclusive");
        assert_eq!(LineState::Modified.as_str(), "Modified");
    }

    #[test]
    fn line_transition_types_accept_only_valid_source_states() {
        let mut entry = CacheEntry {
            line_state: LineState::Invalid,
            tag: 0,
        };

        assert!(!entry.apply::<GrantShared>());
        assert!(entry.apply::<AllocateShared>());
        assert_eq!(entry.line_state, LineState::AllocatedShared);

        assert!(!entry.apply::<LocalWriteModified>());
        assert!(entry.apply::<GrantShared>());
        assert_eq!(entry.line_state, LineState::Shared);

        assert!(entry.apply::<AllocateExclusive>());
        assert_eq!(entry.line_state, LineState::AllocatedExclusive);
        assert!(entry.apply::<GrantExclusiveClean>());
        assert_eq!(entry.line_state, LineState::Exclusive);

        assert!(entry.apply::<LocalWriteModified>());
        assert_eq!(entry.line_state, LineState::Modified);

        assert!(entry.apply::<InvalidateLine>());
        assert_eq!(entry.line_state, LineState::Invalid);
    }
}
