// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::collections::BTreeSet;

use regex::RegexBuilder;

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
    selected: BTreeSet<String>,
    pattern: String,
}

impl Filter {
    pub(crate) fn new(values: Vec<String>) -> Self {
        let selected = values.iter().cloned().collect();
        Self {
            values,
            selected,
            pattern: String::new(),
        }
    }

    pub(crate) fn values(&self) -> &[String] {
        &self.values
    }

    pub(crate) fn is_all(&self) -> bool {
        self.selected.len() == self.values.len()
    }

    pub(crate) fn is_selected(&self, value: &str) -> bool {
        self.selected.contains(value)
    }

    pub(crate) fn set_selected(&mut self, value: &str, selected: bool) {
        if selected {
            self.selected.insert(value.to_string());
        } else {
            self.selected.remove(value);
        }
    }

    pub(crate) fn select_all(&mut self, selected: bool) {
        self.selected.clear();
        if selected {
            self.selected.extend(self.values.iter().cloned());
        }
    }

    pub(crate) fn select_only(&mut self, value: &str) {
        self.selected.clear();
        if self.values.iter().any(|candidate| candidate == value) {
            self.selected.insert(value.to_string());
        }
    }

    pub(crate) fn set_pattern(&mut self, pattern: String) {
        self.pattern = pattern;
    }

    pub(crate) fn clear_pattern(&mut self) {
        self.pattern.clear();
    }

    pub(crate) fn matches_pattern(&self) -> Result<Vec<&str>, regex::Error> {
        if self.pattern.is_empty() {
            return Ok(self.values.iter().map(String::as_str).collect());
        }
        let expression = RegexBuilder::new(&self.pattern)
            .case_insensitive(true)
            .build()?;
        Ok(self
            .values
            .iter()
            .map(String::as_str)
            .filter(|value| expression.is_match(value))
            .collect())
    }

    pub(crate) fn select_pattern_matches(&mut self) -> Result<usize, regex::Error> {
        let matches = self
            .matches_pattern()?
            .into_iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        let count = matches.len();
        self.selected = matches;
        Ok(count)
    }

    pub(crate) fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub(crate) fn selected_values(&self) -> impl Iterator<Item = &str> {
        self.values
            .iter()
            .filter(|value| self.selected.contains(value.as_str()))
            .map(String::as_str)
    }

    pub(crate) fn first_selected(&self) -> Option<&str> {
        self.values
            .iter()
            .find(|value| self.selected.contains(value.as_str()))
            .map(String::as_str)
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
    pub(crate) pe_mode: String,
    pub(crate) pe_measure: String,
    pub(crate) relationship_mode: String,
    pub(crate) relationship_measure: String,
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
}
