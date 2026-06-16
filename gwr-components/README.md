<!-- Copyright (c) 2023 Graphcore Ltd. All rights reserved. -->

# gwr-components

<!-- ANCHOR: intro -->

Simulation components are the basic building blocks of any GWR model.

The GWR [`Engine`] runs components that are connected together using [`ports`].

The `gwr_components` library provides a collection of connectable component
primitives to be used when building models.

<!-- ANCHOR_END: intro -->

[`Engine`]: ../gwr-engine/README.md

<!-- ANCHOR: testing -->

## Testing

Components can be tested by connecting them into a small simulation and driving
their ports directly. For simple cases this can be done by hand with
`OutPort`/`InPort`, but most component tests need the same testbench structure:

1. Create an engine and the device under test (DUT).
1. Connect driver ports to DUT input ports.
1. Connect receiver ports to DUT output ports.
1. Run a sequence of sends, expects, delays, and no-traffic checks.

The `build_component_harness!` macro will generate the repeated testbench code.
It generates the harness `struct`, `Port`/`Step` enums, helper functions, etc.

Harnesses are usually declared inside a small test module. This keeps generated
names such as `Port`, `Step`, `step_send_rx`, and `step_expect_tx` local to the
harness and avoids clashes with other harnesses in the same test file.

For example, the harness around a `Delay` component is created and used below:

```rust,no_run
mod delay_harness {
    use std::rc::Rc;

    use gwr_components::build_component_harness;
    use gwr_components::delay::Delay;
    use gwr_engine::test_helpers::start_test;

    build_component_harness! {
        harness DelayHarness<T> {
            component: delay: Rc<Delay<T>>,
            rx ports: {
                Rx<T> => rx,
            },
            tx ports: {
                Tx<T> => tx,
            },
        }
    }

    #[test]
    fn delay_forwards_values() {
        let mut engine = start_test(file!());
        let clock = engine.default_clock();
        let delay = Delay::new_and_register(&engine, &clock, engine.top(), "delay", 5).unwrap();
        let mut harness = DelayHarness::new(engine, delay);

        harness.run_steps([
            step_send_rx(10),
            step_expect_no_traffic(&[Port::Tx], 4),
            step_expect_tx(10),
        ]);
    }
}
```

The macro supports scalar RX/TX ports and RX/TX port arrays. Each port section
is optional, so a source-only component can define only `tx ports` and a
sink-only component can define only `rx ports`.

`Step` can be a send, expect, delay, no-traffic check, `Seq(Vec<Step<...>>)`
that runs child steps in order, or `Par(Vec<Step<...>>)` that runs child steps
concurrently and waits for all of them before moving on. The `step_seq` and
`step_par` helpers build those recursive control structures, so tests can
express parallel sequences on different ports.

The harness checks that each step is used on a compatible port; for example,
using an expect step on an RX port or a send step on a TX port will fail the
test.

Use `run_steps([Step<...>])` for fixed test sequences and
`run_step_generator(iterator)` for stateful generators that yield steps as the
test progresses.

<!-- ANCHOR_END: testing -->

[`ports`]: ../gwr-developer-guide/md_src/components/ports.md

## Further reading

The [components] chapter of the gwr-developer-guide contains details on the
creation and use of new components.

[components]:
  ../gwr-developer-guide/md_src/components/chapter.md#creating-new-components
