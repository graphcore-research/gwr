// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Cache path segmentation. Each segment owns exactly its request/return wires.

use super::{
    BTreeSet, CachePortId, CacheResourceUses, CompiledRoute, ConnectionEndpointId, DemandKind,
    FabricAddressKey, Operation, ResourceKind, RooflineRoutes, SimError, ticks,
};

pub(crate) struct CacheStage {
    pub resources: CacheResourceUses,
}

pub(crate) struct CompiledReadPath {
    pub caches: Vec<CacheStage>,
    pub memory: CompiledRoute,
    pub write: CompiledRoute,
}

impl RooflineRoutes<'_> {
    pub(crate) fn compile_path_for_address(
        &self,
        pe: &str,
        memory: &str,
        offset: u64,
    ) -> Result<CompiledReadPath, SimError> {
        self.compile_path_with_key(pe, memory, Some(self.address_key(memory, offset)?))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "Keep cache-level route planning in execution order"
    )]
    fn compile_path_with_key(
        &self,
        pe: &str,
        memory: &str,
        key: Option<FabricAddressKey>,
    ) -> Result<CompiledReadPath, SimError> {
        let source_index = *self
            .indices
            .pes
            .get(pe)
            .ok_or_else(|| SimError(format!("Unknown PE '{pe}'")))?;
        let source = &self.cfg.processing_elements.as_ref().unwrap()[source_index];
        let memory_index = *self
            .indices
            .memories
            .get(memory)
            .ok_or_else(|| SimError(format!("Unknown memory '{memory}'")))?;
        if !self
            .cfg
            .memory_maps
            .iter()
            .any(|m| m.name == source.memory_map && m.devices.iter().any(|d| d == memory))
        {
            return Err(SimError("Invalid roofline cache path mapping".into()));
        }
        let blank = || CompiledRoute {
            operation: Operation::Read,
            uses: vec![],
            resource_count: self.resources.resources().len(),
            latency: 0,
            #[cfg(test)]
            decisions: Vec::new(),
        };
        let pe_endpoint = ConnectionEndpointId::Pe { name: pe.into() };
        let memory_endpoint = ConnectionEndpointId::Mem {
            name: memory.into(),
        };
        let mut start = pe_endpoint;
        let mut previous_cache: Option<String> = None;
        let mut seen = BTreeSet::new();
        let mut stages = Vec::new();
        let mut write = blank();
        write.operation = Operation::WriteNonPosted;
        loop {
            let mut request = blank();
            let mut response = blank();
            if let Some(parent) = &previous_cache {
                let index = self.indices.caches[parent];
                request.add_latency(ticks(self.effective.caches[index].delay_ticks())?)?;
                self.charge(
                    &mut response,
                    parent,
                    ResourceKind::CacheMemoryResponse,
                    DemandKind::ResponseWire,
                )?;
            }
            let target = self.walk_segment(
                &mut request,
                &start,
                &memory_endpoint,
                self.device_ids[pe].0,
                self.device_ids[memory].0,
                DemandKind::RequestWire,
                key,
            )?;
            self.walk(
                &mut response,
                &target,
                &start,
                self.device_ids[memory].0,
                self.device_ids[pe].0,
                DemandKind::ResponseWire,
                key,
            )?;
            if target == memory_endpoint {
                self.charge(
                    &mut request,
                    memory,
                    ResourceKind::MemoryPayload,
                    DemandKind::Payload,
                )?;
                request.add_latency(ticks(self.effective.memories[memory_index].delay_ticks())?)?;
                request.uses.extend(response.uses);
                #[cfg(test)]
                request.decisions.extend(response.decisions);
                request.add_latency(response.latency)?;
                write.uses.extend(request.uses.clone());
                #[cfg(test)]
                write.decisions.extend(request.decisions.clone());
                write.add_latency(request.latency)?;
                return Ok(CompiledReadPath {
                    caches: stages,
                    memory: request,
                    write,
                });
            }
            let ConnectionEndpointId::Cache {
                name,
                port: Some(CachePortId::Dev),
            } = target
            else {
                return Err(SimError("Roofline request entered wrong cache port".into()));
            };
            if !seen.insert(name.clone()) {
                return Err(SimError(format!("Cyclic roofline cache path at '{name}'")));
            }
            let index = self.indices.caches[&name];
            self.charge(
                &mut request,
                &name,
                ResourceKind::CacheRequest,
                DemandKind::RequestWire,
            )?;
            response.add_latency(ticks(self.effective.caches[index].delay_ticks())?)?;
            write.uses.extend(request.uses.clone());
            write.uses.extend(response.uses.clone());
            #[cfg(test)]
            write.decisions.extend(request.decisions.clone());
            #[cfg(test)]
            write.decisions.extend(response.decisions.clone());
            write.add_latency(request.latency)?;
            write.add_latency(response.latency)?;
            stages.push(CacheStage {
                resources: CacheResourceUses {
                    request: request.uses,
                    response: response.uses,
                    resource_count: self.resources.resources().len(),
                },
            });
            start = ConnectionEndpointId::Cache {
                name: name.clone(),
                port: Some(CachePortId::Mem),
            };
            previous_cache = Some(name);
        }
    }
}
