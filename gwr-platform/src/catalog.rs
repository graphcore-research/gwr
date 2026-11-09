// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Shared YAML catalogs for platform capabilities and timetable programs.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use gwr_engine::types::SimError;
use yaml_serde::{Mapping, Value};

pub fn load_yaml_with_includes(path: &Path, target: CatalogTarget) -> Result<String, SimError> {
    let mut loader = Loader {
        active: Vec::new(),
        visited: HashSet::new(),
        definitions: BTreeMap::new(),
    };
    let mut root = loader.load(path, true)?;
    for section in SECTIONS {
        root.remove(section);
    }
    for section in target.sections() {
        let mut entries = Mapping::new();
        for ((entry_section, name), (value, _)) in &loader.definitions {
            if entry_section == section {
                entries.insert(Value::String(name.clone()), value.clone());
            }
        }
        if !entries.is_empty() {
            root.insert(Value::String((*section).into()), Value::Mapping(entries));
        }
    }
    yaml_serde::to_string(&root)
        .map_err(|error| SimError(format!("Unable to serialize merged YAML: {error}")))
}

/// Convert one already resolved platform YAML value to complete JSON.
///
/// This performs no file reads or include resolution. Callers must pass the
/// held String returned by `load_yaml_with_includes`; use that same String
/// for the YAML snapshot and typed platform construction. String mapping
/// keys and finite numeric leaves are required; YAML tags are unsupported.
pub fn resolved_platform_json_from_yaml(
    resolved_yaml: &str,
) -> Result<serde_json::Value, SimError> {
    fn check(value: &Value) -> Result<(), SimError> {
        match value {
            Value::Mapping(map) => {
                for (key, value) in map {
                    if key.as_str().is_none() {
                        return Err(SimError(
                            "Resolved platform JSON requires string mapping keys".into(),
                        ));
                    }
                    check(value)?;
                }
            }
            Value::Sequence(values) => {
                for value in values {
                    check(value)?;
                }
            }
            Value::Number(number) => {
                if number.as_f64().is_some_and(|number| !number.is_finite()) {
                    return Err(SimError(
                        "Resolved platform JSON requires finite numeric values".into(),
                    ));
                }
            }
            Value::Tagged(_) => {
                return Err(SimError(
                    "Resolved platform JSON does not support YAML tags".into(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
    let value: Value = yaml_serde::from_str(resolved_yaml)
        .map_err(|error| SimError(format!("Invalid resolved platform YAML: {error}")))?;
    check(&value)?;
    serde_json::to_value(value).map_err(|error| {
        SimError(format!(
            "Unable to serialize resolved platform JSON: {error}"
        ))
    })
}

#[derive(Clone, Copy)]
pub enum CatalogTarget {
    Platform,
    Timetable,
}

impl CatalogTarget {
    fn sections(self) -> &'static [&'static str] {
        match self {
            Self::Platform => &SECTIONS[..2],
            Self::Timetable => &SECTIONS[2..3],
        }
    }
}

pub fn reject_string_includes(yaml: &str) -> Result<(), SimError> {
    let value: Value =
        yaml_serde::from_str(yaml).map_err(|error| SimError(format!("Invalid YAML: {error}")))?;
    if value
        .as_mapping()
        .is_some_and(|map| map.contains_key("includes"))
    {
        return Err(SimError(
            "YAML includes require a file path; use from_file instead of from_string".into(),
        ));
    }
    Ok(())
}

struct Loader {
    active: Vec<PathBuf>,
    visited: HashSet<PathBuf>,
    definitions: BTreeMap<(String, String), (Value, PathBuf)>,
}

impl Loader {
    fn load(&mut self, path: &Path, root: bool) -> Result<Mapping, SimError> {
        let path = path.canonicalize().map_err(|error| {
            SimError(format!(
                "Unable to resolve include {}: {error}",
                path.display()
            ))
        })?;
        if self.active.contains(&path) {
            let mut chain = self
                .active
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>();
            chain.push(path.display().to_string());
            return Err(SimError(format!("Include cycle: {}", chain.join(" -> "))));
        }
        if !self.visited.insert(path.clone()) {
            return Ok(Mapping::new());
        }
        let contents = std::fs::read_to_string(&path)
            .map_err(|error| SimError(format!("Unable to read {}: {error}", path.display())))?;
        let value: Value = yaml_serde::from_str(&contents)
            .map_err(|error| SimError(format!("Invalid YAML in {}: {error}", path.display())))?;
        let mut map = value
            .as_mapping()
            .cloned()
            .ok_or_else(|| SimError(format!("{} must contain a YAML mapping", path.display())))?;
        if !root {
            for key in map.keys() {
                let Some(key) = key.as_str() else {
                    return Err(SimError(format!(
                        "Catalog {} has a non-string key",
                        path.display()
                    )));
                };
                if key != "includes" && !SECTIONS.contains(&key) {
                    return Err(SimError(format!(
                        "Unknown catalog section '{key}' in {}",
                        path.display()
                    )));
                }
            }
        }
        self.active.push(path.clone());
        let includes = map.remove("includes");
        if let Some(includes) = includes {
            let sequence = includes.as_sequence().ok_or_else(|| {
                SimError(format!(
                    "includes in {} must be a list of paths",
                    path.display()
                ))
            })?;
            for entry in sequence {
                let relative = entry.as_str().ok_or_else(|| {
                    SimError(format!(
                        "includes in {} must be a list of paths",
                        path.display()
                    ))
                })?;
                self.load(
                    &path
                        .parent()
                        .expect("canonical file has parent")
                        .join(relative),
                    false,
                )?;
            }
        }
        for section in SECTIONS {
            if let Some(value) = map.get(section) {
                let entries = value.as_mapping().ok_or_else(|| {
                    SimError(format!("{section} in {} must be a mapping", path.display()))
                })?;
                for (name, definition) in entries {
                    let name = name.as_str().ok_or_else(|| {
                        SimError(format!(
                            "{section} in {} has a non-string name",
                            path.display()
                        ))
                    })?;
                    let key = (section.to_owned(), name.to_owned());
                    if let Some((_, previous)) = self.definitions.get(&key) {
                        return Err(SimError(format!(
                            "Duplicate {section} definition '{name}' in {} and {}",
                            previous.display(),
                            path.display()
                        )));
                    }
                    self.definitions
                        .insert(key, (definition.clone(), path.clone()));
                }
            }
        }
        self.active.pop();
        Ok(map)
    }
}

const SECTIONS: [&str; 4] = [
    "operation_classes",
    "compute_profiles",
    "programs",
    "operator_gaps",
];

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{CatalogTarget, load_yaml_with_includes, reject_string_includes};

    #[test]
    fn nested_includes_merge_by_section_and_deduplicate_files() {
        let dir = fixture(&[
            (
                "root.yaml",
                "includes: [sub/first.yaml, sub/second.yaml]\nprograms: {local: {inputs: [], outputs: []}}\nnodes: []\nedges: []\n",
            ),
            (
                "sub/first.yaml",
                "includes: [base.yaml]\nprograms: {first: {inputs: [], outputs: []}}\n",
            ),
            (
                "sub/second.yaml",
                "includes: [base.yaml]\nprograms: {second: {inputs: [], outputs: []}}\n",
            ),
            (
                "sub/base.yaml",
                "programs: {base: {inputs: [], outputs: []}}\noperation_classes: {add: {operations: [add], arity: 2}}\n",
            ),
        ]);
        let merged =
            load_yaml_with_includes(&dir.join("root.yaml"), CatalogTarget::Timetable).unwrap();
        let value: yaml_serde::Value = yaml_serde::from_str(&merged).unwrap();
        assert_eq!(value["programs"].as_mapping().unwrap().len(), 4);
        assert!(value.get("operation_classes").is_none());
    }

    #[test]
    fn cycles_missing_files_and_duplicate_names_are_errors() {
        let dir = fixture(&[
            ("cycle.yaml", "includes: [cycle.yaml]\n"),
            ("missing.yaml", "includes: [absent.yaml]\n"),
            (
                "duplicate.yaml",
                "includes: [a.yaml]\nprograms: {same: {}}\n",
            ),
            ("a.yaml", "programs: {same: {}}\n"),
        ]);
        for (name, expected) in [
            ("cycle.yaml", "Include cycle"),
            ("missing.yaml", "Unable to resolve include"),
            ("duplicate.yaml", "Duplicate programs definition 'same'"),
        ] {
            let error =
                load_yaml_with_includes(&dir.join(name), CatalogTarget::Timetable).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    fn fixture(files: &[(&str, &str)]) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "gwr-catalog-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        for (name, contents) in files {
            let path = directory.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        directory
    }

    #[test]
    fn string_loader_requires_file_for_includes() {
        assert!(
            reject_string_includes("includes: [a.yaml]\n")
                .unwrap_err()
                .to_string()
                .contains("from_file")
        );
    }
}
