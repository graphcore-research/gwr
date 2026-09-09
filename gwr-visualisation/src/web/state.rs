// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::{BTreeSet, HashSet};

#[cfg(all(feature = "web", target_arch = "wasm32"))]
use wasm_bindgen::{JsCast, JsValue};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum EntityKind {
    Layer,
    Pe,
    Memory,
    Tensor,
}

impl EntityKind {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "layer" => Some(Self::Layer),
            "pe" => Some(Self::Pe),
            "memory" => Some(Self::Memory),
            "tensor" => Some(Self::Tensor),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Layer => "layer",
            Self::Pe => "pe",
            Self::Memory => "memory",
            Self::Tensor => "tensor",
        }
    }
}

#[derive(Debug)]
pub(crate) struct Filter {
    values: Vec<String>,
    available: HashSet<String>,
    selection: Selection,
    pattern: String,
}

impl Filter {
    pub(crate) fn new(values: Vec<String>) -> Self {
        let available = values.iter().cloned().collect();
        Self {
            values,
            available,
            selection: Selection::All,
            pattern: String::new(),
        }
    }

    pub(crate) fn replace_values(&mut self, values: Vec<String>) -> Result<(), PatternError> {
        let available = values.iter().cloned().collect::<HashSet<_>>();
        match &mut self.selection {
            Selection::All => {}
            Selection::Only(selected) => {
                selected.retain(|value| available.contains(value));
            }
            Selection::Matching {
                pattern,
                values: selected,
            } => {
                *selected = matching_values(pattern, &values)?;
            }
        }
        self.available = available;
        self.values = values;
        Ok(())
    }

    pub(crate) fn values(&self) -> &[String] {
        &self.values
    }

    pub(crate) fn is_all(&self) -> bool {
        matches!(self.selection, Selection::All)
    }

    pub(crate) fn is_selected(&self, value: &str) -> bool {
        match &self.selection {
            Selection::All => self.available.contains(value),
            Selection::Only(selected)
            | Selection::Matching {
                values: selected, ..
            } => selected.contains(value),
        }
    }

    pub(crate) fn set_selected(&mut self, value: &str, selected: bool) {
        if !self.available.contains(value) {
            return;
        }
        if matches!(self.selection, Selection::Matching { .. }) {
            self.selection = Selection::Only(
                self.values
                    .iter()
                    .filter(|candidate| self.is_selected(candidate))
                    .cloned()
                    .collect(),
            );
        }
        if matches!(self.selection, Selection::All) && !selected {
            self.selection = Selection::Only(
                self.values
                    .iter()
                    .filter(|candidate| candidate.as_str() != value)
                    .cloned()
                    .collect(),
            );
            return;
        }
        if let Selection::Only(values) = &mut self.selection {
            if selected {
                values.insert(value.to_string());
            } else {
                values.remove(value);
            }
        }
    }

    pub(crate) fn select_all(&mut self, selected: bool) {
        self.selection = if selected {
            Selection::All
        } else {
            Selection::Only(BTreeSet::new())
        };
    }

    pub(crate) fn select_only(&mut self, value: &str) {
        self.selection = Selection::Only(
            self.values
                .iter()
                .find(|candidate| candidate.as_str() == value)
                .cloned()
                .into_iter()
                .collect(),
        );
    }

    pub(crate) fn set_pattern(&mut self, pattern: String) {
        self.pattern = pattern;
    }

    pub(crate) fn clear_pattern(&mut self) {
        self.pattern.clear();
    }

    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    pub(crate) fn matches_pattern(&self) -> Result<Vec<&str>, PatternError> {
        if self.pattern.is_empty() {
            return Ok(self.values.iter().map(String::as_str).collect());
        }
        let expression = Pattern::new(&self.pattern)?;
        Ok(self
            .values
            .iter()
            .map(String::as_str)
            .filter(|value| expression.is_match(value))
            .collect())
    }

    #[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
    pub(crate) fn select_pattern_matches(&mut self) -> Result<usize, PatternError> {
        let matches = matching_values(&self.pattern, &self.values)?;
        let count = matches.len();
        self.selection = Selection::Matching {
            pattern: self.pattern.clone(),
            values: matches,
        };
        Ok(count)
    }

    pub(crate) fn selected_count(&self) -> usize {
        match &self.selection {
            Selection::All => self.values.len(),
            Selection::Only(selected)
            | Selection::Matching {
                values: selected, ..
            } => selected.len(),
        }
    }

    pub(crate) fn selected_values(&self) -> impl Iterator<Item = &str> {
        self.values
            .iter()
            .filter(|value| self.is_selected(value))
            .map(String::as_str)
    }

    pub(crate) fn first_selected(&self) -> Option<&str> {
        self.values
            .iter()
            .find(|value| self.is_selected(value))
            .map(String::as_str)
    }
}

#[derive(Debug)]
enum Selection {
    All,
    Only(BTreeSet<String>),
    Matching {
        pattern: String,
        values: BTreeSet<String>,
    },
}

#[cfg(any(test, all(feature = "web", target_arch = "wasm32")))]
fn matching_values(pattern: &str, values: &[String]) -> Result<BTreeSet<String>, PatternError> {
    let expression = Pattern::new(pattern)?;
    Ok(values
        .iter()
        .filter(|value| expression.is_match(value))
        .cloned()
        .collect())
}

#[cfg(all(feature = "web", target_arch = "wasm32"))]
struct Pattern(js_sys::RegExp);

#[cfg(all(feature = "web", target_arch = "wasm32"))]
pub(crate) type PatternError = JsValue;

#[cfg(all(feature = "web", target_arch = "wasm32"))]
impl Pattern {
    fn new(pattern: &str) -> Result<Self, PatternError> {
        let constructor =
            js_sys::Function::new_with_args("pattern", "return new RegExp(pattern, 'i');");
        constructor
            .call1(&JsValue::UNDEFINED, &JsValue::from_str(pattern))
            .map(|value| Self(value.unchecked_into()))
    }

    fn is_match(&self, value: &str) -> bool {
        self.0.test(value)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
struct Pattern(regex::Regex);

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) type PatternError = regex::Error;

#[cfg(all(test, not(target_arch = "wasm32")))]
impl Pattern {
    fn new(pattern: &str) -> Result<Self, PatternError> {
        regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map(Self)
    }

    fn is_match(&self, value: &str) -> bool {
        self.0.is_match(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeMode {
    Grid,
    Chart,
}

impl PeMode {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "grid" => Some(Self::Grid),
            "chart" => Some(Self::Chart),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Grid => "grid",
            Self::Chart => "chart",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PeMeasure {
    MachineOps,
    ComputeNodes,
    MachineOperation(String),
    Data(TrafficMeasure),
    SelectedTensor(TrafficDirection),
    Overlay(String),
}

impl PeMeasure {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "compute:machine-ops" => Some(Self::MachineOps),
            "compute:compute-nodes" => Some(Self::ComputeNodes),
            "data:total" => Some(Self::Data(TrafficMeasure::Total)),
            "data:read" => Some(Self::Data(TrafficMeasure::Read)),
            "data:write" => Some(Self::Data(TrafficMeasure::Write)),
            "tensor:read" => Some(Self::SelectedTensor(TrafficDirection::Read)),
            "tensor:write" => Some(Self::SelectedTensor(TrafficDirection::Write)),
            value => value
                .strip_prefix("compute:machine-op:")
                .map(|name| Self::MachineOperation(name.to_string()))
                .or_else(|| {
                    value
                        .strip_prefix("metric:")
                        .map(|name| Self::Overlay(name.to_string()))
                }),
        }
    }

    pub(crate) fn name(&self) -> String {
        match self {
            Self::MachineOps => "compute:machine-ops".into(),
            Self::ComputeNodes => "compute:compute-nodes".into(),
            Self::MachineOperation(name) => format!("compute:machine-op:{name}"),
            Self::Data(measure) => format!("data:{}", measure.name()),
            Self::SelectedTensor(direction) => format!("tensor:{}", direction.name()),
            Self::Overlay(name) => format!("metric:{name}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RelationshipMode {
    Compute,
    LayerMemory,
    PeMemory,
    TensorMemory,
    TensorPe,
}

impl RelationshipMode {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "compute" => Some(Self::Compute),
            "memory" => Some(Self::LayerMemory),
            "pe-memory" => Some(Self::PeMemory),
            "tensor-memory" => Some(Self::TensorMemory),
            "tensor-pe" => Some(Self::TensorPe),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Compute => "compute",
            Self::LayerMemory => "memory",
            Self::PeMemory => "pe-memory",
            Self::TensorMemory => "tensor-memory",
            Self::TensorPe => "tensor-pe",
        }
    }

    pub(crate) fn needs_platform(self) -> bool {
        matches!(
            self,
            Self::LayerMemory | Self::PeMemory | Self::TensorMemory
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RelationshipMeasure {
    MachineOps,
    ComputeNodes,
    MachineOperation(String),
    Read,
    Write,
}

impl RelationshipMeasure {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "machine-ops" => Some(Self::MachineOps),
            "nodes" => Some(Self::ComputeNodes),
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "" => None,
            name => Some(Self::MachineOperation(name.to_string())),
        }
    }

    pub(crate) fn name(&self) -> &str {
        match self {
            Self::MachineOps => "machine-ops",
            Self::ComputeNodes => "nodes",
            Self::MachineOperation(name) => name,
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrafficDirection {
    Read,
    Write,
}

impl TrafficDirection {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrafficMeasure {
    Total,
    Read,
    Write,
}

impl TrafficMeasure {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Total => "total",
            Self::Read => "read",
            Self::Write => "write",
        }
    }
}

impl From<TrafficDirection> for TrafficMeasure {
    fn from(direction: TrafficDirection) -> Self {
        match direction {
            TrafficDirection::Read => Self::Read,
            TrafficDirection::Write => Self::Write,
        }
    }
}

#[derive(Debug)]
pub(crate) struct AppState {
    pub(crate) layers: Filter,
    pub(crate) pes: Filter,
    pub(crate) memories: Filter,
    pub(crate) tensors: Filter,
    pub(crate) selected_layer: Option<String>,
    pub(crate) selected_pe: Option<String>,
    pub(crate) selected_memory: Option<String>,
    pub(crate) selected_tensor: Option<String>,
    pub(crate) pe_mode: PeMode,
    pub(crate) pe_measure: PeMeasure,
    pub(crate) relationship_mode: RelationshipMode,
    pub(crate) relationship_measure: RelationshipMeasure,
    pub(crate) relationship_strength: u32,
    pub(crate) skip_memory_gaps: bool,
    pub(crate) generation: u64,
}

impl AppState {
    pub(crate) fn filter(&self, kind: EntityKind) -> &Filter {
        match kind {
            EntityKind::Layer => &self.layers,
            EntityKind::Pe => &self.pes,
            EntityKind::Memory => &self.memories,
            EntityKind::Tensor => &self.tensors,
        }
    }

    pub(crate) fn filter_mut(&mut self, kind: EntityKind) -> &mut Filter {
        match kind {
            EntityKind::Layer => &mut self.layers,
            EntityKind::Pe => &mut self.pes,
            EntityKind::Memory => &mut self.memories,
            EntityKind::Tensor => &mut self.tensors,
        }
    }

    pub(crate) fn select(&mut self, kind: EntityKind, value: String) {
        match kind {
            EntityKind::Layer => self.selected_layer = Some(value),
            EntityKind::Pe => self.selected_pe = Some(value),
            EntityKind::Memory => self.selected_memory = Some(value),
            EntityKind::Tensor => self.selected_tensor = Some(value),
        }
    }

    pub(crate) fn selected(&self, kind: EntityKind) -> Option<&str> {
        match kind {
            EntityKind::Layer => self.selected_layer.as_deref(),
            EntityKind::Pe => self.selected_pe.as_deref(),
            EntityKind::Memory => self.selected_memory.as_deref(),
            EntityKind::Tensor => self.selected_tensor.as_deref(),
        }
    }

    pub(crate) fn filters_changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::Filter;

    #[test]
    fn selects_case_insensitive_regular_expression_matches() {
        let mut filter = Filter::new(vec!["Layer 1".into(), "Layer 2".into(), "Other".into()]);
        filter.set_pattern("^layer".into());

        assert_eq!(filter.select_pattern_matches().unwrap(), 2);
        assert!(filter.is_selected("Layer 1"));
        assert!(!filter.is_selected("Other"));
    }

    #[test]
    fn rejects_invalid_regular_expressions_without_changing_selection() {
        let mut filter = Filter::new(vec!["Layer 1".into(), "Layer 2".into()]);
        filter.select_only("Layer 1");
        filter.set_pattern("[".into());

        assert!(filter.select_pattern_matches().is_err());
        assert!(filter.is_selected("Layer 1"));
        assert!(!filter.is_selected("Layer 2"));
    }

    #[test]
    fn preserves_source_order_when_finding_first_selection() {
        let mut filter = Filter::new(vec!["second".into(), "first".into()]);
        filter.select_only("first");

        assert_eq!(filter.first_selected(), Some("first"));
    }

    #[test]
    fn all_selection_includes_values_attached_later() {
        let mut filter = Filter::new(Vec::new());
        filter
            .replace_values(vec!["first".into(), "second".into()])
            .unwrap();

        assert!(filter.is_all());
        assert!(filter.is_selected("second"));
    }

    #[test]
    fn explicit_empty_selection_survives_values_attached_later() {
        let mut filter = Filter::new(Vec::new());
        filter.select_all(false);
        filter
            .replace_values(vec!["first".into(), "second".into()])
            .unwrap();

        assert!(!filter.is_all());
        assert_eq!(filter.selected_count(), 0);
    }

    #[test]
    fn pattern_selection_is_reapplied_to_values_attached_later() {
        let mut filter = Filter::new(Vec::new());
        filter.set_pattern("^layer [12]$".into());
        assert_eq!(filter.select_pattern_matches().unwrap(), 0);

        filter
            .replace_values(vec!["layer 1".into(), "layer 2".into(), "layer 3".into()])
            .unwrap();

        assert_eq!(filter.selected_count(), 2);
        assert!(filter.is_selected("layer 1"));
        assert!(!filter.is_selected("layer 3"));
    }

    #[test]
    fn manual_selection_replaces_a_pattern_selection() {
        let mut filter = Filter::new(vec!["layer 1".into(), "layer 2".into()]);
        filter.set_pattern("^layer 1$".into());
        filter.select_pattern_matches().unwrap();
        filter.set_selected("layer 2", true);

        filter
            .replace_values(vec!["layer 1".into(), "layer 2".into(), "layer 3".into()])
            .unwrap();

        assert!(filter.is_selected("layer 1"));
        assert!(filter.is_selected("layer 2"));
        assert!(!filter.is_selected("layer 3"));
    }
}
