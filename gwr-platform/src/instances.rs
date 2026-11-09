// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! Platform-owned instances, retaining concrete access for detailed models.
//! `Model` stores any implementation of the shared interface; it does not imply
//! roofline fidelity or register that implementation with an engine.

use std::ops::Deref;
use std::rc::Rc;

use gwr_engine::types::SimError;
use gwr_models::cache::Cache;
use gwr_models::fabric::Fabric;
use gwr_models::memory::Memory;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::model::{CacheModel, DetailedFabricModel, FabricModel, MemoryModel};
use gwr_models::processing_element::ProcessingElement;
use gwr_models::processing_element::model::ProcessingElementModel;

macro_rules! instance {
    ($name:ident, $detailed:ty, $interface:ident) => {
        pub enum $name {
            Detailed(Rc<$detailed>),
            Model(Rc<dyn $interface>),
        }
        impl Deref for $name {
            type Target = dyn $interface;
            fn deref(&self) -> &Self::Target {
                match self {
                    Self::Detailed(model) => model.as_ref(),
                    Self::Model(model) => model.as_ref(),
                }
            }
        }
        impl From<Rc<$detailed>> for $name {
            fn from(model: Rc<$detailed>) -> Self {
                Self::Detailed(model)
            }
        }
        impl $name {
            pub fn detailed(&self) -> Result<&Rc<$detailed>, SimError> {
                match self {
                    Self::Detailed(model) => Ok(model),
                    Self::Model(_) => Err(SimError(format!(
                        "Model '{}' has no concrete detailed handle",
                        self.entity().name
                    ))),
                }
            }
        }
    };
}

instance!(
    ProcessingElementInstance,
    ProcessingElement,
    ProcessingElementModel
);
instance!(MemoryInstance, Memory<MemoryAccess>, MemoryModel);
instance!(CacheInstance, Cache<MemoryAccess>, CacheModel);

pub enum FabricInstance {
    Detailed(DetailedFabricModel),
    Model(Rc<dyn FabricModel>),
}
impl Deref for FabricInstance {
    type Target = dyn FabricModel;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Detailed(model) => model,
            Self::Model(model) => model.as_ref(),
        }
    }
}
impl From<Rc<dyn Fabric<MemoryAccess>>> for FabricInstance {
    fn from(model: Rc<dyn Fabric<MemoryAccess>>) -> Self {
        Self::Detailed(DetailedFabricModel(model))
    }
}
impl FabricInstance {
    pub fn detailed(&self) -> Result<&Rc<dyn Fabric<MemoryAccess>>, SimError> {
        match self {
            Self::Detailed(model) => Ok(&model.0),
            Self::Model(_) => Err(SimError(format!(
                "Model '{}' has no concrete detailed handle",
                self.entity().name
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gwr_engine::engine::Engine;
    use gwr_engine::port::{InPort, OutPort};
    use gwr_models::model::MemoryEndpoint;
    use gwr_models::test_helpers::{create_default_memory_map, create_read};
    use gwr_track::tracker::dev_null_tracker;

    use super::*;
    use crate::Platform;
    use crate::connect::connect_ports;
    use crate::types::PlatformConfig;

    #[test]
    fn platform_wires_erased_memory_and_reports_live_traffic() {
        let mut engine = Engine::new(&dev_null_tracker());
        let clock = engine.default_clock();
        let mut platform = Platform::from_string(
            &engine,
            &clock,
            "memory_maps: []\nmemories:\n  - name: ram\n    kind: hbm\n    base_address: 0\n    config:\n      capacity_bytes: 4096\n      delay_ticks: 2\n",
        )
        .unwrap();
        let memory = platform.memory("ram").unwrap().clone();
        platform.memories[0] = MemoryInstance::Model(memory.clone());
        assert!(platform.memory("ram").is_err());
        let mut rx = InPort::new(&engine, &clock, engine.top(), "client_rx");
        let tx = RefCell::new(OutPort::new(engine.top(), "client_tx"));
        {
            let client = MemoryEndpoint::detailed(|| rx.state(), |p| tx.borrow_mut().connect(p));
            client
                .connect_detailed(&platform.memory_model("ram").unwrap().endpoint())
                .unwrap();
        }
        let request = create_read(
            engine.top(),
            &Rc::new(create_default_memory_map()),
            64,
            0,
            1024,
            8,
        );
        let mut tx = tx.into_inner();
        engine.spawn(async move {
            tx.put(request)?.await;
            rx.get()?.await;
            Ok(())
        });
        engine.run().unwrap();
        assert_eq!(platform.memory_model("ram").unwrap().bytes_read(), 64);
        assert_eq!(memory.bytes_read(), 64);
    }

    #[test]
    fn platform_connector_resolves_erased_pe_and_memory() {
        let mut engine = Engine::new(&dev_null_tracker());
        let clock = engine.default_clock();
        let mut platform = Platform::from_string(
            &engine,
            &clock,
            "operation_classes: {binary: {operations: [add, mul, compare], arity: 2}}\nmemory_maps:\n  - name: mm\n    devices: [ram]\nprocessing_elements:\n  - name: pe\n    memory_map: mm\n    config: {compute: {engines: {vector: {capabilities: [{class: binary, ops_per_tick: 1}]}}}}\nmemories:\n  - name: ram\n    kind: hbm\n    base_address: 0\n    config:\n      capacity_bytes: 4096\n",
        )
        .unwrap();
        let pe = platform.pe("pe").unwrap().clone();
        let memory = platform.memory("ram").unwrap().clone();
        platform.processing_elements[0] = ProcessingElementInstance::Model(pe.clone());
        platform.memories[0] = MemoryInstance::Model(memory.clone());
        assert!(platform.pe("pe").is_err());
        assert!(platform.memory("ram").is_err());
        let cfg: PlatformConfig =
            yaml_serde::from_str("memory_maps: []\nconnections:\n  - connect: [mem.ram, pe.pe]\n")
                .unwrap();
        connect_ports(&engine, &clock, &platform, &cfg).unwrap();
        assert!(pe.port_rx().is_err());
        assert!(memory.port_rx().is_err());
        assert!(connect_ports(&engine, &clock, &platform, &cfg).is_err());
    }
}
