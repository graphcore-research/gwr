// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::rc::Rc;

use gwr_components::build_component_harness;
use gwr_components::delay::Delay;

build_component_harness! {
    harness DelayHarness<T> {
        component: delay: Rc<Delay<T>>,
        rx ports: { Rx<T> => rx },
        tx ports: { Tx<T> => tx },
    }
}

fn main() {
    let _: Step<i32> = script! {
        ports { input = UnknownPort }
        async {
            input.put(7)?.await;
            Ok(())
        }
    };
}
