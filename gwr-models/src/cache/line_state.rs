// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

gwr_components::create_state_machine!(
    #[allow(dead_code)]
    pub(super) LineState {
        states: [
            Invalid,
            AllocatedShared,
            AllocatedExclusive,
            Shared,
            Exclusive,
            Modified,
        ],
        default: Invalid,
        transitions: [
            allocateShared: [Invalid, Shared, Exclusive] => AllocatedShared,
            allocateExclusive: [Invalid, Shared, Exclusive] => AllocatedExclusive,
            grantShared: [AllocatedShared] => Shared,
            grantExclusiveClean: [AllocatedShared, AllocatedExclusive] => Exclusive,
            grantExclusiveModified: [AllocatedShared, AllocatedExclusive] => Modified,
            localWriteModified: [Exclusive, Modified] => Modified,
            invalidateLine: [
                AllocatedShared,
                AllocatedExclusive,
                Shared,
                Exclusive,
                Modified,
            ] => Invalid,
        ],
    }
);

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
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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

        assert!(!entry.apply::<LineStateGrantSharedTransition>());
        assert!(entry.apply::<LineStateAllocateSharedTransition>());
        assert_eq!(entry.line_state, LineState::AllocatedShared);

        assert!(!entry.apply::<LineStateLocalWriteModifiedTransition>());
        assert!(entry.apply::<LineStateGrantSharedTransition>());
        assert_eq!(entry.line_state, LineState::Shared);

        assert!(entry.apply::<LineStateAllocateExclusiveTransition>());
        assert_eq!(entry.line_state, LineState::AllocatedExclusive);
        assert!(entry.apply::<LineStateGrantExclusiveCleanTransition>());
        assert_eq!(entry.line_state, LineState::Exclusive);

        assert!(entry.apply::<LineStateLocalWriteModifiedTransition>());
        assert_eq!(entry.line_state, LineState::Modified);

        assert!(entry.apply::<LineStateInvalidateLineTransition>());
        assert_eq!(entry.line_state, LineState::Invalid);
    }

    #[test]
    fn dump_line_state_mermaid_diagram() {
        let mermaid = LineState::mermaid();
        assert!(mermaid.contains("stateDiagram-v2"));
        assert!(mermaid.contains("[*] --> Invalid"));
        assert!(mermaid.contains("Invalid --> AllocatedShared: allocateShared"));
        assert!(mermaid.contains("AllocatedExclusive --> Modified: grantExclusiveModified"));
        assert!(mermaid.contains("Modified --> Invalid: invalidateLine"));

        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("gwr-models should live inside the workspace root");
        let output_dir = workspace_root.join("target/state_machines");
        let mermaid_path = output_dir.join("line_state.mmd");

        std::fs::create_dir_all(&output_dir)
            .expect("create state machine diagram output directory");

        std::fs::write(&mermaid_path, mermaid).expect("write LineState Mermaid diagram");
    }
}
