// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Opt-in exact-word admission over an already validated ordinary timetable.
//! This sidecar is not dispatched by the ordinary ProgramSpec runner.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use gwr_engine::types::{SimError, SimResult};
use gwr_models::memory::atomic::{
    AtomicOperation, AtomicPath, AtomicServiceConfig, ControlEffect, ControlGroup, Effect,
};
use gwr_models::processing_element::tensors::dtype::DataType;
use gwr_models::processing_element::tensors::{Tensor, TensorView};
use serde::{Deserialize, Serialize};

use crate::TimetableGraph;
fn error(message: &str) -> SimError {
    SimError(message.into())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRegion {
    pub device: u64,
    pub base: u64,
    pub bytes: u64,
    pub path: AtomicPath,
    pub service: AtomicServiceConfig,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WordBinding {
    pub device: u64,
    pub address: u64,
    pub tensor: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectClaim {
    pub node: String,
    pub device: u64,
    pub address: u64,
    pub effect: Effect,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlLinks {
    pub group: ControlGroup,
    pub initialization: Vec<String>,
    pub publish: String,
    pub polls: Vec<String>,
    pub increments: Vec<String>,
    pub partials: Vec<String>,
    pub output_updates: Vec<Vec<String>>,
    pub reset_semaphore: String,
    pub reset_signal: String,
    pub final_join: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AtomicGraphSpec {
    pub memories: Vec<MemoryRegion>,
    pub bindings: Vec<WordBinding>,
    pub claims: Vec<EffectClaim>,
    #[serde(default)]
    pub groups: Vec<ControlLinks>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<StreamClaim>,
}
/// Private fields keep the exact validated claim set, not mutable caller data.
#[derive(Debug, Clone)]
pub struct AdmittedEffects {
    claims: Vec<EffectClaim>,
    groups: Vec<ControlLinks>,
    memories: Vec<MemoryRegion>,
    streams: Vec<AdmittedStream>,
}
impl AdmittedEffects {
    #[must_use]
    pub fn streams(&self) -> &[AdmittedStream] {
        &self.streams
    }
    #[must_use]
    pub fn memories(&self) -> &[MemoryRegion] {
        &self.memories
    }
    #[must_use]
    pub fn claims(&self) -> &[EffectClaim] {
        &self.claims
    }
    #[must_use]
    pub fn groups(&self) -> &[ControlLinks] {
        &self.groups
    }
}
#[expect(
    clippy::too_many_lines,
    reason = "Admission checks form one ordered certificate transaction before effects become visible."
)]
pub fn admit(
    graph: &TimetableGraph,
    memories: &[MemoryRegion],
    bindings: &[WordBinding],
    claims: &[EffectClaim],
    groups: &[ControlLinks],
) -> Result<AdmittedEffects, SimError> {
    // TimetableGraph cannot be constructed without the original access and
    // ProgramSpec validation. No ordinary read/write exception is introduced.
    let nodes: BTreeMap<_, _> = graph
        .nodes()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id(), i))
        .collect();
    let mut memory_ids = BTreeSet::new();
    for memory in memories {
        memory.path.validate()?;
        memory.service.validate()?;
        if !memory_ids.insert(memory.device)
            || memory.bytes == 0
            || memory.base.checked_add(memory.bytes - 1).is_none()
        {
            return Err(error("invalid/duplicate atomic memory region"));
        }
    }
    let mut words = BTreeMap::new();
    for binding in bindings {
        if binding.address % 4 != 0
            || binding.address.checked_add(3).is_none()
            || words
                .insert((binding.device, binding.address), binding)
                .is_some()
        {
            return Err(error("invalid/duplicate aligned word backing"));
        }
        let index = *nodes
            .get(binding.tensor.as_str())
            .ok_or_else(|| error("unknown word backing tensor"))?;
        let tensor = graph.nodes()[index]
            .tensor()
            .ok_or_else(|| error("word backing must be actual tensor"))?;
        if tensor.addr() != binding.address || tensor.num_bytes() != 4 {
            return Err(error("word backing must match complete actual4B tensor"));
        }
        let memory = memories
            .iter()
            .find(|m| m.device == binding.device)
            .ok_or_else(|| error("unknown word memory device"))?;
        if binding.address < memory.base || binding.address + 3 > memory.base + memory.bytes - 1 {
            return Err(error("word outside admitted memory"));
        }
    }
    let ordinary_connections = graph
        .edges()
        .iter()
        .filter_map(|e| e.tensor_connection())
        .collect::<Vec<_>>();
    let mut claimed_nodes = BTreeSet::new();
    for claim in claims {
        let node = *nodes
            .get(claim.node.as_str())
            .ok_or_else(|| error("unknown atomic effect node"))?;
        if !claimed_nodes.insert(claim.node.as_str()) {
            return Err(error("one explicit effect per task is required"));
        }
        let program = graph.nodes()[node]
            .program()
            .ok_or_else(|| error("typed effect must attach to compute barrier task"))?;
        if !program.inputs.is_empty()
            || !program.outputs.is_empty()
            || !program.assignments.is_empty()
        {
            return Err(error(
                "atomic effect must not duplicate ordinary program traffic/work",
            ));
        }
        let binding = words
            .get(&(claim.device, claim.address))
            .ok_or_else(|| error("effect target lacks exact word backing"))?;
        let tensor = graph.nodes()[*nodes.get(binding.tensor.as_str()).unwrap()]
            .tensor()
            .unwrap();
        let memory = memories.iter().find(|m| m.device == claim.device).unwrap();
        match &claim.effect {
            Effect::Atomic { word, .. } => {
                word.validate()?;
                if word.address != claim.address
                    || word.device != claim.device
                    || word.operation != AtomicOperation::Bf16PairFadd
                    || *tensor.dtype() != DataType::Bf16
                    || memory.service.bf16_pair_per_tick.is_none()
                {
                    return Err(error("BF16 pair backing/signature/capability mismatch"));
                }
            }
            Effect::InitializeOutput { .. } if *tensor.dtype() == DataType::Bf16 => {}
            Effect::Control { effect, .. } if *tensor.dtype() == DataType::Int32 => {
                if *effect == ControlEffect::FetchAddOne
                    && memory.service.i32_fetch_add_per_tick.is_none()
                {
                    return Err(error("missing Int32 fetch-add capability"));
                }
            }
            _ => {
                return Err(error(
                    "typed word dtype/effect mismatch; ordinary effects stay ordinary",
                ));
            }
        }
        // Real parent offsets/strides are checked via TensorView overlap, not
        // merely bounding boxes. Control words never exempt ordinary Data
        // edges.
        let target = TensorView::new_full(Tensor::new(&[4], &DataType::Int8, claim.address)?);
        for edge in &ordinary_connections {
            let control_word = groups.iter().any(|g| {
                g.group.device == claim.device
                    && (g.group.signal == claim.address || g.group.semaphore == claim.address)
            });
            if edge.view().first_overlapping_byte_ranges(&target).is_some()
                && (control_word
                    || edge.compute_node() == node
                    || !ordered(graph, node, edge.compute_node()))
            {
                return Err(error(
                    "ordinary/typed word overlap lacks actual dependency ordering",
                ));
            }
        }
    }
    let mut same_words: BTreeMap<u64, Vec<&EffectClaim>> = BTreeMap::new();
    for claim in claims {
        same_words.entry(claim.address).or_default().push(claim);
    }
    for same in same_words.values() {
        for (i, a) in same.iter().enumerate() {
            for b in &same[i + 1..] {
                if a.device != b.device {
                    return Err(error("same source address has ambiguous memory owner"));
                }
                let x = *nodes.get(a.node.as_str()).unwrap();
                let y = *nodes.get(b.node.as_str()).unwrap();
                if compatible_atomic(a, b) || compatible_control(a, b, groups) {
                    continue;
                }
                if !ordered(graph, x, y) {
                    return Err(error("incompatible unordered typed-word overlap"));
                }
            }
        }
    }
    let mut group_words = BTreeSet::new();
    let mut group_keys = BTreeSet::new();
    let mut covered = BTreeSet::new();
    let claims_by_node: BTreeMap<_, _> = claims.iter().map(|c| (c.node.as_str(), c)).collect();
    for links in groups {
        links.group.validate()?;
        if !group_keys.insert((links.group.invocation, links.group.generation)) {
            return Err(error("duplicate control group"));
        }
        for address in std::iter::once(links.group.signal)
            .chain(std::iter::once(links.group.semaphore))
            .chain(links.group.output_words.iter().copied())
        {
            if !group_words.insert((links.group.device, address)) {
                return Err(error("overlapping registered generation words"));
            }
        }
        validate_links(graph, &nodes, &claims_by_node, links, &mut covered)?;
    }
    for claim in claims {
        let group_effect = match &claim.effect {
            Effect::Atomic { word, .. } => groups.iter().any(|g| {
                g.group.invocation == word.invocation && g.group.generation == word.generation
            }),
            _ => true,
        };
        if group_effect && !covered.contains(claim.node.as_str()) {
            return Err(error(
                "control/init/output effect not in complete admitted group",
            ));
        }
    }
    Ok(AdmittedEffects {
        claims: claims.to_vec(),
        groups: groups.to_vec(),
        memories: memories.to_vec(),
        streams: Vec::new(),
    })
}
fn compatible_atomic(a: &EffectClaim, b: &EffectClaim) -> bool {
    match (&a.effect, &b.effect) {
        (Effect::Atomic { word: x, .. }, Effect::Atomic { word: y, .. }) => x == y,
        _ => false,
    }
}
fn compatible_control(a: &EffectClaim, b: &EffectClaim, groups: &[ControlLinks]) -> bool {
    let (
        Effect::Control {
            invocation: ai,
            generation: ag,
            ..
        },
        Effect::Control {
            invocation: bi,
            generation: bg,
            ..
        },
    ) = (&a.effect, &b.effect)
    else {
        return false;
    };
    ai == bi
        && ag == bg
        && groups.iter().any(|g| {
            g.group.invocation == *ai
                && g.group.generation == *ag
                && g.group.device == a.device
                && (a.address == g.group.signal || a.address == g.group.semaphore)
        })
}
fn reaches(graph: &TimetableGraph, from: usize, to: usize) -> bool {
    if from == to {
        return false;
    }
    let mut pending = VecDeque::from([from]);
    let mut seen = BTreeSet::from([from]);
    while let Some(i) = pending.pop_front() {
        for &next in graph.nodes()[i].successors() {
            if next == to {
                return true;
            }
            if seen.insert(next) {
                pending.push_back(next);
            }
        }
    }
    false
}
fn ordered(graph: &TimetableGraph, a: usize, b: usize) -> bool {
    reaches(graph, a, b) || reaches(graph, b, a)
}
#[expect(
    clippy::too_many_lines,
    reason = "Validate the complete arrival/wait/epoch protocol together before certifying its coverage."
)]
fn validate_links<'a>(
    graph: &TimetableGraph,
    nodes: &BTreeMap<&str, usize>,
    claims: &BTreeMap<&'a str, &'a EffectClaim>,
    links: &ControlLinks,
    covered: &mut BTreeSet<&'a str>,
) -> SimResult {
    let g = &links.group;
    let count = g.participants as usize;
    if links.initialization.len() != g.output_words.len()
        || links.polls.len() != count
        || links.increments.len() != count
        || links.partials.len() != count
        || links.output_updates.len() != count
    {
        return Err(error("control group role population mismatch"));
    }
    for id in links
        .partials
        .iter()
        .chain(std::iter::once(&links.final_join))
    {
        if !graph.nodes()[*nodes
            .get(id.as_str())
            .ok_or_else(|| error("unknown control work/join node"))?]
        .is_task()
        {
            return Err(error("partial and final join must be actual tasks"));
        }
    }
    let claim = |id: &str| {
        claims
            .get(id)
            .copied()
            .ok_or_else(|| error("control role has no actual effect task"))
    };
    let dependency = |from: &str, to: &str| -> SimResult {
        let a = *nodes
            .get(from)
            .ok_or_else(|| error("unknown control dependency task"))?;
        let b = *nodes
            .get(to)
            .ok_or_else(|| error("unknown control dependency task"))?;
        if !reaches(graph, a, b) {
            return Err(error("missing source control dependency path"));
        }
        Ok(())
    };
    let check = |id: &str, address: u64, expected: ControlEffect, participant: u32| -> SimResult {
        let c = claim(id)?;
        if c.device != g.device
            || c.address != address
            || c.effect
                != (Effect::Control {
                    invocation: g.invocation,
                    generation: g.generation,
                    participant,
                    effect: expected,
                })
        {
            return Err(error("control role target/participant/generation mismatch"));
        }
        Ok(())
    };
    for (id, address) in links.initialization.iter().zip(&g.output_words) {
        let c = claim(id)?;
        if c.device != g.device
            || c.address != *address
            || c.effect
                != (Effect::InitializeOutput {
                    invocation: g.invocation,
                    generation: g.generation,
                })
        {
            return Err(error("output initialization mismatch"));
        }
        dependency(id, &links.publish)?;
        covered.insert(c.node.as_str());
    }
    check(
        &links.publish,
        g.signal,
        ControlEffect::PublishSignalOne,
        g.publisher,
    )?;
    covered.insert(claim(&links.publish)?.node.as_str());
    // Resets use the reserved dynamic last-arrival marker, never a prescribed
    // winning source partition. Runtime substitutes actual returned last owner.
    check(
        &links.reset_semaphore,
        g.semaphore,
        ControlEffect::ResetSemaphore,
        u32::MAX,
    )?;
    check(
        &links.reset_signal,
        g.signal,
        ControlEffect::ResetSignal,
        u32::MAX,
    )?;
    covered.insert(claim(&links.reset_semaphore)?.node.as_str());
    covered.insert(claim(&links.reset_signal)?.node.as_str());
    dependency(&links.reset_semaphore, &links.reset_signal)?;
    dependency(&links.reset_signal, &links.final_join)?;
    for p in 0..count {
        check(
            &links.polls[p],
            g.signal,
            ControlEffect::PollSignal,
            p as u32,
        )?;
        check(
            &links.increments[p],
            g.semaphore,
            ControlEffect::FetchAddOne,
            p as u32,
        )?;
        dependency(&links.partials[p], &links.polls[p])?;
        dependency(&links.polls[p], &links.increments[p])?;
        dependency(&links.increments[p], &links.reset_semaphore)?;
        covered.insert(claim(&links.polls[p])?.node.as_str());
        covered.insert(claim(&links.increments[p])?.node.as_str());
        if links.output_updates[p].len() != g.output_words.len() {
            return Err(error("missing participant output words"));
        }
        for (id, address) in links.output_updates[p].iter().zip(&g.output_words) {
            let c = claim(id)?;
            let Effect::Atomic { word, participant } = &c.effect else {
                return Err(error("output must use actual pair atomic effect"));
            };
            if *participant != p as u32
                || word.device != g.device
                || word.address != *address
                || word.invocation != g.invocation
                || word.generation != g.generation
            {
                return Err(error("output atomic group mismatch"));
            }
            dependency(&links.increments[p], id)?;
            dependency(id, &links.final_join)?;
            // Initialization orders every target RMW, independently of the
            // dynamic signal visibility rule. Reset is not an output join.
            let init =
                &links.initialization[g.output_words.iter().position(|a| a == address).unwrap()];
            dependency(init, id)?;
            covered.insert(c.node.as_str());
        }
    }
    Ok(())
}

/// Original parent allocations and actual source block/participant. No global
/// partial tensor or numerical scratch is serialized as a Memory allocation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StreamClaim {
    pub node: String,
    pub a: String,
    pub b: String,
    pub c: String,
    pub invocation: u64,
    pub generation: u64,
    pub participant: usize,
    pub n_offset: usize,
    pub split: usize,
    pub execution: gwr_models::processing_element::compute::ExecutionMode,
}
#[derive(Clone, Debug)]
pub struct AdmittedStream {
    pub node: String,
    pub config: gwr_models::processing_element::task::StreamedMatrixTaskConfig,
    pub outputs: Vec<(String, usize)>,
}
impl AdmittedEffects {
    #[expect(
        clippy::too_many_lines,
        reason = "Stream admission checks share one evolving set of validated resources and claims."
    )]
    pub fn with_streams(
        mut self,
        graph: &TimetableGraph,
        streams: &[StreamClaim],
    ) -> Result<Self, SimError> {
        use gwr_models::processing_element::tensors::HasShape;
        let nodes: BTreeMap<_, _> = graph
            .nodes()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id(), i))
            .collect();
        let mut seen = BTreeSet::new();
        for stream in streams {
            let idx = *nodes
                .get(stream.node.as_str())
                .ok_or_else(|| error("unknown source stream node"))?;
            let node = &graph.nodes()[idx];
            let program = node
                .program()
                .ok_or_else(|| error("stream must be actual compute barrier"))?;
            if !seen.insert(stream.node.clone())
                || self.claims.iter().any(|c| c.node == stream.node)
                || !program.inputs.is_empty()
                || !program.outputs.is_empty()
                || !program.assignments.is_empty()
            {
                return Err(error("stream role duplicates ordinary/effect work"));
            }
            let tensor = |name: &str| -> Result<TensorView, SimError> {
                let i = *nodes
                    .get(name)
                    .ok_or_else(|| error("unknown full source parent"))?;
                let t = graph.nodes()[i]
                    .tensor()
                    .ok_or_else(|| error("source parent must be tensor"))?;
                Ok(TensorView::new_full(t.clone()))
            };
            let a = tensor(&stream.a)?;
            let b = tensor(&stream.b)?;
            let c = tensor(&stream.c)?;
            if a.shape().dims().len() != 2 || b.shape().dims().len() != 2 {
                return Err(error("source parent rank mismatch"));
            }
            let config = gwr_models::processing_element::task::StreamedMatrixTaskConfig {
                id: stream.node.clone(),
                m: a.shape().dims()[0],
                n: b.shape().dims()[0],
                k: a.shape().dims()[1],
                a,
                b,
                c,
                invocation: stream.invocation,
                generation: stream.generation,
                participant: stream.participant,
                n_offset: stream.n_offset,
                split: stream.split,
                execution: stream.execution.clone(),
            };
            config.validate()?;
            let links = self
                .groups
                .iter()
                .find(|g| {
                    (g.group.invocation, g.group.generation)
                        == (stream.invocation, stream.generation)
                })
                .ok_or_else(|| error("stream lacks admitted complete group"))?;
            if config.split != links.group.participants as usize
                || links.partials.get(stream.participant) != Some(&stream.node)
            {
                return Err(error("stream partial participant mismatch"));
            }
            let (base, n, n_offset) = (config.c.tensor().addr(), config.n, config.n_offset);
            let expected: Vec<_> = (0..config.m)
                .flat_map(|row| {
                    (0..64).step_by(2).map(move |col| {
                        (
                            row * 128 + col * 2,
                            base + ((row * n + n_offset + col) * 2) as u64,
                        )
                    })
                })
                .collect();
            if links.group.output_words
                != expected.iter().map(|(_, addr)| *addr).collect::<Vec<_>>()
            {
                return Err(error(
                    "source block output words differ from exact original C pairs",
                ));
            }
            let pe = node
                .pe()
                .ok_or_else(|| error("stream requires actual worker PE"))?;
            let mut outputs = Vec::new();
            for (id, (offset, _)) in links.output_updates[stream.participant]
                .iter()
                .zip(expected)
            {
                let output = graph.nodes()[*nodes.get(id.as_str()).unwrap()].pe();
                if output != Some(pe) {
                    return Err(error("local scratch output must stay on producer PE"));
                }
                outputs.push((id.clone(), offset));
            }
            self.streams.push(AdmittedStream {
                node: stream.node.clone(),
                config,
                outputs,
            });
        }
        // An opted-in streamed group cannot hide one symbolic empty partial
        // among genuine participants. All groups in this recipe are
        // complete source roles.
        if !streams.is_empty() {
            for (i, links) in self.groups.iter().enumerate() {
                for partial in &links.partials {
                    if !seen.contains(partial) {
                        return Err(error("source group missing actual streamed participant"));
                    }
                    if i > 0
                        && !reaches(
                            graph,
                            *nodes.get(self.groups[i - 1].final_join.as_str()).unwrap(),
                            *nodes.get(partial.as_str()).unwrap(),
                        )
                    {
                        return Err(error(
                            "source tiles require prior complete output ACK join before local storage reuse",
                        ));
                    }
                }
            }
        }
        Ok(self)
    }
}
