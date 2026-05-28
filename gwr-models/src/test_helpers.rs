// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::fmt::Debug;
use std::rc::Rc;

#[doc(hidden)]
pub use gwr_components::build_component_harness;
use gwr_engine::types::AccessType;
use gwr_track::entity::Entity;

use crate::cache::CacheHintType;
use crate::cache::coherency_manager::CoherenceOp;
use crate::cache::traits::CoherentAccess;
use crate::memory::memory_access::MemoryAccess;
use crate::memory::memory_map::{DeviceId, MemoryMap};
use crate::memory::traits::AccessMemory;

/// Builds a simulation test harness for models that implement `AccessMemory`.
///
/// This macro uses the same harness DSL and generated API as
/// `gwr_components::build_component_harness!`, but requires `AccessMemory` and
/// checks TX expectations against `MemoryTxn`.
///
/// See the crate-level Testing documentation for the intended usage pattern,
/// generated API, and examples.
#[macro_export]
macro_rules! build_model_harness {
    (
        $(#[$meta:meta])*
        $vis:vis harness $harness:ident <$item:ident> {
            component: $component_field:ident : $component_ty:ty,
            $($sections:tt)*
        }
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$(#[$meta])*]
            [$vis]
            [$harness]
            [$item]

            [$component_field: $component_ty]
            []
            []
            []
            []
            $($sections)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
        rx ports: { $($rx_section:tt)* }, $($rest:tt)*
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            [$($rx_section)*] [$($tx_ports)*] [$($rx_arrays)*] [$($tx_arrays)*]
            $($rest)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
        rx ports: { $($rx_section:tt)* }
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            [$($rx_section)*] [$($tx_ports)*] [$($rx_arrays)*] [$($tx_arrays)*]
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
        tx ports: { $($tx_section:tt)* }, $($rest:tt)*
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            [$($rx_ports)*] [$($tx_section)*] [$($rx_arrays)*] [$($tx_arrays)*]
            $($rest)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
        tx ports: { $($tx_section:tt)* }
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            [$($rx_ports)*] [$($tx_section)*] [$($rx_arrays)*] [$($tx_arrays)*]
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
        rx port arrays: { $($rx_array_section:tt)* }, $($rest:tt)*
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            [$($rx_ports)*] [$($tx_ports)*] [$($rx_array_section)*] [$($tx_arrays)*]
            $($rest)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
        rx port arrays: { $($rx_array_section:tt)* }
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            [$($rx_ports)*] [$($tx_ports)*] [$($rx_array_section)*] [$($tx_arrays)*]
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
        tx port arrays: { $($tx_array_section:tt)* }, $($rest:tt)*
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            [$($rx_ports)*] [$($tx_ports)*] [$($rx_arrays)*] [$($tx_array_section)*]
            $($rest)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
        tx port arrays: { $($tx_array_section:tt)* }
    ) => {
        $crate::build_model_harness! {
            @normalize
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            [$($rx_ports)*] [$($tx_ports)*] [$($rx_arrays)*] [$($tx_array_section)*]
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_arrays:tt)*]
        [$($tx_arrays:tt)*]
    ) => {
        $crate::build_model_harness! {
            @impl
            [$($meta)*] [$vis] [$harness] [$item] [$component_field: $component_ty]
            rx ports: { $($rx_ports)* },
            tx ports: { $($tx_ports)* },
            rx port arrays: { $($rx_arrays)* },
            tx port arrays: { $($tx_arrays)* },
        }
    };

    (
        @impl
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$component_field:ident : $component_ty:ty]
        rx ports: { $($rx_variant:ident <$rx_ty:ty> => $rx_field:ident),* $(,)? },
        tx ports: { $($tx_variant:ident <$tx_ty:ty> => $tx_field:ident),* $(,)? },
        rx port arrays: {
            $($rx_array_variant:ident <$rx_array_ty:ty> => $rx_array_field:ident {
                count: $rx_array_count:ident
            }),* $(,)?
        },
        tx port arrays: {
            $($tx_array_variant:ident <$tx_array_ty:ty> => $tx_array_field:ident {
                count: $tx_array_count:ident
            }),* $(,)?
        } $(,)?
    ) => {
        $crate::test_helpers::build_component_harness! {
            @impl_model
            [$($meta)*]
            [$vis]
            [$harness]
            [$item]

            [$crate::test_helpers::MemoryTxn]
            [$crate::cache::traits::CoherentAccess]
            [$component_field: $component_ty]
            rx ports: {
                $(
                    $rx_variant <$rx_ty> => $rx_field
                ),*
            },
            tx ports: {
                $(
                    $tx_variant <$tx_ty> => $tx_field
                ),*
            },
            rx port arrays: {
                $(
                    $rx_array_variant <$rx_array_ty> => $rx_array_field {
                        count: $rx_array_count
                    }
                ),*
            },
            tx port arrays: {
                $(
                    $tx_array_variant <$tx_array_ty> => $tx_array_field {
                        count: $tx_array_count
                    }
                ),*
            },
        }
    };

}

#[derive(Clone, Debug)]
pub struct MemoryTxn {
    access_type: AccessType,
    dst_addr: u64,
    src_addr: Option<u64>,
    bytes: Option<usize>,
    total_bytes: Option<usize>,
    destination: Option<u64>,
    dst_device: Option<DeviceId>,
    src_device: Option<DeviceId>,

    // We use one Option to indicate whether to check the Option value which could be None
    #[expect(clippy::option_option)]
    coherence_op: Option<Option<CoherenceOp>>,
    cache_hint: Option<CacheHintType>,
}

impl MemoryTxn {
    #[must_use]
    pub fn new(access_type: AccessType, dst_addr: u64) -> Self {
        Self {
            access_type,
            dst_addr,
            src_addr: None,
            bytes: None,
            total_bytes: None,
            destination: None,
            dst_device: None,
            src_device: None,
            coherence_op: None,
            cache_hint: None,
        }
    }

    #[must_use]
    pub fn control(dst_addr: u64) -> Self {
        Self::new(AccessType::Control, dst_addr)
    }

    #[must_use]
    pub fn read_req(dst_addr: u64) -> Self {
        Self::new(AccessType::ReadRequest, dst_addr)
    }

    #[must_use]
    pub fn read_rsp(dst_addr: u64) -> Self {
        Self::new(AccessType::ReadResponse, dst_addr)
    }

    #[must_use]
    pub fn write_req(dst_addr: u64) -> Self {
        Self::new(AccessType::WriteRequest, dst_addr)
    }

    #[must_use]
    pub fn write_np_req(dst_addr: u64) -> Self {
        Self::new(AccessType::WriteNonPostedRequest, dst_addr)
    }

    #[must_use]
    pub fn write_np_rsp(dst_addr: u64) -> Self {
        Self::new(AccessType::WriteNonPostedResponse, dst_addr)
    }

    #[must_use]
    pub fn barrier_req(dst_addr: u64) -> Self {
        Self::new(AccessType::BarrierRequest, dst_addr)
    }

    #[must_use]
    pub fn barrier_rsp(dst_addr: u64) -> Self {
        Self::new(AccessType::BarrierResponse, dst_addr)
    }

    #[must_use]
    pub fn with_src_addr(mut self, src_addr: u64) -> Self {
        self.src_addr = Some(src_addr);
        self
    }

    #[must_use]
    pub fn with_bytes(mut self, bytes: usize) -> Self {
        self.bytes = Some(bytes);
        self
    }

    #[must_use]
    pub fn with_total_bytes(mut self, total_bytes: usize) -> Self {
        self.total_bytes = Some(total_bytes);
        self
    }

    #[must_use]
    pub fn with_destination(mut self, destination: u64) -> Self {
        self.destination = Some(destination);
        self
    }

    #[must_use]
    pub fn with_dst_device(mut self, dst_device: DeviceId) -> Self {
        self.dst_device = Some(dst_device);
        self
    }

    #[must_use]
    pub fn with_src_device(mut self, src_device: DeviceId) -> Self {
        self.src_device = Some(src_device);
        self
    }

    #[must_use]
    pub fn with_coherence_op(mut self, coherence_op: Option<CoherenceOp>) -> Self {
        self.coherence_op = Some(coherence_op);
        self
    }

    #[must_use]
    pub fn with_cache_hint(mut self, cache_hint: CacheHintType) -> Self {
        self.cache_hint = Some(cache_hint);
        self
    }
}

pub trait MemoryAccessMatcher<T>
where
    T: AccessMemory + Debug,
{
    fn assert_matches(&self, step: &str, actual: &T);
}

impl<T> MemoryAccessMatcher<T> for MemoryTxn
where
    T: AccessMemory + Debug + CoherentAccess,
{
    fn assert_matches(&self, check_id: &str, actual: &T) {
        assert_eq!(
            actual.access_type(),
            self.access_type,
            "{check_id}: access type mismatch for actual {actual:?}",
        );
        assert_eq!(
            actual.dst_addr(),
            self.dst_addr,
            "{check_id}: address mismatch for actual {actual:?}",
        );
        if let Some(src_addr) = self.src_addr {
            assert_eq!(
                actual.src_addr(),
                src_addr,
                "{check_id}: source address mismatch for actual {actual:?}",
            );
        }
        if let Some(bytes) = self.bytes {
            assert_eq!(
                actual.access_size_bytes(),
                bytes,
                "{check_id}: byte count mismatch for actual {actual:?}",
            );
        }
        if let Some(total_bytes) = self.total_bytes {
            assert_eq!(
                actual.total_bytes(),
                total_bytes,
                "{check_id}: total byte count mismatch for actual {actual:?}",
            );
        }
        if let Some(destination) = self.destination {
            assert_eq!(
                actual.destination(),
                destination,
                "{check_id}: destination mismatch for actual {actual:?}",
            );
        }
        if let Some(dst_device) = self.dst_device {
            assert_eq!(
                actual.dst_device(),
                dst_device,
                "{check_id}: dst device mismatch for actual {actual:?}",
            );
        }
        if let Some(src_device) = self.src_device {
            assert_eq!(
                actual.src_device(),
                src_device,
                "{check_id}: src device mismatch for actual {actual:?}",
            );
        }
        if let Some(coherence_op) = self.coherence_op {
            assert_eq!(
                actual.coherence_op(),
                coherence_op,
                "{check_id}: coherence op mismatch for actual {actual:?}",
            );
        }
        if let Some(cache_hint) = self.cache_hint {
            assert_eq!(
                actual.cache_hint(),
                cache_hint,
                "{check_id}: cache hint mismatch for actual {actual:?}",
            );
        }
    }
}

impl<T> MemoryAccessMatcher<T> for T
where
    T: AccessMemory + Debug + CoherentAccess,
{
    fn assert_matches(&self, check_id: &str, actual: &T) {
        MemoryTxn::new(self.access_type(), self.dst_addr())
            .with_src_addr(self.src_addr())
            .with_bytes(self.access_size_bytes())
            .with_total_bytes(self.total_bytes())
            .with_destination(self.destination())
            .with_dst_device(self.dst_device())
            .with_src_device(self.src_device())
            .with_coherence_op(self.coherence_op())
            .with_cache_hint(self.cache_hint())
            .assert_matches(check_id, actual);
    }
}

impl<T> gwr_components::test_helpers::ValueCheck<T> for MemoryTxn
where
    T: AccessMemory + Debug + CoherentAccess,
{
    fn assert_matches(&self, check_id: &str, actual: &T) {
        MemoryAccessMatcher::assert_matches(self, check_id, actual);
    }
}

impl gwr_components::test_helpers::ValueCheck<MemoryAccess> for MemoryAccess {
    fn assert_matches(&self, check_id: &str, actual: &MemoryAccess) {
        MemoryAccessMatcher::assert_matches(self, check_id, actual);
    }
}

#[must_use]
pub fn create_default_memory_map() -> MemoryMap {
    // Map all addresses to a single device ID.
    MemoryMap::from_regions(&[(0x0, u64::MAX, DeviceId(0))]).unwrap()
}

#[must_use]
pub fn create_read(
    created_by: &Rc<Entity>,
    memory_map: &Rc<MemoryMap>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    let (dst_device, _) = memory_map.lookup(dst_addr).unwrap();
    let (src_device, _) = memory_map.lookup(src_addr).unwrap();
    MemoryAccess::new(
        created_by,
        AccessType::ReadRequest,
        num_bytes,
        dst_addr,
        src_addr,
        dst_device,
        src_device,
        overhead_size_bytes,
    )
}

#[must_use]
pub fn create_write(
    created_by: &Rc<Entity>,
    memory_map: &Rc<MemoryMap>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    let (dst_device, _) = memory_map.lookup(dst_addr).unwrap();
    let (src_device, _) = memory_map.lookup(src_addr).unwrap();
    MemoryAccess::new(
        created_by,
        AccessType::WriteRequest,
        num_bytes,
        dst_addr,
        src_addr,
        dst_device,
        src_device,
        overhead_size_bytes,
    )
}

#[must_use]
pub fn create_write_np(
    created_by: &Rc<Entity>,
    memory_map: &Rc<MemoryMap>,
    num_bytes: usize,
    dst_addr: u64,
    src_addr: u64,
    overhead_size_bytes: usize,
) -> MemoryAccess {
    let (dst_device, _) = memory_map.lookup(dst_addr).unwrap();
    let (src_device, _) = memory_map.lookup(src_addr).unwrap();
    MemoryAccess::new(
        created_by,
        AccessType::WriteNonPostedRequest,
        num_bytes,
        dst_addr,
        src_addr,
        dst_device,
        src_device,
        overhead_size_bytes,
    )
}
