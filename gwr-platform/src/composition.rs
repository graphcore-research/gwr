// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Named platform definitions, checked native materialization and retained
//! hierarchy.
//!
//! Source graphs cache definitions, never hardware instances. Viewers can
//! traverse counted instances without materializing the simulator's flat device
//! topology.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gwr_engine::types::SimError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use yaml_serde::{Mapping, Value};

use crate::catalog::{CatalogTarget, load_catalog};
use crate::types::PlatformConfig;
mod repeat;
pub use repeat::ConnectionRepeat;

/// Maximum native objects expanded by the default file loader.
pub const DEFAULT_MATERIALIZATION_LIMIT: usize = 100_000;

/// Logical ownership level, independent of die/interposer geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    System,
    Rack,
    Board,
    Package,
    Component,
}

/// One reusable definition's named child or counted child array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instance {
    pub name: String,
    pub file: String,
    #[serde(default = "one")]
    #[serde(serialize_with = "decimal")]
    pub count: u64,
    #[serde(
        default,
        deserialize_with = "crate::types::parse_u64_byte_str",
        serialize_with = "decimal"
    )]
    pub address_base: u64,
    #[serde(
        default,
        deserialize_with = "crate::types::parse_u64_byte_str",
        serialize_with = "decimal"
    )]
    pub address_stride: u64,
    #[serde(default)]
    pub memory_map_bindings: BTreeMap<String, String>,
}
fn one() -> u64 {
    1
}
fn decimal<S: serde::Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&v.to_string())
}

/// Catalog-resolved native fragment plus its reusable children and public
/// ports.
#[derive(Debug, Clone, Serialize)]
pub struct Definition {
    pub name: String,
    pub level: Option<Level>,
    pub instances: Vec<Instance>,
    pub ports: BTreeMap<String, String>,
    pub native: Mapping,
    pub repeat_connections: Vec<ConnectionRepeat>,
}

/// Immutable snapshot of every transitive definition/catalog read by the
/// resolver.
#[derive(Debug, Serialize)]
pub struct SourceGraph {
    pub root: String,
    pub definitions: BTreeMap<String, Definition>,
    pub files: BTreeMap<String, String>,
}

/// Native YAML held alongside original source hierarchy.
#[derive(Debug)]
pub struct MaterializedPlatform {
    pub resolved_yaml: String,
    pub memory_windows: Vec<MemoryWindow>,
}
/// Local and relocated memory window, represented losslessly for JavaScript.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryWindow {
    pub name: String,
    pub local_base: String,
    pub global_base: String,
    pub capacity_bytes: String,
}

/// Read a platform hierarchy without expanding counted hardware instances.
pub fn load_source(path: &Path) -> Result<SourceGraph, SimError> {
    let mut graph = SourceGraph {
        root: String::new(),
        definitions: BTreeMap::new(),
        files: BTreeMap::new(),
    };
    graph.root = graph.load(path, &mut Vec::new())?;
    Ok(graph)
}
pub(crate) fn hash_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn err(message: impl Into<String>) -> SimError {
    SimError(message.into())
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, SimError> {
    yaml_serde::from_value(value).map_err(|e| err(e.to_string()))
}
fn identifier(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

impl SourceGraph {
    #[expect(
        clippy::too_many_lines,
        reason = "Keep import validation and active-path cleanup in one recursive transaction"
    )]
    fn load(&mut self, path: &Path, active: &mut Vec<PathBuf>) -> Result<String, SimError> {
        let path = path
            .canonicalize()
            .map_err(|e| err(format!("{}: {e}", path.display())))?;
        if active.len() >= 128 {
            return Err(err("Platform import nesting limit exceeded"));
        }
        if active.contains(&path) {
            return Err(err(format!(
                "Platform import cycle: {:?} -> {}",
                active,
                path.display()
            )));
        }
        let id = path.display().to_string();
        if self.definitions.contains_key(&id) {
            return Ok(id);
        }
        active.push(path.clone());
        let result = (|| {
            let (yaml, files) = load_catalog(&path, CatalogTarget::Platform)?;
            self.files.extend(files);
            let mut native: Mapping =
                decode(yaml_serde::from_str(&yaml).map_err(|e| err(e.to_string()))?)?;
            let name: String = native
                .remove("name")
                .map(decode)
                .transpose()?
                .unwrap_or_else(|| path.file_stem().unwrap().to_string_lossy().into_owned());
            let level: Option<Level> = native.remove("level").map(decode).transpose()?;
            let mut instances: Vec<Instance> = native
                .remove("instances")
                .map(decode)
                .transpose()?
                .unwrap_or_default();
            let ports = native
                .remove("ports")
                .map(decode)
                .transpose()?
                .unwrap_or_default();
            let repeat_connections: Vec<ConnectionRepeat> = native
                .remove("repeat_connections")
                .map(decode)
                .transpose()?
                .unwrap_or_default();
            for repeat in &repeat_connections {
                repeat.count()?;
            }
            native
                .entry(Value::String("memory_maps".into()))
                .or_insert(Value::Sequence(vec![]));
            // Native deserialization catches misspelled/unknown fields but
            // intentionally leaves reference and connection validation until
            // parent wiring is complete.
            if level.is_some() {
                let _: PlatformConfig = decode(Value::Mapping(native.clone()))?;
            }
            if !instances.is_empty() && level.is_none() {
                return Err(err("Named instances require an explicit level"));
            }
            if level.is_some_and(|v| v != Level::Package && v != Level::Component)
                && [
                    "processing_elements",
                    "caches",
                    "coherency_managers",
                    "memories",
                    "fabrics",
                ]
                .iter()
                .any(|k| {
                    native
                        .get(k)
                        .and_then(Value::as_sequence)
                        .is_some_and(|v| !v.is_empty())
                })
            {
                return Err(err("Hardware components must be owned by a package"));
            }
            let mut names = BTreeSet::new();
            for instance in &mut instances {
                if !identifier(&instance.name)
                    || instance.count == 0
                    || !names.insert(instance.name.clone())
                {
                    return Err(err(format!(
                        "Invalid/duplicate instance name or count: {}",
                        instance.name
                    )));
                }
                instance.file = self.load(&path.parent().unwrap().join(&instance.file), active)?;
                let child = &self.definitions[&instance.file];
                if !allowed(level, child.level) {
                    return Err(err(format!(
                        "Invalid containment {:?} -> {:?} for {}",
                        level, child.level, instance.name
                    )));
                }
            }
            if level == Some(Level::Package) {
                let mut pe = native
                    .get("processing_elements")
                    .and_then(Value::as_sequence)
                    .is_some_and(|s| !s.is_empty());
                let mut ddr = native
                    .get("memories")
                    .and_then(Value::as_sequence)
                    .into_iter()
                    .flatten()
                    .any(|m| m["kind"].as_str() == Some("ddr"));
                for i in &instances {
                    let child = &self.definitions[&i.file];
                    pe |= child
                        .native
                        .get("processing_elements")
                        .and_then(Value::as_sequence)
                        .is_some_and(|s| !s.is_empty());
                    ddr |= child
                        .native
                        .get("memories")
                        .and_then(Value::as_sequence)
                        .into_iter()
                        .flatten()
                        .any(|m| m["kind"].as_str() == Some("ddr"));
                }
                if pe && ddr {
                    return Err(err(
                        "DDR must live in a separate memory package from compute PEs",
                    ));
                }
            }
            self.definitions.insert(
                id.clone(),
                Definition {
                    name,
                    level,
                    instances,
                    ports,
                    native,
                    repeat_connections,
                },
            );
            Ok(id.clone())
        })();
        active.pop();
        result.map_err(|e| err(format!("{}: {e}", path.display())))
    }

    /// Export a compact browser snapshot. Addresses and large integers are
    /// decimal strings, so JavaScript never rounds a 64-bit memory location.
    pub fn browser_json(&self) -> Result<serde_json::Value, SimError> {
        fn lossless(value: &mut serde_json::Value) {
            match value {
                serde_json::Value::Object(m) => {
                    for v in m.values_mut() {
                        lossless(v);
                    }
                }
                serde_json::Value::Array(a) => {
                    for v in a {
                        lossless(v);
                    }
                }
                serde_json::Value::Number(n)
                    if n.as_u64().is_some_and(|n| n > 9_007_199_254_740_991) =>
                {
                    *value = n.to_string().into();
                }
                _ => {}
            }
        }
        let mut json = serde_json::to_value(self).map_err(|e| err(e.to_string()))?;
        for (id, definition) in &self.definitions {
            for (i, memory) in definition
                .native
                .get("memories")
                .and_then(Value::as_sequence)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let memory: crate::types::MemorySection = decode(memory.clone())?;
                let target = &mut json["definitions"][id]["native"]["memories"][i];
                target["base_address"] = memory.base_address.to_string().into();
                target["config"]["capacity_bytes"] =
                    memory.config.capacity_bytes.to_string().into();
            }
        }
        lossless(&mut json);
        Ok(json)
    }

    /// Expand a bounded native topology, resolving ports, defaults and
    /// addresses. Explicit physical hierarchies receive final native
    /// validation; legacy flat documents retain their existing parse/validation
    /// behavior.
    pub fn materialize(&self, limit: usize) -> Result<MaterializedPlatform, SimError> {
        let root = &self.definitions[&self.root];
        if root.level.is_none() && root.instances.is_empty() {
            return Ok(MaterializedPlatform {
                resolved_yaml: yaml_serde::to_string(&root.native)
                    .map_err(|e| err(e.to_string()))?,
                memory_windows: vec![],
            });
        }
        let mut sizes = BTreeMap::new();
        self.expansion_size(&self.root, limit as u64, &mut sizes)?;
        let mut out = Expansion::default();
        self.expand(&self.root, "", 0, &mut out)?;
        out.finish(self.definitions[&self.root].level.is_some())
    }

    fn expansion_size(
        &self,
        id: &str,
        limit: u64,
        sizes: &mut BTreeMap<String, (u64, bool)>,
    ) -> Result<(u64, bool), SimError> {
        if let Some(size) = sizes.get(id) {
            return Ok(*size);
        }
        let d = &self.definitions[id];
        let mut count = 1u64;
        for repeat in &d.repeat_connections {
            count = count
                .checked_add(repeat.count()?)
                .ok_or_else(|| err("Connection count overflow"))?;
        }
        let mut memory = false;
        for section in [
            "processing_elements",
            "caches",
            "coherency_managers",
            "fabrics",
            "memories",
            "memory_maps",
            "connections",
        ] {
            let n = d
                .native
                .get(section)
                .and_then(Value::as_sequence)
                .map_or(0, |s| s.len() as u64);
            count = count
                .checked_add(n)
                .ok_or_else(|| err("Materialization size overflow"))?;
            memory |= section == "memories" && n > 0;
        }
        for fabric in d
            .native
            .get("fabrics")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
        {
            let fabric: crate::types::FabricSection = decode(fabric.clone())?;
            let tiles = (fabric.columns as u64)
                .checked_mul(fabric.rows as u64)
                .and_then(|n| {
                    n.checked_mul(
                        fabric
                            .config
                            .fabric_ports_per_node
                            .unwrap_or(crate::builder::DEFAULT_FABRIC_PORTS_PER_NODE)
                            as u64,
                    )
                })
                .ok_or_else(|| err("Fabric size overflow"))?;
            count = count
                .checked_add(tiles)
                .ok_or_else(|| err("Materialization size overflow"))?;
        }
        for instance in &d.instances {
            let (n, has_memory) = self.expansion_size(&instance.file, limit, sizes)?;
            if has_memory && instance.count > 1 && instance.address_stride == 0 {
                return Err(err(format!(
                    "{}: repeated memory-bearing instance requires positive address_stride",
                    instance.name
                )));
            }
            count = n
                .checked_mul(instance.count)
                .and_then(|n| count.checked_add(n))
                .ok_or_else(|| err("Materialization size overflow"))?;
            memory |= has_memory;
        }
        if count > limit {
            return Err(err(format!(
                "Native materialization requires {count} objects, exceeds limit {limit}; inspect the compressed source hierarchy instead"
            )));
        }
        sizes.insert(id.into(), (count, memory));
        Ok((count, memory))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep the ordered namespace and address transformations in one traversal"
    )]
    fn expand(
        &self,
        id: &str,
        scope: &str,
        origin: u64,
        out: &mut Expansion,
    ) -> Result<(), SimError> {
        let d = &self.definitions[id];
        for instance in &d.instances {
            for i in 0..instance.count {
                let child = qualify(scope, &format!("{}[{i}]", instance.name));
                let origin = instance
                    .address_stride
                    .checked_mul(i)
                    .and_then(|n| n.checked_add(instance.address_base))
                    .and_then(|n| n.checked_add(origin))
                    .ok_or_else(|| err(format!("{child}: address origin overflow")))?;
                for (local, parent) in &instance.memory_map_bindings {
                    out.bindings
                        .insert(qualify(&child, local), qualify(scope, parent));
                }
                self.expand(&instance.file, &child, origin, out)
                    .map_err(|e| err(format!("{child} ({id}): {e}")))?;
            }
        }
        for (alias, endpoint) in &d.ports {
            if !identifier(alias) {
                return Err(err(format!("Invalid port alias {alias}")));
            }
            out.aliases
                .insert(qualify(scope, alias), (scope.into(), endpoint.clone()));
        }
        let defaults = d.native.get("defaults").and_then(Value::as_mapping);
        for (section, entries) in &d.native {
            let section = section
                .as_str()
                .ok_or_else(|| err("Platform section must be a string"))?;
            if section == "defaults" {
                continue;
            }
            if section == "connections" {
                for connection in entries.as_sequence().into_iter().flatten() {
                    let endpoints: crate::types::ConnectSection = decode(connection.clone())?;
                    out.connections.push((scope.into(), endpoints.connect));
                }
                continue;
            }
            if section == "operation_classes" || section == "compute_profiles" {
                for (name, config) in entries.as_mapping().into_iter().flatten() {
                    let name = string(name)?;
                    let mut config = config.clone();
                    if section == "compute_profiles" {
                        scope_classes(&mut config, scope)?;
                    }
                    out.native
                        .entry(Value::String(section.into()))
                        .or_insert(Value::Mapping(Mapping::new()))
                        .as_mapping_mut()
                        .unwrap()
                        .insert(Value::String(qualify(scope, name)), config);
                }
                continue;
            }
            for component in entries.as_sequence().into_iter().flatten() {
                let mut item = component
                    .as_mapping()
                    .cloned()
                    .ok_or_else(|| err(format!("Invalid {section} item")))?;
                let name = string(
                    item.get("name")
                        .ok_or_else(|| err("Missing component name"))?,
                )?
                .to_owned();
                if d.level.is_some() && !identifier(&name) {
                    return Err(err(format!("Invalid local component name {name}")));
                }
                item.insert(
                    Value::String("name".into()),
                    Value::String(qualify(scope, &name)),
                );
                match section {
                    "processing_elements" | "caches" => {
                        let mut config = defaults
                            .and_then(|m| {
                                m.get(if section == "caches" {
                                    "cache_config"
                                } else {
                                    "pe_config"
                                })
                            })
                            .and_then(Value::as_mapping)
                            .cloned()
                            .unwrap_or_default();
                        for (k, v) in item
                            .get("config")
                            .and_then(Value::as_mapping)
                            .into_iter()
                            .flatten()
                        {
                            if !v.is_null() {
                                config.insert(k.clone(), v.clone());
                            }
                        }
                        if section == "processing_elements" {
                            for key in ["memory_map", "dma_memory_map"] {
                                if let Some(v) = item.get_mut(key)
                                    && !v.is_null()
                                {
                                    *v = Value::String(qualify(scope, string(v)?));
                                }
                            }
                            if let Some(compute) = config.get_mut("compute") {
                                if let Some(name) = compute.as_str() {
                                    *compute = Value::String(qualify(scope, name));
                                } else {
                                    scope_classes(compute, scope)?;
                                }
                            }
                        }
                        if section == "caches"
                            && let Some(managers) = item.get_mut("coherency_managers")
                        {
                            for manager in managers
                                .as_sequence_mut()
                                .ok_or_else(|| err("coherency_managers must be a list"))?
                            {
                                *manager = Value::String(qualify(scope, string(manager)?));
                            }
                        }
                        item.insert(Value::String("config".into()), Value::Mapping(config));
                    }
                    "coherency_managers" => {
                        if let Some(map) = item.get_mut("memory_map") {
                            *map = Value::String(qualify(scope, string(map)?));
                        }
                    }
                    "memory_maps" => scope_devices(&mut item, scope)?,
                    "memories" => {
                        let m: crate::types::MemorySection = decode(Value::Mapping(item.clone()))?;
                        let address = origin
                            .checked_add(m.base_address)
                            .ok_or_else(|| err(format!("{scope}/{name}: address overflow")))?;
                        if u128::from(address) + u128::from(m.config.capacity_bytes)
                            > u128::from(u64::MAX) + 1
                        {
                            return Err(err(format!("{scope}/{name}: memory range overflow")));
                        }
                        out.windows.push(MemoryWindow {
                            name: qualify(scope, &name),
                            local_base: m.base_address.to_string(),
                            global_base: address.to_string(),
                            capacity_bytes: m.config.capacity_bytes.to_string(),
                        });
                        item.insert(
                            Value::String("base_address".into()),
                            Value::Number(address.into()),
                        );
                    }
                    _ => {}
                }
                out.native
                    .entry(Value::String(section.into()))
                    .or_insert(Value::Sequence(vec![]))
                    .as_sequence_mut()
                    .unwrap()
                    .push(Value::Mapping(item));
            }
        }
        for repeat in &d.repeat_connections {
            for endpoints in repeat.expand()? {
                out.connections.push((scope.into(), endpoints));
            }
        }
        Ok(())
    }
}
fn allowed(parent: Option<Level>, child: Option<Level>) -> bool {
    matches!(
        (parent, child),
        (Some(Level::System), Some(Level::Rack))
            | (Some(Level::Rack), Some(Level::Board))
            | (Some(Level::Board), Some(Level::Board | Level::Package))
            | (Some(Level::Package), Some(Level::Component))
    )
}

#[derive(Default)]
struct Expansion {
    native: Mapping,
    aliases: BTreeMap<String, (String, String)>,
    bindings: BTreeMap<String, String>,
    connections: Vec<(String, Vec<String>)>,
    windows: Vec<MemoryWindow>,
}
impl Expansion {
    fn finish(mut self, validate: bool) -> Result<MaterializedPlatform, SimError> {
        self.native
            .entry(Value::String("memory_maps".into()))
            .or_insert(Value::Sequence(vec![]));
        let maps: BTreeSet<_> = self.native["memory_maps"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_owned())
            .collect();
        for (key, value) in &self.bindings {
            if maps.contains(key) {
                return Err(err(format!(
                    "Cannot replace concrete child memory map {key}"
                )));
            }
            if !maps.contains(&self.bound_map(value)?) {
                return Err(err(format!("Unknown bound memory map {value}")));
            }
        }
        let mut pes = self.native.remove("processing_elements");
        if let Some(pes) = pes.as_mut() {
            for pe in pes.as_sequence_mut().unwrap() {
                for key in ["memory_map", "dma_memory_map"] {
                    if let Some(name) = pe.get(key).and_then(Value::as_str) {
                        let bound = self.bound_map(name)?;
                        pe[key] = Value::String(bound);
                    }
                }
            }
        }
        if let Some(pes) = pes {
            self.native
                .insert(Value::String("processing_elements".into()), pes);
        }
        let mut managers = self.native.remove("coherency_managers");
        if let Some(managers) = managers.as_mut() {
            for manager in managers.as_sequence_mut().unwrap() {
                let map = string(&manager["memory_map"])?;
                manager["memory_map"] = Value::String(self.bound_map(map)?);
            }
        }
        if let Some(managers) = managers {
            self.native
                .insert(Value::String("coherency_managers".into()), managers);
        }
        let mut connections = vec![];
        for (scope, endpoints) in &self.connections {
            let endpoints = endpoints
                .iter()
                .map(|s| self.endpoint(scope, s, &mut BTreeSet::new()))
                .collect::<Result<Vec<_>, _>>()?;
            let mut c = Mapping::new();
            c.insert(
                Value::String("connect".into()),
                Value::Sequence(endpoints.into_iter().map(Value::String).collect()),
            );
            connections.push(Value::Mapping(c));
        }
        if !connections.is_empty() {
            self.native.insert(
                Value::String("connections".into()),
                Value::Sequence(connections),
            );
        }
        let yaml = yaml_serde::to_string(&self.native).map_err(|e| err(e.to_string()))?;
        let cfg: PlatformConfig = yaml_serde::from_str(&yaml).map_err(|e| err(e.to_string()))?;
        if validate {
            cfg.validate()?;
            validate_request_paths(&cfg)?;
        }
        // Even unused aliases must designate real ports and must not form
        // cycles.
        for (scope, endpoint) in self.aliases.values() {
            let endpoint = self.endpoint(scope, endpoint, &mut BTreeSet::new())?;
            check_endpoint(&cfg, &endpoint)?;
        }
        Ok(MaterializedPlatform {
            resolved_yaml: yaml,
            memory_windows: self.windows,
        })
    }
    fn bound_map(&self, name: &str) -> Result<String, SimError> {
        let mut name = name;
        let mut visited = BTreeSet::new();
        while let Some(next) = self.bindings.get(name) {
            if !visited.insert(name) {
                return Err(err(format!("Memory map binding cycle at {name}")));
            }
            name = next;
        }
        Ok(name.into())
    }
    fn endpoint(
        &self,
        scope: &str,
        s: &str,
        active: &mut BTreeSet<String>,
    ) -> Result<String, SimError> {
        use crate::connection_id::{ConnectionEndpointId as E, parse_connection_endpoint_id};
        if s == "null" {
            return Ok(s.into());
        }
        let alias = qualify(scope, s);
        if active.len() >= 128 {
            return Err(err("Port alias nesting limit exceeded"));
        }
        if let Some((scope, endpoint)) = self.aliases.get(&alias) {
            if !active.insert(alias.clone()) {
                return Err(err(format!("Port alias cycle at {alias}")));
            }
            let result = self.endpoint(scope, endpoint, active);
            active.remove(&alias);
            return result;
        }
        let (path, endpoint) = s
            .rsplit_once('/')
            .map_or((scope.to_owned(), s), |(p, e)| (qualify(scope, p), e));
        let mut e = parse_connection_endpoint_id(endpoint)?;
        match &mut e {
            E::Pe { name }
            | E::Cache { name, .. }
            | E::Mem { name }
            | E::CoherencyManager { name } => {
                *name = qualify(&path, name);
            }
            E::FabricPort { fabric, .. } => *fabric = qualify(&path, fabric),
            E::Null => {}
        }
        Ok(e.to_string())
    }
}
fn string(value: &Value) -> Result<&str, SimError> {
    value
        .as_str()
        .ok_or_else(|| err("Expected a string reference"))
}
fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.into()
    } else {
        format!("{scope}/{name}")
    }
}
fn scope_classes(value: &mut Value, scope: &str) -> Result<(), SimError> {
    match value {
        Value::Mapping(m) => {
            for (k, v) in m {
                if k.as_str() == Some("class") {
                    *v = Value::String(qualify(scope, string(v)?));
                } else {
                    scope_classes(v, scope)?;
                }
            }
        }
        Value::Sequence(s) => {
            for v in s {
                scope_classes(v, scope)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn scope_devices(item: &mut Mapping, scope: &str) -> Result<(), SimError> {
    fn devices(value: Option<&mut Value>, scope: &str) -> Result<(), SimError> {
        if let Some(value) = value {
            for v in value
                .as_sequence_mut()
                .ok_or_else(|| err("devices must be a list"))?
            {
                *v = Value::String(qualify(scope, string(v)?));
            }
        }
        Ok(())
    }
    devices(item.get_mut("devices"), scope)?;
    if let Some(groups) = item
        .get_mut("address_mapping")
        .and_then(Value::as_mapping_mut)
        .and_then(|m| m.get_mut("groups"))
        .and_then(Value::as_sequence_mut)
    {
        for group in groups {
            devices(
                group.as_mapping_mut().and_then(|m| m.get_mut("devices")),
                scope,
            )?;
        }
    }
    Ok(())
}

fn check_endpoint(cfg: &PlatformConfig, endpoint: &str) -> Result<(), SimError> {
    use crate::connection_id::{ConnectionEndpointId as E, parse_connection_endpoint_id};
    let valid = match parse_connection_endpoint_id(endpoint)? {
        E::Pe { name } => cfg
            .processing_elements
            .iter()
            .flatten()
            .any(|v| v.name == name || name.strip_suffix(".dma") == Some(&v.name)),
        E::CoherencyManager { name } => cfg
            .coherency_managers
            .iter()
            .flatten()
            .any(|v| v.name == name),
        E::Cache { name, .. } => cfg.caches.iter().flatten().any(|v| v.name == name),
        E::Mem { name } => cfg.memories.iter().flatten().any(|v| v.name == name),
        E::FabricPort {
            fabric,
            column,
            row,
            port,
        } => cfg.fabrics.iter().flatten().any(|v| {
            v.name == fabric
                && column < v.columns
                && row < v.rows
                && port
                    < v.config
                        .fabric_ports_per_node
                        .unwrap_or(crate::builder::DEFAULT_FABRIC_PORTS_PER_NODE)
        }),
        E::Null => false,
    };
    if valid {
        Ok(())
    } else {
        Err(err(format!("Alias references unknown port {endpoint}")))
    }
}

fn validate_request_paths(cfg: &PlatformConfig) -> Result<(), SimError> {
    let graph = crate::memory_mapping::request_graph(cfg)?;
    for pe in cfg.processing_elements.iter().flatten() {
        for (endpoint, map) in [
            (format!("pe.{}", pe.name), Some(pe.memory_map.as_str())),
            (format!("pe.{}.dma", pe.name), pe.dma_memory_map.as_deref()),
        ] {
            let Some(map) = map else { continue };
            let map = cfg
                .memory_maps
                .iter()
                .find(|m| m.name == map)
                .ok_or_else(|| err(format!("Unknown map {map}")))?;
            let mut pending = vec![endpoint.clone()];
            let mut seen = BTreeSet::new();
            while let Some(node) = pending.pop() {
                if seen.insert(node.clone()) {
                    pending.extend(graph.get(&node).into_iter().flatten().cloned());
                }
            }
            for memory in &map.devices {
                if !seen.contains(&format!("mem.{memory}")) {
                    return Err(err(format!(
                        "{endpoint}: memory {memory} in map {} has no request path",
                        map.name
                    )));
                }
            }
        }
    }
    Ok(())
}
