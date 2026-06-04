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
    )
    .unwrap();
    let mut sources = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        sources.push(
            Source::new_and_register(
                engine,
                engine.top(),
                &("source_".to_owned() + &i.to_string()),
                option_box_repeat!(input.val; input.count),
            )
            .unwrap(),
        );
    }

    let write_limiter = rc_limiter!(&clock, 1);
    let store_limiter =
        Limiter::new_and_register(engine, &clock, engine.top(), "limit_wr", write_limiter).unwrap();
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
/// This macro generates the harness struct, local `Port`/`Action`/`Step` enums,
/// helper functions, fixed step execution, stateful step generators, and
/// parallel port driving for a component testbench.
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            [$($struct_head)*]
            [$($impl_head)*]
            [$($action_generics_decl)*]
            [$expected_ident]
            [$expected_ty]
            [$($action_where)*]
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
                Step::<$item_ty, $expected_ty>::Action {
                    ports: vec![Port::$rx_variant],
                    action: Action::<$item_ty, $expected_ty>::[<Send $rx_variant>] { value },
                }
            }

            pub fn [<action_send_ $rx_field>]<$item_ident>(
                value: $rx_ty,
            ) -> Action<$item_ty, $expected_ty> {
                Action::<$item_ty, $expected_ty>::[<Send $rx_variant>] { value }
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
                Step::<$item_ty, $expected_ty>::Action {
                    ports: vec![Port::$tx_variant],
                    action: Action::<$item_ty, $expected_ty>::[<Expect $tx_variant>] { value },
                }
            }

            pub fn [<action_expect_ $tx_field>]<$item_ident>(
                value: $tx_expected_ty,
            ) -> Action<$item_ty, $expected_ty> {
                Action::<$item_ty, $expected_ty>::[<Expect $tx_variant>] { value }
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
                Step::<$item_ty, $expected_ty>::Action {
                    ports: vec![Port::$rx_array_variant(idx)],
                    action: Action::<$item_ty, $expected_ty>::[<Send $rx_array_variant>] { value },
                }
            }

            pub fn [<action_send_ $rx_array_field>]<$item_ident>(
                value: $rx_array_ty,
            ) -> Action<$item_ty, $expected_ty> {
                Action::<$item_ty, $expected_ty>::[<Send $rx_array_variant>] { value }
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
                Step::<$item_ty, $expected_ty>::Action {
                    ports: vec![Port::$tx_array_variant(idx)],
                    action: Action::<$item_ty, $expected_ty>::[<Expect $tx_array_variant>] { value },
                }
            }

            pub fn [<action_expect_ $tx_array_field>]<$item_ident>(
                value: $tx_array_expected_ty,
            ) -> Action<$item_ty, $expected_ty> {
                Action::<$item_ty, $expected_ty>::[<Expect $tx_array_variant>] { value }
            }
        }
    };

    (
        @impl
        [$(#[$meta:meta])*]
        [$vis:vis]
        [$($struct_head:tt)+]
        [$($impl_head:tt)+]
        [$($action_generics_decl:tt)*]
        [$expected_ident:ident]
        [$expected_ty:ty]
        [$($action_where:tt)*]
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
            $vis enum Action<$item_ty, $expected_ident = ()> {
                $([<Send $rx_variant>] { value: $rx_ty },)*
                $([<Expect $tx_variant>] { value: $tx_expected_ty },)*
                $([<Send $rx_array_variant>] { value: $rx_array_ty },)*
                $([<Expect $tx_array_variant>] { value: $tx_array_expected_ty },)*
                ExpectNoTraffic { ticks: u64 },
                Delay { ticks: u64 },
                #[doc(hidden)]
                __Expected(std::marker::PhantomData<fn() -> $expected_ident>),
            }

            #[derive(Clone, Debug)]
            $vis enum Step<$item_ty, $expected_ident = ()> {
                Action {
                    ports: Vec<Port>,
                    action: Action<$item_ty, $expected_ty>,
                },
                Parallel {
                    sections: std::collections::HashMap<
                        Port,
                        Vec<Action<$item_ty, $expected_ty>>,
                    >,
                },
                #[doc(hidden)]
                __Expected(std::marker::PhantomData<fn() -> $expected_ident>),
            }

            enum ParallelPort<$item_ty> $($action_where)* {
                $($rx_variant(gwr_engine::port::OutPort<$rx_ty>),)*
                $($tx_variant(gwr_engine::port::InPort<$tx_ty>),)*
                $($rx_array_variant(usize, gwr_engine::port::OutPort<$rx_array_ty>),)*
                $($tx_array_variant(usize, gwr_engine::port::InPort<$tx_array_ty>),)*
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
                pub fn run_steps(
                    &mut self,
                    steps: &[Step<$item_ty, $expected_ty>],
                )
                where
                    $($rx_ty: Clone,)*
                    $($tx_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_ty>,)*
                    $($rx_array_ty: Clone,)*
                    $($tx_array_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_array_ty>,)*
                {
                    self.run_step_generator(steps.to_vec().into_iter());
                }

                #[allow(unreachable_code)]
                pub fn run_step_generator<I>(
                    &mut self,
                    mut steps: I,
                )
                where
                    I: Iterator<Item = Step<$item_ty, $expected_ty>> + 'static,
                    $($rx_ty: Clone,)*
                    $($tx_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_ty>,)*
                    $($rx_array_ty: Clone,)*
                    $($tx_array_expected_ty: Clone + $crate::test_helpers::ValueCheck<$tx_array_ty>,)*
                {
                    let harness_complete = gwr_engine::events::once::Once::default();
                    let notify_harness_complete = harness_complete.clone();
                    let harness_completed = std::rc::Rc::new(std::cell::RefCell::new(false));
                    let mark_harness_completed = harness_completed.clone();
                    let clock = self.clock.clone();
                    let spawner = self.engine.spawner();
                    $(let mut [<$rx_field _driver>] = Some(self.[<take_ $rx_field _driver>]());)*
                    $(let mut [<$tx_field _receiver>] = Some(self.[<take_ $tx_field _receiver>]());)*
                    $(
                        let mut [<$rx_array_field _drivers>]: Vec<_> = self
                            .[<take_ $rx_array_field _drivers>]()
                            .into_iter()
                            .map(Some)
                            .collect();
                    )*
                    $(
                        let mut [<$tx_array_field _receivers>]: Vec<_> = self
                            .[<take_ $tx_array_field _receivers>]()
                            .into_iter()
                            .map(Some)
                            .collect();
                    )*

                    self.engine.spawn(async move {
                        for (step_idx, step) in steps.enumerate() {
                            match &step {
                                Step::<$item_ty, $expected_ty>::Action { ports, action } => {
                                    match action {
                                        $(
                                        Action::<$item_ty, $expected_ty>::[<Send $rx_variant>] { value } => {
                                            let [port] = ports.as_slice() else {
                                                panic!("step {step_idx}: send requires exactly one port");
                                            };
                                            let Port::$rx_variant = port else {
                                                panic!("step {step_idx} {port:?}: action is for {}", stringify!($rx_variant));
                                            };
                                            [<$rx_field _driver>]
                                                .as_ref()
                                                .expect(concat!(stringify!($rx_field), " driver already taken"))
                                                .put(value.clone())?
                                                .await;
                                        }
                                        )*
                                        $(
                                        Action::<$item_ty, $expected_ty>::[<Expect $tx_variant>] { value } => {
                                            let [port] = ports.as_slice() else {
                                                panic!("step {step_idx}: expect requires exactly one port");
                                            };
                                            let Port::$tx_variant = port else {
                                                panic!("step {step_idx} {port:?}: action is for {}", stringify!($tx_variant));
                                            };
                                            let actual = [<$tx_field _receiver>]
                                                .as_ref()
                                                .expect(concat!(stringify!($tx_field), " receiver already taken"))
                                                .get()?
                                                .await;
                                            $crate::test_helpers::ValueCheck::assert_matches(
                                                value,
                                                &format!("step {step_idx} {port:?}"),
                                                &actual,
                                            );
                                        }
                                        )*
                                        $(
                                        Action::<$item_ty, $expected_ty>::[<Send $rx_array_variant>] { value } => {
                                            let [port] = ports.as_slice() else {
                                                panic!("step {step_idx}: send requires exactly one port");
                                            };
                                            let Port::$rx_array_variant(idx) = port else {
                                                panic!("step {step_idx} {port:?}: action is for {}", stringify!($rx_array_variant));
                                            };
                                            [<$rx_array_field _drivers>]
                                                .get(*idx)
                                                .and_then(|driver| driver.as_ref())
                                                .unwrap_or_else(|| panic!("{} driver index {idx} out of range or already taken", stringify!($rx_array_field)))
                                                .put(value.clone())?
                                                .await;
                                        }
                                        )*
                                        $(
                                        Action::<$item_ty, $expected_ty>::[<Expect $tx_array_variant>] { value } => {
                                            let [port] = ports.as_slice() else {
                                                panic!("step {step_idx}: expect requires exactly one port");
                                            };
                                            let Port::$tx_array_variant(idx) = port else {
                                                panic!("step {step_idx} {port:?}: action is for {}", stringify!($tx_array_variant));
                                            };
                                            let actual = [<$tx_array_field _receivers>]
                                                .get(*idx)
                                                .and_then(|receiver| receiver.as_ref())
                                                .unwrap_or_else(|| panic!("{} receiver index {idx} out of range or already taken", stringify!($tx_array_field)))
                                                .get()?
                                                .await;
                                            $crate::test_helpers::ValueCheck::assert_matches(
                                                value,
                                                &format!("step {step_idx} {port:?}"),
                                                &actual,
                                            );
                                        }
                                        )*
                                        Action::<$item_ty, $expected_ty>::ExpectNoTraffic { ticks } => {
                                            let mut receivers = Vec::new();
                                            for port in ports {
                                                match port {
                                                    $(
                                                    Port::$tx_variant => {
                                                        let receiver = [<$tx_field _receiver>]
                                                            .as_ref()
                                                            .expect(concat!(stringify!($tx_field), " receiver already taken"));
                                                        receivers.push((stringify!($tx_field), receiver as &dyn $crate::test_helpers::NoTrafficPort));
                                                    }
                                                    )*
                                                    $(
                                                    Port::$tx_array_variant(idx) => {
                                                        let receiver = [<$tx_array_field _receivers>]
                                                            .get(*idx)
                                                            .and_then(|receiver| receiver.as_ref())
                                                            .unwrap_or_else(|| panic!("{} receiver index {idx} out of range or already taken", stringify!($tx_array_field)));
                                                        receivers.push((stringify!($tx_array_field), receiver as &dyn $crate::test_helpers::NoTrafficPort));
                                                    }
                                                    )*
                                                    _ => {
                                                        panic!("step {step_idx} {port:?}: expect no traffic requires tx ports");
                                                    }
                                                }
                                            }

                                            $crate::test_helpers::expect_no_traffic(
                                                &format!("step {step_idx}"),
                                                &clock,
                                                *ticks,
                                                receivers,
                                            )
                                            .await?;
                                        }
                                        Action::<$item_ty, $expected_ty>::Delay { ticks } => {
                                            if !ports.is_empty() {
                                                panic!("step {step_idx}: delay does not take ports");
                                            }
                                            clock.wait_ticks(*ticks).await;
                                        }
                                        Action::<$item_ty, $expected_ty>::__Expected(_) => {
                                            unreachable!("marker variant is not a harness action");
                                        }
                                    }
                                }
                                Step::<$item_ty, $expected_ty>::Parallel { sections } => {
                                    let mut completions = Vec::with_capacity(sections.len());

                                    for (parallel_port, parallel_steps) in sections {
                                        let resource = match parallel_port {
                                            $(
                                            Port::$rx_variant => ParallelPort::<$item_ty>::$rx_variant(
                                                [<$rx_field _driver>]
                                                    .take()
                                                    .expect(concat!(stringify!($rx_field), " driver already taken")),
                                            ),
                                            )*
                                            $(
                                            Port::$tx_variant => ParallelPort::<$item_ty>::$tx_variant(
                                                [<$tx_field _receiver>]
                                                    .take()
                                                    .expect(concat!(stringify!($tx_field), " receiver already taken")),
                                            ),
                                            )*
                                            $(
                                            Port::$rx_array_variant(idx) => ParallelPort::<$item_ty>::$rx_array_variant(
                                                *idx,
                                                [<$rx_array_field _drivers>]
                                                    .get_mut(*idx)
                                                    .and_then(|driver| driver.take())
                                                    .unwrap_or_else(|| panic!("{} driver index {idx} out of range or already taken", stringify!($rx_array_field))),
                                            ),
                                            )*
                                            $(
                                            Port::$tx_array_variant(idx) => ParallelPort::<$item_ty>::$tx_array_variant(
                                                *idx,
                                                [<$tx_array_field _receivers>]
                                                    .get_mut(*idx)
                                                    .and_then(|receiver| receiver.take())
                                                    .unwrap_or_else(|| panic!("{} receiver index {idx} out of range or already taken", stringify!($tx_array_field))),
                                            ),
                                            )*
                                        };
                                        let parallel_port = *parallel_port;
                                        let parallel_steps = parallel_steps.clone();
                                        let parallel_clock = clock.clone();
                                        let (complete_tx, complete_rx) = $crate::test_helpers::one_shot_channel();

                                        spawner.spawn(async move {
                                            for (par_step_idx, action) in parallel_steps.iter().enumerate() {
                                                match (&resource, action) {
                                                    $(
                                                    (ParallelPort::<$item_ty>::$rx_variant(driver), Action::<$item_ty, $expected_ty>::[<Send $rx_variant>] { value }) => {
                                                        driver.put(value.clone())?.await;
                                                    }
                                                    )*
                                                    $(
                                                    (ParallelPort::<$item_ty>::$tx_variant(receiver), Action::<$item_ty, $expected_ty>::[<Expect $tx_variant>] { value }) => {
                                                        let actual = receiver.get()?.await;
                                                        $crate::test_helpers::ValueCheck::assert_matches(
                                                            value,
                                                            &format!("step {step_idx}: parallel step {par_step_idx} {parallel_port:?}"),
                                                            &actual,
                                                        );
                                                    }
                                                    )*
                                                    $(
                                                    (ParallelPort::<$item_ty>::$rx_array_variant(_, driver), Action::<$item_ty, $expected_ty>::[<Send $rx_array_variant>] { value }) => {
                                                        driver.put(value.clone())?.await;
                                                    }
                                                    )*
                                                    $(
                                                    (ParallelPort::<$item_ty>::$tx_array_variant(_, receiver), Action::<$item_ty, $expected_ty>::[<Expect $tx_array_variant>] { value }) => {
                                                        let actual = receiver.get()?.await;
                                                        $crate::test_helpers::ValueCheck::assert_matches(
                                                            value,
                                                            &format!("step {step_idx}: parallel step {par_step_idx} {parallel_port:?}"),
                                                            &actual,
                                                        );
                                                    }
                                                    )*
                                                    (_, Action::<$item_ty, $expected_ty>::ExpectNoTraffic { ticks }) => {
                                                        match &resource {
                                                            $(
                                                            ParallelPort::<$item_ty>::$tx_variant(receiver) => {
                                                                $crate::test_helpers::expect_no_traffic(
                                                                    &format!("step {step_idx}: parallel step {par_step_idx}"),
                                                                    &parallel_clock,
                                                                    *ticks,
                                                                    vec![(
                                                                        stringify!($tx_field),
                                                                        receiver as &dyn $crate::test_helpers::NoTrafficPort,
                                                                    )],
                                                                )
                                                                .await?;
                                                            }
                                                            )*
                                                            $(
                                                            ParallelPort::<$item_ty>::$tx_array_variant(_, receiver) => {
                                                                $crate::test_helpers::expect_no_traffic(
                                                                    &format!("step {step_idx}: parallel step {par_step_idx}"),
                                                                    &parallel_clock,
                                                                    *ticks,
                                                                    vec![(
                                                                        stringify!($tx_array_field),
                                                                        receiver as &dyn $crate::test_helpers::NoTrafficPort,
                                                                    )],
                                                                )
                                                                .await?;
                                                            }
                                                            )*
                                                            _ => {
                                                                panic!("step {step_idx}: parallel step {par_step_idx} {parallel_port:?}: cannot expect traffic on rx port");
                                                            }
                                                        }
                                                    }
                                                    (_, Action::<$item_ty, $expected_ty>::Delay { ticks }) => {
                                                        parallel_clock.wait_ticks(*ticks).await;
                                                    }
                                                    _ => {
                                                        panic!("step {step_idx}: parallel step {par_step_idx} {parallel_port:?}: action is for a different port");
                                                    }
                                                }
                                            }

                                            complete_tx
                                                .send((parallel_port, resource))
                                                .unwrap_or_else(|_| panic!("parallel step receiver dropped"));
                                            Ok::<(), gwr_engine::types::SimError>(())
                                        });
                                        completions.push(complete_rx);
                                    }

                                    for completion in completions {
                                        let (parallel_port, resource) = completion
                                            .await
                                            .unwrap_or_else(|_| panic!("step {step_idx}: parallel section dropped"));
                                        match resource {
                                            $(ParallelPort::<$item_ty>::$rx_variant(driver) => {
                                                [<$rx_field _driver>] = Some(driver);
                                            })*
                                            $(ParallelPort::<$item_ty>::$tx_variant(receiver) => {
                                                [<$tx_field _receiver>] = Some(receiver);
                                            })*
                                            $(ParallelPort::<$item_ty>::$rx_array_variant(idx, driver) => {
                                                [<$rx_array_field _drivers>][idx] = Some(driver);
                                            })*
                                            $(ParallelPort::<$item_ty>::$tx_array_variant(idx, receiver) => {
                                                [<$tx_array_field _receivers>][idx] = Some(receiver);
                                            })*
                                        }
                                    }
                                }
                                Step::<$item_ty, $expected_ty>::__Expected(_) => {
                                    unreachable!("marker variant is not a harness step");
                                }
                            }
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
                Step::<$item_ty, $expected_ty>::Action {
                    ports: Vec::new(),
                    action: Action::<$item_ty, $expected_ty>::Delay { ticks },
                }
            }

            pub fn step_expect_no_traffic<$item_ty>(
                ports: &[Port],
                ticks: u64,
            ) -> Step<$item_ty, $expected_ty> {
                Step::<$item_ty, $expected_ty>::Action {
                    ports: ports.to_vec(),
                    action: Action::<$item_ty, $expected_ty>::ExpectNoTraffic { ticks },
                }
            }

            pub fn action_expect_no_traffic<$item_ty>(
                ticks: u64,
            ) -> Action<$item_ty, $expected_ty> {
                Action::<$item_ty, $expected_ty>::ExpectNoTraffic { ticks }
            }

            pub fn action_delay<$item_ty>(
                ticks: u64,
            ) -> Action<$item_ty, $expected_ty> {
                Action::<$item_ty, $expected_ty>::Delay { ticks }
            }

            pub fn step_parallel<$item_ty>(
                sections: std::collections::HashMap<
                    Port,
                    Vec<Action<$item_ty, $expected_ty>>,
                >,
            ) -> Step<$item_ty, $expected_ty> {
                Step::<$item_ty, $expected_ty>::Parallel { sections }
            }
        }
    };
}
