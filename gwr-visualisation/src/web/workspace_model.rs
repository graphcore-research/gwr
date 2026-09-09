// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

pub(crate) const VERSION: u8 = 1;

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Snapshot {
    pub(crate) version: u8,
    pub(crate) layout: String,
    pub(crate) visible: Vec<String>,
    pub(crate) order: Vec<String>,
    #[serde(default)]
    pub(crate) panels: BTreeMap<String, PanelSnapshot>,
}

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct PanelSnapshot {
    pub(crate) width: String,
    pub(crate) height: Option<String>,
    pub(crate) collapsed: bool,
}

impl Snapshot {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let snapshot: Self = serde_json::from_str(value).ok()?;
        (snapshot.version == VERSION).then_some(snapshot)
    }

    pub(crate) fn is_legacy_memory_layout(&self) -> bool {
        !self.panels.contains_key("memories-overview")
    }

    pub(crate) fn migrated_visible(&self) -> BTreeSet<String> {
        let mut visible = self
            .visible
            .iter()
            .map(|name| migrate_name(name).to_string())
            .collect::<BTreeSet<_>>();
        if self.is_legacy_memory_layout() && visible.contains("memory-summary") {
            visible.insert("memories-overview".into());
        }
        visible
    }

    pub(crate) fn panel_config(&self, name: &str) -> Option<&PanelSnapshot> {
        let legacy = (name == "pe-grid" && self.panels.contains_key("compute-balance"))
            .then_some("compute-balance");
        legacy
            .and_then(|legacy| self.panels.get(legacy))
            .or_else(|| self.panels.get(name))
    }
}

pub(crate) fn migrate_name(name: &str) -> &str {
    if name == "compute-balance" {
        "pe-grid"
    } else {
        name
    }
}

pub(crate) fn retained_focus<'a>(
    focused: Option<&'a str>,
    visible: &HashSet<String>,
) -> Option<&'a str> {
    focused.filter(|name| visible.contains(*name))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{Snapshot, retained_focus};

    #[test]
    fn migrates_legacy_workspace_names_and_memory_visibility() {
        let snapshot = Snapshot::parse(
            r#"{
                "version": 1,
                "layout": "two",
                "visible": ["compute-balance", "memory-summary"],
                "order": ["compute-balance", "memory-summary"],
                "panels": {
                    "compute-balance": {
                        "width": "2",
                        "height": "480px",
                        "collapsed": true
                    }
                }
            }"#,
        )
        .unwrap();

        let visible = snapshot.migrated_visible();
        assert!(visible.contains("pe-grid"));
        assert!(visible.contains("memories-overview"));
        assert_eq!(snapshot.panel_config("pe-grid").unwrap().width, "2");
    }

    #[test]
    fn rejects_unknown_workspace_versions() {
        assert!(Snapshot::parse(r#"{"version":2}"#).is_none());
    }

    #[test]
    fn clears_focus_when_a_preset_hides_the_panel() {
        let visible = HashSet::from(["timetable-summary".to_string()]);

        assert_eq!(retained_focus(Some("memory-details"), &visible), None);
        assert_eq!(
            retained_focus(Some("timetable-summary"), &visible),
            Some("timetable-summary")
        );
    }
}
