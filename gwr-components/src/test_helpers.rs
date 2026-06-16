// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cmp::min;
use std::collections::HashMap;
use std::rc::Rc;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::channel::oneshot::{Receiver, Sender};
use futures::future::select_all;
use gwr_engine::engine::Engine;
use gwr_engine::port::InPort;
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::SimObject;
use gwr_engine::types::SimResult;
use gwr_track::entity::{Entity, GetEntity};
#[doc(hidden)]
pub use paste::paste;

use crate::arbiter::Arbiter;
use crate::arbiter::policy::{Priority, PriorityRoundRobin};
use crate::flow_controls::limiter::Limiter;
use crate::source::Source;
use crate::store::Store;
use crate::{connect_port, option_box_repeat, rc_limiter};

#[derive(Clone)]
pub struct ArbiterInputData {
    pub val: usize,
    pub count: usize,
    pub weight: usize,
    pub priority: Priority,
}

pub fn check_round_robin(inputs: &[ArbiterInputData], data: &[usize]) {
    let total_count: usize = inputs.iter().map(|i| i.count).sum();
    assert_eq!(data.len(), total_count);

    let mut inputs = inputs.to_vec();
    let mut offset = 0;
    loop {
        // Determine the count for each input value in the next window. Note that this
        // copes with inputs producing the same value and inputs not producing
        // their full weight in the window.
        let mut expected_window_counts: HashMap<usize, usize> = HashMap::new();
        let mut window_length = 0;
        let max_priority = inputs
            .iter()
            .map(|i| {
                if i.count > 0 {
                    i.priority
                } else {
                    Priority::default()
                }
            })
            .max()
            .unwrap();
        for input in &mut inputs {
            let value_count = min(input.count, input.weight);
            if input.priority == max_priority && value_count > 0 {
                expected_window_counts
                    .entry(input.val)
                    .and_modify(|e| *e += value_count)
                    .or_insert(value_count);

                window_length += value_count;
                input.count -= value_count;
            }
        }
        if window_length == 0 {
            return;
        }

        let mut window_counts = HashMap::new();
        for value in data.iter().skip(offset).take(window_length) {
            window_counts
                .entry(*value)
                .and_modify(|e| *e += 1)
                .or_insert(1);
        }
        assert_eq!(window_counts, expected_window_counts);

        offset += window_length;
    }
}

pub fn priority_policy_test_core(engine: &mut Engine, inputs: &[ArbiterInputData]) {
    let clock = engine.default_clock();
    let num_inputs = inputs.len();
    let total_count = inputs.iter().map(|e| e.count).sum();
    let mut policy = PriorityRoundRobin::new(num_inputs);
    for (i, input) in inputs.iter().enumerate() {
        policy = policy.set_priority(i, input.priority);
    }

    let arbiter = Arbiter::new_and_register(
        engine,
        &clock,
        engine.top(),
        "arb",
        num_inputs,
        Box::new(policy),
    );
    let mut sources = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        sources.push(Source::new_and_register(
            engine,
            engine.top(),
            &("source_".to_owned() + &i.to_string()),
            option_box_repeat!(input.val; input.count),
        ));
    }

    let write_limiter = rc_limiter!(&clock, 1);
    let store_limiter =
        Limiter::new_and_register(engine, &clock, engine.top(), "limit_wr", write_limiter);
    let store =
        Store::new_and_register(engine, &clock, engine.top(), "store", total_count).unwrap();
    connect_port!(store_limiter, tx => store, rx).unwrap();

    for (i, source) in sources.iter_mut().enumerate() {
        connect_port!(source, tx => arbiter, rx, i).unwrap();
    }
    connect_port!(arbiter, tx => store_limiter, rx).unwrap();

    let port = InPort::new(
        engine,
        &clock,
        &Rc::new(Entity::new(engine.top(), "port")),
        "test_rx",
    );
    store.connect_port_tx(port.state()).unwrap();

    let check_inputs = inputs.to_owned();
    engine.spawn(async move {
        let mut store_get = vec![0; total_count];
        for i in &mut store_get {
            *i = port.get()?.await;
        }

        check_round_robin(&check_inputs, &store_get);
        Ok(())
    });
}

pub fn one_shot_channel<T>() -> (Sender<T>, Receiver<T>) {
    oneshot::channel()
}

pub trait NoTrafficPort {
    fn wait_for_traffic<'a>(
        &'a self,
        step_name: &'a str,
        port_name: &'static str,
    ) -> futures::future::LocalBoxFuture<'a, SimResult>;
}

impl<T> NoTrafficPort for InPort<T>
where
    T: SimObject,
{
    fn wait_for_traffic<'a>(
        &'a self,
        step_name: &'a str,
        port_name: &'static str,
    ) -> futures::future::LocalBoxFuture<'a, SimResult> {
        async move {
            let value = self.get()?.await;
            panic!("{step_name}: unexpected {port_name} traffic: {value}");
            #[allow(unreachable_code)]
            Ok(())
        }
        .boxed_local()
    }
}

pub async fn expect_no_traffic(
    step_name: &str,
    clock: &Clock,
    ticks: u64,
    receivers: Vec<(&'static str, &dyn NoTrafficPort)>,
) -> SimResult {
    if receivers.is_empty() {
        clock.wait_ticks(ticks).await;
        return Ok(());
    }

    let mut traffic = select_all(
        receivers
            .into_iter()
            .map(|(port_name, receiver)| receiver.wait_for_traffic(step_name, port_name))
            .collect::<Vec<_>>(),
    )
    .fuse();
    let mut timeout = clock.wait_ticks(ticks).fuse();

    futures::select! {
        (result, _, _) = traffic => {
            result?;
            panic!("{step_name}: no-traffic check completed unexpectedly");
        }
        _ = timeout => {}
    }

    Ok(())
}

pub trait ValueCheck<T> {
    fn assert_matches(&self, check_id: &str, actual: &T);
}

impl<T> ValueCheck<T> for T
where
    T: PartialEq + std::fmt::Debug,
{
    fn assert_matches(&self, check_id: &str, actual: &T) {
        assert_eq!(actual, self, "{check_id}: value mismatch");
    }
}

/// Build a simulation test harness around a component.
///
/// This macro generates the harness struct, local `Port`/`Step` enums, helper
/// functions, fixed step execution, stateful step generators, and recursive
/// sequence/parallel step driving for a component testbench.
///
/// See the crate-level Testing documentation for the intended usage pattern,
/// generated API, and examples.
#[macro_export]
macro_rules! build_component_harness {
    (
        $(#[$meta:meta])*
        $vis:vis harness $harness:ident <$item:ident> {
            component: $component_field:ident : $component_ty:ty,
            $($sections:tt)*
        }
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$(#[$meta])*]
            [$vis]
            [$harness]
            [$vis struct $harness<$item> where $item: gwr_engine::traits::SimObject]
            [impl<$item> $harness<$item> where $item: gwr_engine::traits::SimObject]
            [<$item, Expected>]
            [Expected]
            [()]
            [where $item: gwr_engine::traits::SimObject]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
        rx ports: { $($rx_section:tt)* }, $($rest:tt)*
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            [$($rx_section)*]
            [$($tx_ports)*]
            [$($rx_port_arrays)*]
            [$($tx_port_arrays)*]
            $($rest)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
        rx ports: { $($rx_section:tt)* }
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            [$($rx_section)*]
            [$($tx_ports)*]
            [$($rx_port_arrays)*]
            [$($tx_port_arrays)*]
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
        tx ports: { $($tx_section:tt)* }, $($rest:tt)*
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            [$($rx_ports)*]
            [$($tx_section)*]
            [$($rx_port_arrays)*]
            [$($tx_port_arrays)*]
            $($rest)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
        tx ports: { $($tx_section:tt)* }
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            [$($rx_ports)*]
            [$($tx_section)*]
            [$($rx_port_arrays)*]
            [$($tx_port_arrays)*]
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
        rx port arrays: { $($rx_array_section:tt)* }, $($rest:tt)*
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            [$($rx_ports)*]
            [$($tx_ports)*]
            [$($rx_array_section)*]
            [$($tx_port_arrays)*]
            $($rest)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
        rx port arrays: { $($rx_array_section:tt)* }
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            [$($rx_ports)*]
            [$($tx_ports)*]
            [$($rx_array_section)*]
            [$($tx_port_arrays)*]
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
        tx port arrays: { $($tx_array_section:tt)* }, $($rest:tt)*
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            [$($rx_ports)*]
            [$($tx_ports)*]
            [$($rx_port_arrays)*]
            [$($tx_array_section)*]
            $($rest)*
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
        tx port arrays: { $($tx_array_section:tt)* }
    ) => {
        $crate::build_component_harness! {
            @normalize
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            [$($rx_ports)*]
            [$($tx_ports)*]
            [$($rx_port_arrays)*]
            [$($tx_array_section)*]
        }
    };

    (
        @normalize
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        [$($rx_ports:tt)*]
        [$($tx_ports:tt)*]
        [$($rx_port_arrays:tt)*]
        [$($tx_port_arrays:tt)*]
    ) => {
        $crate::build_component_harness! {
            @impl_inferred
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            rx ports: { $($rx_ports)* },
            tx ports: { $($tx_ports)* },
            rx port arrays: { $($rx_port_arrays)* },
            tx port arrays: { $($tx_port_arrays)* },
        }
    };

    (
        @impl_inferred
        [$($meta:tt)*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        rx ports: {
            $(
                $rx_variant:ident <$rx_ty:ty> => $rx_field:ident
            ),* $(,)?
        },
        tx ports: {
            $(
                $tx_variant:ident <$tx_ty:ty> => $tx_field:ident
            ),* $(,)?
        },
        rx port arrays: {
            $(
                $rx_array_variant:ident <$rx_array_ty:ty> => $rx_array_field:ident {
                    count: $rx_array_count:ident
                }
            ),* $(,)?
        },
        tx port arrays: {
            $(
                $tx_array_variant:ident <$tx_array_ty:ty> => $tx_array_field:ident {
                    count: $tx_array_count:ident
                }
            ),* $(,)?
        } $(,)?
    ) => {
        $crate::build_component_harness! {
            @impl
            [$($meta)*]
            [$vis]
            [$harness]
            [$($struct_head)*]
            [$($impl_head)*]
            [$($step_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($step_where)*]
            [$item_ty]
            [$component_field: $component_ty]
            rx ports: {
                $(
                    $rx_variant <$rx_ty> => $rx_field {
                        port: { [<port_ $rx_field>] }
                    }
                ),*
            },
            tx ports: {
                $(
                    $tx_variant <$tx_ty, $tx_ty> => $tx_field {
                        connect: { [<connect_port_ $tx_field>] }
                    }
                ),*
            },
            rx port arrays: {
                $(
                    $rx_array_variant <$rx_array_ty> => $rx_array_field {
                        port: { [<port_ $rx_array_field _i>] },
                        count: $rx_array_count
                    }
                ),*
            },
            tx port arrays: {
                $(
                    $tx_array_variant <$tx_array_ty, $tx_array_ty> => $tx_array_field {
                        connect: { [<connect_port_ $tx_array_field _i>] },
                        count: $tx_array_count
                    }
                ),*
            }
        }
    };

    (
        @impl_model
        [$(#[$meta:meta])*]
        [$vis:vis]
        [$harness:ident]
        [$item:ident]
        [$default_expected:ty]
        [$access_memory:path]
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
        $crate::build_component_harness! {
            @impl
            [$(#[$meta])*]
            [$vis]
            [$harness]
            [
                $vis struct $harness<$item>
                where
                    $item: $access_memory
                        + gwr_engine::traits::SimObject
                        + Clone
                        + std::fmt::Debug
                        + 'static
            ]
            [
                impl<$item> $harness<$item>
                where
                    $item: $access_memory
                        + gwr_engine::traits::SimObject
                        + Clone
                        + std::fmt::Debug
                        + 'static
            ]
            [<$item, Expected>]
            [Expected]
            [$default_expected]
            [where
                $item: $access_memory
                    + gwr_engine::traits::SimObject
                    + Clone
                    + std::fmt::Debug
                    + 'static
            ]
            [$item]
            [$component_field: $component_ty]
            rx ports: {
                $(
                    $rx_variant <$rx_ty> => $rx_field {
                        port: { [<port_ $rx_field>] }
                    }
                ),*
            },
            tx ports: {
                $(
                    $tx_variant <$tx_ty, $default_expected> => $tx_field {
                        connect: { [<connect_port_ $tx_field>] }
                    }
                ),*
            },
            rx port arrays: {
                $(
                    $rx_array_variant <$rx_array_ty> => $rx_array_field {
                        port: { [<port_ $rx_array_field _i>] },
                        count: $rx_array_count
                    }
                ),*
            },
            tx port arrays: {
                $(
                    $tx_array_variant <$tx_array_ty, $default_expected> => $tx_array_field {
                        connect: { [<connect_port_ $tx_array_field _i>] },
                        count: $tx_array_count
                    }
                ),*
            },
        }
    };

    (
        @emit_rx_helpers
        [$item_ident:ident]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$item_ty:ty]
        [$rx_variant:ident]
        [$rx_ty:ty]
        [$rx_field:ident]
    ) => {
        $crate::test_helpers::paste! {
            pub fn [<step_send_ $rx_field>]<$item_ident>(
                value: $rx_ty,
            ) -> Step<$item_ty, $expected_ty> {
                Step::<$item_ty, $expected_ty>::[<Send $rx_variant>] {
                    port: Port::$rx_variant,
                    value,
                }
            }
        }
    };

    (
        @emit_tx_helpers
        [$item_ident:ident]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$item_ty:ty]
        [$tx_variant:ident]
        [$tx_expected_ty:ty]
        [$tx_field:ident]
    ) => {
        $crate::test_helpers::paste! {
            pub fn [<step_expect_ $tx_field>]<$item_ident>(
                value: $tx_expected_ty,
            ) -> Step<$item_ty, $expected_ty> {
                Step::<$item_ty, $expected_ty>::[<Expect $tx_variant>] {
                    port: Port::$tx_variant,
                    value,
                }
            }
        }
    };

    (
        @emit_rx_array_helpers
        [$item_ident:ident]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$item_ty:ty]
        [$rx_array_variant:ident]
        [$rx_array_ty:ty]
        [$rx_array_field:ident]
    ) => {
        $crate::test_helpers::paste! {
            pub fn [<step_send_ $rx_array_field>]<$item_ident>(
                idx: usize,
                value: $rx_array_ty,
            ) -> Step<$item_ty, $expected_ty> {
                Step::<$item_ty, $expected_ty>::[<Send $rx_array_variant>] {
                    port: Port::$rx_array_variant(idx),
                    value,
                }
            }
        }
    };

    (
        @emit_tx_array_helpers
        [$item_ident:ident]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$item_ty:ty]
        [$tx_array_variant:ident]
        [$tx_array_expected_ty:ty]
        [$tx_array_field:ident]
    ) => {
        $crate::test_helpers::paste! {
            pub fn [<step_expect_ $tx_array_field>]<$item_ident>(
                idx: usize,
                value: $tx_array_expected_ty,
            ) -> Step<$item_ty, $expected_ty> {
                Step::<$item_ty, $expected_ty>::[<Expect $tx_array_variant>] {
                    port: Port::$tx_array_variant(idx),
                    value,
                }
            }
        }
    };

    (
        @impl
        [$(#[$meta:meta])*]
        [$vis:vis]
        [$harness:ident]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($step_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($step_where:tt)*]
        [$item_ty:ty]
        [$component_field:ident : $component_ty:ty]
        rx ports: {
            $(
                $rx_variant:ident <$rx_ty:ty> => $rx_field:ident {
                    port: { $($rx_method:tt)+ }
                }
            ),* $(,)?
        },
        tx ports: {
            $(
                $tx_variant:ident <$tx_ty:ty, $tx_expected_ty:ty> => $tx_field:ident {
                    connect: { $($tx_method:tt)+ }
                }
            ),* $(,)?
        },
        rx port arrays: {
            $(
                $rx_array_variant:ident <$rx_array_ty:ty> => $rx_array_field:ident {
                    port: { $($rx_array_method:tt)+ },
                    count: $rx_array_count:ident
                }
            ),* $(,)?
        },
        tx port arrays: {
            $(
                $tx_array_variant:ident <$tx_array_ty:ty, $tx_array_expected_ty:ty> => $tx_array_field:ident {
                    connect: { $($tx_array_method:tt)+ },
                    count: $tx_array_count:ident
                }
            ),* $(,)?
        } $(,)?
    ) => {
        $crate::test_helpers::paste! {
            #[derive(Clone, Copy, Debug, PartialEq, Eq, std::hash::Hash)]
            $vis enum Port {
                $($rx_variant,)*
                $($tx_variant,)*
                $($rx_array_variant(usize),)*
                $($tx_array_variant(usize),)*
            }

            #[derive(Clone, Debug)]
            $vis enum Step<$item_ty, $expected_ident = ()> {
                Seq(Vec<Step<$item_ty, $expected_ty>>),
                Par(Vec<Step<$item_ty, $expected_ty>>),
                $([<Send $rx_variant>] { port: Port, value: $rx_ty },)*
                $([<Expect $tx_variant>] { port: Port, value: $tx_expected_ty },)*
                $([<Send $rx_array_variant>] { port: Port, value: $rx_array_ty },)*
                $([<Expect $tx_array_variant>] { port: Port, value: $tx_array_expected_ty },)*
                ExpectNoTraffic { ports: Vec<Port>, ticks: u64 },
                Delay { ports: Vec<Port>, ticks: u64 },
                #[doc(hidden)]
                __Expected(std::marker::PhantomData<fn() -> $expected_ident>),
            }

            struct [<$harness Ports>]<$item_ty> $($step_where)* {
                $(
                    [<$rx_field _driver>]: Option<gwr_engine::port::OutPort<$rx_ty>>,
                )*
                $(
                    [<$tx_field _receiver>]: Option<gwr_engine::port::InPort<$tx_ty>>,
                )*
                $(
                    [<$rx_array_field _drivers>]: Vec<Option<gwr_engine::port::OutPort<$rx_array_ty>>>,
                )*
                $(
                    [<$tx_array_field _receivers>]: Vec<Option<gwr_engine::port::InPort<$tx_array_ty>>>,
                )*
                _item: std::marker::PhantomData<fn() -> $item_ty>,
            }

            impl<$item_ty> [<$harness Ports>]<$item_ty> $($step_where)* {
                fn new_empty(&self) -> Self {
                    Self {
                        $(
                            [<$rx_field _driver>]: None,
                        )*
                        $(
                            [<$tx_field _receiver>]: None,
                        )*
                        $(
                            [<$rx_array_field _drivers>]: std::iter::repeat_with(|| None)
                                .take(self.[<$rx_array_field _drivers>].len())
                                .collect(),
                        )*
                        $(
                            [<$tx_array_field _receivers>]: std::iter::repeat_with(|| None)
                                .take(self.[<$tx_array_field _receivers>].len())
                                .collect(),
                        )*
                        _item: std::marker::PhantomData,
                    }
                }

                fn take_selected(
                    &mut self,
                    selected: &std::collections::HashSet<Port>,
                    context: &str,
                ) -> Self {
                    let mut port_collection = self.new_empty();
                    for port in selected {
                        match *port {
                            $(
                            Port::$rx_variant => {
                                port_collection.[<$rx_field _driver>] = Some(
                                    self.[<$rx_field _driver>]
                                        .take()
                                        .unwrap_or_else(|| panic!("{context}: {} driver already taken", stringify!($rx_field))),
                                );
                            }
                            )*
                            $(
                            Port::$tx_variant => {
                                port_collection.[<$tx_field _receiver>] = Some(
                                    self.[<$tx_field _receiver>]
                                        .take()
                                        .unwrap_or_else(|| panic!("{context}: {} receiver already taken", stringify!($tx_field))),
                                );
                            }
                            )*
                            $(
                            Port::$rx_array_variant(idx) => {
                                port_collection.[<$rx_array_field _drivers>][idx] = Some(
                                    self.[<$rx_array_field _drivers>]
                                        .get_mut(idx)
                                        .and_then(|driver| driver.take())
                                        .unwrap_or_else(|| panic!("{context}: {} driver index {idx} out of range or already taken", stringify!($rx_array_field))),
                                );
                            }
                            )*
                            $(
                            Port::$tx_array_variant(idx) => {
                                port_collection.[<$tx_array_field _receivers>][idx] = Some(
                                    self.[<$tx_array_field _receivers>]
                                        .get_mut(idx)
                                        .and_then(|receiver| receiver.take())
                                        .unwrap_or_else(|| panic!("{context}: {} receiver index {idx} out of range or already taken", stringify!($tx_array_field))),
                                );
                            }
                            )*
                        }
                    }
                    port_collection
                }

                fn return_ports(&mut self, mut port_collection: Self, context: &str) {
                    $(
                    if let Some(driver) = port_collection.[<$rx_field _driver>].take() {
                        if self.[<$rx_field _driver>].replace(driver).is_some() {
                            panic!("{context}: {} driver returned twice", stringify!($rx_field));
                        }
                    }
                    )*
                    $(
                    if let Some(receiver) = port_collection.[<$tx_field _receiver>].take() {
                        if self.[<$tx_field _receiver>].replace(receiver).is_some() {
                            panic!("{context}: {} receiver returned twice", stringify!($tx_field));
                        }
                    }
                    )*
                    $(
                    for (idx, driver) in port_collection.[<$rx_array_field _drivers>].into_iter().enumerate() {
                        if let Some(driver) = driver {
                            if self.[<$rx_array_field _drivers>][idx].replace(driver).is_some() {
                                panic!("{context}: {} driver index {idx} returned twice", stringify!($rx_array_field));
                            }
                        }
                    }
                    )*
                    $(
                    for (idx, receiver) in port_collection.[<$tx_array_field _receivers>].into_iter().enumerate() {
                        if let Some(receiver) = receiver {
                            if self.[<$tx_array_field _receivers>][idx].replace(receiver).is_some() {
                                panic!("{context}: {} receiver index {idx} returned twice", stringify!($tx_array_field));
                            }
                        }
                    }
                    )*
                }

                fn collect_step_ports(
                    step: &Step<$item_ty, $expected_ty>,
                    ports: &mut std::collections::HashSet<Port>,
                ) {
                    match step {
                        Step::<$item_ty, $expected_ty>::Seq(steps)
                        | Step::<$item_ty, $expected_ty>::Par(steps) => {
                            for step in steps {
                                Self::collect_step_ports(step, ports);
                            }
                        }
                        $(
                        Step::<$item_ty, $expected_ty>::[<Send $rx_variant>] { port, .. } => {
                            ports.insert(*port);
                        }
                        )*
                        $(
                        Step::<$item_ty, $expected_ty>::[<Expect $tx_variant>] { port, .. } => {
                            ports.insert(*port);
                        }
                        )*
                        $(
                        Step::<$item_ty, $expected_ty>::[<Send $rx_array_variant>] { port, .. } => {
                            ports.insert(*port);
                        }
                        )*
                        $(
                        Step::<$item_ty, $expected_ty>::[<Expect $tx_array_variant>] { port, .. } => {
                            ports.insert(*port);
                        }
                        )*
                        Step::<$item_ty, $expected_ty>::ExpectNoTraffic { ports: step_ports, .. } => {
                            ports.extend(step_ports.iter().copied());
                        }
                        Step::<$item_ty, $expected_ty>::Delay { ports: step_ports, .. } => {
                            ports.extend(step_ports.iter().copied());
                        }
                        Step::<$item_ty, $expected_ty>::__Expected(_) => {
                            unreachable!("marker variant is not a harness step");
                        }
                    }
                }

                fn run_steps(
                    mut self,
                    steps: Vec<Step<$item_ty, $expected_ty>>,
                    clock: gwr_engine::time::clock::Clock,
                    spawner: gwr_engine::executor::Spawner,
                    context: String,
                ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Self, gwr_engine::types::SimError>> + 'static>>
                where
                    $($rx_ty: Clone + 'static,)*
                    $($tx_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_ty> + 'static,)*
                    $($rx_array_ty: Clone + 'static,)*
                    $($tx_array_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_array_ty> + 'static,)*
                    $item_ty: 'static,
                    $expected_ty: 'static,
                {
                    Box::pin(async move {
                        for (step_idx, step) in steps.into_iter().enumerate() {
                            let step_context = if context.is_empty() {
                                format!("step {step_idx}")
                            } else {
                                format!("{context} step {step_idx}")
                            };
                            match step {
                                Step::<$item_ty, $expected_ty>::Seq(steps) => {
                                    self = self.run_steps(steps, clock.clone(), spawner.clone(), step_context).await?;
                                }
                                Step::<$item_ty, $expected_ty>::Par(steps) => {
                                    let mut completions = Vec::with_capacity(steps.len());

                                    for (par_step_idx, step) in steps.into_iter().enumerate() {
                                        let branch_context = format!("{step_context}: parallel step {par_step_idx}");
                                        let mut branch_ports = std::collections::HashSet::new();
                                        Self::collect_step_ports(&step, &mut branch_ports);
                                        let branch_runner_ports = self.take_selected(&branch_ports, &branch_context);
                                        let branch_clock = clock.clone();
                                        let branch_spawner = spawner.clone();
                                        let (complete_tx, complete_rx) = $crate::test_helpers::one_shot_channel();

                                        spawner.spawn(async move {
                                            let branch_steps = match step {
                                                Step::<$item_ty, $expected_ty>::Seq(steps) => steps,
                                                step => vec![step],
                                            };
                                            let result = branch_runner_ports
                                                .run_steps(branch_steps, branch_clock, branch_spawner, branch_context.clone())
                                                .await;
                                            complete_tx
                                                .send((branch_context, result))
                                                .unwrap_or_else(|_| panic!("parallel step receiver dropped"));
                                            Ok::<(), gwr_engine::types::SimError>(())
                                        });
                                        completions.push(complete_rx);
                                    }

                                    for completion in completions {
                                        let (branch_context, result) = completion
                                            .await
                                            .unwrap_or_else(|_| panic!("{step_context}: parallel section dropped"));
                                        let returned = result?;
                                        self.return_ports(returned, &branch_context);
                                    }
                                }
                                $(
                                Step::<$item_ty, $expected_ty>::[<Send $rx_variant>] { port, value } => {
                                    let Port::$rx_variant = port else {
                                        panic!("{step_context} {port:?}: step is for {}", stringify!($rx_variant));
                                    };
                                    self.[<$rx_field _driver>]
                                        .as_ref()
                                        .expect(concat!(stringify!($rx_field), " driver already taken"))
                                        .put(value.clone())?
                                        .await;
                                }
                                )*
                                $(
                                Step::<$item_ty, $expected_ty>::[<Expect $tx_variant>] { port, value } => {
                                    let Port::$tx_variant = port else {
                                        panic!("{step_context} {port:?}: step is for {}", stringify!($tx_variant));
                                    };
                                    let actual = self.[<$tx_field _receiver>]
                                        .as_ref()
                                        .expect(concat!(stringify!($tx_field), " receiver already taken"))
                                        .get()?
                                        .await;
                                    $crate::test_helpers::ValueCheck::assert_matches(
                                        &value,
                                        &format!("{step_context} {port:?}"),
                                        &actual,
                                    );
                                }
                                )*
                                $(
                                Step::<$item_ty, $expected_ty>::[<Send $rx_array_variant>] { port, value } => {
                                    let Port::$rx_array_variant(idx) = port else {
                                        panic!("{step_context} {port:?}: step is for {}", stringify!($rx_array_variant));
                                    };
                                    self.[<$rx_array_field _drivers>]
                                        .get(idx)
                                        .and_then(|driver| driver.as_ref())
                                        .unwrap_or_else(|| panic!("{} driver index {idx} out of range or already taken", stringify!($rx_array_field)))
                                        .put(value.clone())?
                                        .await;
                                }
                                )*
                                $(
                                Step::<$item_ty, $expected_ty>::[<Expect $tx_array_variant>] { port, value } => {
                                    let Port::$tx_array_variant(idx) = port else {
                                        panic!("{step_context} {port:?}: step is for {}", stringify!($tx_array_variant));
                                    };
                                    let actual = self.[<$tx_array_field _receivers>]
                                        .get(idx)
                                        .and_then(|receiver| receiver.as_ref())
                                        .unwrap_or_else(|| panic!("{} receiver index {idx} out of range or already taken", stringify!($tx_array_field)))
                                        .get()?
                                        .await;
                                    $crate::test_helpers::ValueCheck::assert_matches(
                                        &value,
                                        &format!("{step_context} {port:?}"),
                                        &actual,
                                    );
                                }
                                )*
                                Step::<$item_ty, $expected_ty>::ExpectNoTraffic { ports, ticks } => {
                                    let mut receivers = Vec::new();
                                    for port in &ports {
                                        match port {
                                            $(
                                            Port::$tx_variant => {
                                                let receiver = self.[<$tx_field _receiver>]
                                                    .as_ref()
                                                    .expect(concat!(stringify!($tx_field), " receiver already taken"));
                                                receivers.push((stringify!($tx_field), receiver as &dyn $crate::test_helpers::NoTrafficPort));
                                            }
                                            )*
                                            $(
                                            Port::$tx_array_variant(idx) => {
                                                let receiver = self.[<$tx_array_field _receivers>]
                                                    .get(*idx)
                                                    .and_then(|receiver| receiver.as_ref())
                                                    .unwrap_or_else(|| panic!("{} receiver index {idx} out of range or already taken", stringify!($tx_array_field)));
                                                receivers.push((stringify!($tx_array_field), receiver as &dyn $crate::test_helpers::NoTrafficPort));
                                            }
                                            )*
                                            _ => {
                                                panic!("{step_context} {port:?}: expect no traffic requires tx ports");
                                            }
                                        }
                                    }

                                    $crate::test_helpers::expect_no_traffic(
                                        &step_context,
                                        &clock,
                                        ticks,
                                        receivers,
                                    )
                                    .await?;
                                }
                                Step::<$item_ty, $expected_ty>::Delay { ports, ticks } => {
                                    if !ports.is_empty() {
                                        panic!("{step_context}: delay does not take ports");
                                    }
                                    clock.wait_ticks(ticks).await;
                                }
                                Step::<$item_ty, $expected_ty>::__Expected(_) => {
                                    unreachable!("marker variant is not a harness step");
                                }
                            }
                        }
                        Ok(self)
                    })
                }
            }

            $(#[$meta])*
            $($struct_head)* {
                pub engine: gwr_engine::engine::Engine,
                pub clock: gwr_engine::time::clock::Clock,
                pub $component_field: $component_ty,
                $(
                    [<$rx_field _driver>]: Option<gwr_engine::port::OutPort<$rx_ty>>,
                )*
                $(
                    [<$tx_field _receiver>]: Option<gwr_engine::port::InPort<$tx_ty>>,
                )*
                $(
                    [<$rx_array_field _drivers>]: Vec<gwr_engine::port::OutPort<$rx_array_ty>>,
                )*
                $(
                    [<$tx_array_field _receivers>]: Vec<gwr_engine::port::InPort<$tx_array_ty>>,
                )*
                _expected: std::marker::PhantomData<$expected_ty>,
            }

            $($impl_head)* {
                pub fn new(
                    mut engine: gwr_engine::engine::Engine,
                    $component_field: $component_ty,
                    $($rx_array_count: usize,)*
                    $($tx_array_count: usize,)*
                ) -> Self {
                    let clock = engine.default_clock();
                    let top = engine.top();

                    $(
                        let mut [<$rx_field _driver>] = gwr_engine::port::OutPort::new(
                            top,
                            concat!(stringify!($rx_field), "_driver"),
                        );
                        [<$rx_field _driver>]
                            .connect($component_field.$($rx_method)+())
                            .unwrap();
                    )*

                    $(
                        let [<$tx_field _receiver>] = gwr_engine::port::InPort::new(
                            &engine,
                            &clock,
                            top,
                            concat!(stringify!($tx_field), "_receiver"),
                        );
                        $component_field
                            .$($tx_method)+([<$tx_field _receiver>].state())
                            .unwrap();
                    )*

                    $(
                        let mut [<$rx_array_field _drivers>] = Vec::with_capacity($rx_array_count);
                        for idx in 0..$rx_array_count {
                            let mut driver = gwr_engine::port::OutPort::new(
                                top,
                                &format!("{}_{}_driver", stringify!($rx_array_field), idx),
                            );
                            driver.connect($component_field.$($rx_array_method)+(idx)).unwrap();
                            [<$rx_array_field _drivers>].push(driver);
                        }
                    )*

                    $(
                        let mut [<$tx_array_field _receivers>] = Vec::with_capacity($tx_array_count);
                        for idx in 0..$tx_array_count {
                            let receiver = gwr_engine::port::InPort::new(
                                &engine,
                                &clock,
                                top,
                                &format!("{}_{}_receiver", stringify!($tx_array_field), idx),
                            );
                            $component_field
                                .$($tx_array_method)+(idx, receiver.state())
                                .unwrap();
                            [<$tx_array_field _receivers>].push(receiver);
                        }
                    )*

                    Self {
                        engine,
                        clock,
                        $component_field,
                        $(
                            [<$rx_field _driver>]: Some([<$rx_field _driver>]),
                        )*
                        $(
                            [<$tx_field _receiver>]: Some([<$tx_field _receiver>]),
                        )*
                        $(
                            [<$rx_array_field _drivers>],
                        )*
                        $(
                            [<$tx_array_field _receivers>],
                        )*
                        _expected: std::marker::PhantomData,
                    }
                }

                $(
                    pub fn [<take_ $rx_field _driver>](
                        &mut self,
                    ) -> gwr_engine::port::OutPort<$rx_ty> {
                        self.[<$rx_field _driver>]
                            .take()
                            .expect(concat!(stringify!($rx_field), " driver already taken"))
                    }

                )*

                $(
                    pub fn [<take_ $tx_field _receiver>](
                        &mut self,
                    ) -> gwr_engine::port::InPort<$tx_ty> {
                        self.[<$tx_field _receiver>]
                            .take()
                            .expect(concat!(stringify!($tx_field), " receiver already taken"))
                    }

                    pub async fn [<expect_no_ $tx_field _traffic>](
                        &self,
                        ticks: u64,
                    ) -> gwr_engine::types::SimResult {
                        $crate::test_helpers::expect_no_traffic(
                            stringify!($tx_field),
                            &self.clock,
                            ticks,
                            vec![
                                (
                                    stringify!($tx_field),
                                    self.[<$tx_field _receiver>]
                                        .as_ref()
                                        .expect(concat!(stringify!($tx_field), " receiver already taken")),
                                ),
                            ],
                        )
                        .await
                    }
                )*

                $(
                    pub fn [<take_ $rx_array_field _drivers>](
                        &mut self,
                    ) -> Vec<gwr_engine::port::OutPort<$rx_array_ty>> {
                        std::mem::take(&mut self.[<$rx_array_field _drivers>])
                    }

                )*

                $(
                    pub fn [<take_ $tx_array_field _receivers>](
                        &mut self,
                    ) -> Vec<gwr_engine::port::InPort<$tx_array_ty>> {
                        std::mem::take(&mut self.[<$tx_array_field _receivers>])
                    }

                )*

                #[allow(unreachable_code)]
                pub fn run_steps<Steps>(
                    &mut self,
                    steps: Steps,
                )
                where
                    Steps: IntoIterator<Item = Step<$item_ty, $expected_ty>>,
                    Steps::IntoIter: 'static,
                    $($rx_ty: Clone + 'static,)*
                    $($tx_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_ty> + 'static,)*
                    $($rx_array_ty: Clone + 'static,)*
                    $($tx_array_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_array_ty> + 'static,)*
                    $item_ty: 'static,
                    $expected_ty: 'static,
                {
                    self.run_step_generator(steps.into_iter());
                }

                #[allow(unreachable_code)]
                pub fn run_step_generator<I>(
                    &mut self,
                    mut steps: I,
                )
                where
                    I: Iterator<Item = Step<$item_ty, $expected_ty>> + 'static,
                    $($rx_ty: Clone + 'static,)*
                    $($tx_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_ty> + 'static,)*
                    $($rx_array_ty: Clone + 'static,)*
                    $($tx_array_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_array_ty> + 'static,)*
                    $item_ty: 'static,
                    $expected_ty: 'static,
                {
                    let harness_complete = gwr_engine::events::once::Once::default();
                    let notify_harness_complete = harness_complete.clone();
                    let harness_completed = std::rc::Rc::new(std::cell::RefCell::new(false));
                    let mark_harness_completed = harness_completed.clone();
                    let clock = self.clock.clone();
                    let spawner = self.engine.spawner();
                    let runner_ports = [<$harness Ports>]::<$item_ty> {
                        $(
                            [<$rx_field _driver>]: Some(self.[<take_ $rx_field _driver>]()),
                        )*
                        $(
                            [<$tx_field _receiver>]: Some(self.[<take_ $tx_field _receiver>]()),
                        )*
                        $(
                            [<$rx_array_field _drivers>]: self
                                .[<take_ $rx_array_field _drivers>]()
                                .into_iter()
                                .map(Some)
                                .collect(),
                        )*
                        $(
                            [<$tx_array_field _receivers>]: self
                                .[<take_ $tx_array_field _receivers>]()
                                .into_iter()
                                .map(Some)
                                .collect(),
                        )*
                        _item: std::marker::PhantomData,
                    };

                    self.engine.spawn(async move {
                        let mut runner_ports = runner_ports;
                        for step in steps {
                            runner_ports = runner_ports
                                .run_steps(vec![step], clock.clone(), spawner.clone(), String::new())
                                .await?;
                        }
                        *mark_harness_completed.borrow_mut() = true;
                        notify_harness_complete.notify()?;
                        Ok::<(), gwr_engine::types::SimError>(())
                    });

                    let engine = &mut self.engine;
                    engine.run_until(Box::new(harness_complete)).unwrap();
                    if !*harness_completed.borrow() {
                        panic!("test harness did not complete");
                    }
                }

            }

            $(
                $crate::build_component_harness! {
                    @emit_rx_helpers
                    [$item_ty]
                    [$expected_ident]
                    [$expected_ty]
                    [$item_ty]
                    [$rx_variant]
                    [$rx_ty]
                    [$rx_field]
                }
            )*

            $(
                $crate::build_component_harness! {
                    @emit_tx_helpers
                    [$item_ty]
                    [$expected_ident]
                    [$expected_ty]
                    [$item_ty]
                    [$tx_variant]
                    [$tx_expected_ty]
                    [$tx_field]
                }
            )*

            $(
                $crate::build_component_harness! {
                    @emit_rx_array_helpers
                    [$item_ty]
                    [$expected_ident]
                    [$expected_ty]
                    [$item_ty]
                    [$rx_array_variant]
                    [$rx_array_ty]
                    [$rx_array_field]
                }
            )*

            $(
                $crate::build_component_harness! {
                    @emit_tx_array_helpers
                    [$item_ty]
                    [$expected_ident]
                    [$expected_ty]
                    [$item_ty]
                    [$tx_array_variant]
                    [$tx_array_expected_ty]
                    [$tx_array_field]
                }
            )*

            pub fn step_delay<$item_ty>(
                ticks: u64,
            ) -> Step<$item_ty, $expected_ty> {
                Step::<$item_ty, $expected_ty>::Delay {
                    ports: Vec::new(),
                    ticks,
                }
            }

            pub fn step_expect_no_traffic<$item_ty>(
                ports: &[Port],
                ticks: u64,
            ) -> Step<$item_ty, $expected_ty> {
                Step::<$item_ty, $expected_ty>::ExpectNoTraffic {
                    ports: ports.to_vec(),
                    ticks,
                }
            }

            pub fn step_seq<$item_ty, Steps>(
                steps: Steps,
            ) -> Step<$item_ty, $expected_ty>
            where
                Steps: IntoIterator<Item = Step<$item_ty, $expected_ty>>,
            {
                Step::<$item_ty, $expected_ty>::Seq(steps.into_iter().collect())
            }

            pub fn step_par<$item_ty, Steps>(
                steps: Steps,
            ) -> Step<$item_ty, $expected_ty>
            where
                Steps: IntoIterator<Item = Step<$item_ty, $expected_ty>>,
            {
                Step::<$item_ty, $expected_ty>::Par(steps.into_iter().collect())
            }

        }
    };
}
