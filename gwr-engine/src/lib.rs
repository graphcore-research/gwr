// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

// TODO: enable this warning to ensure all public interfaces are documented.
// Enable warnings for missing documentation
// #![warn(missing_docs)]

#![doc(test(attr(deny(unused_must_use))))]
#![doc = std::include_str!(concat!(env!("OUT_DIR"), "/crate-docs.md"))]

pub mod engine;
pub mod events;
pub mod executor;
#[cfg(feature = "global_allocator")]
mod global_allocator;
pub mod port;
pub mod test_helpers;
pub mod time;
pub mod traits;
pub mod types;

/// Spawn all component run() functions and then run the simulation.
#[macro_export]
macro_rules! run_simulation {
    ($engine:ident) => {
        $engine.run().unwrap();
    };
    ($engine:ident, $expect:expr) => {
        match $engine.run() {
            Ok(()) => panic!("Expected an error!"),
            Err(e) => assert_eq!(&format!("{e}"), $expect),
        }
    };
}

/// Spawn a sub-component that is stored in an `RefCell<Option<>>`
///
/// This removes the sub-component from the Option and then spawns the `run()`
/// function.
#[macro_export]
macro_rules! spawn_subcomponent {
    ($($spawner:ident).+ ; $($block:ident).+) => {
        let sub_block = $($block).+.borrow_mut().take().unwrap();
        $($spawner).+.spawn(async move { sub_block.run().await } );
    };
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use async_trait::async_trait;
    use gwr_track::tracker::dev_null_tracker;

    use crate::engine::Engine;
    use crate::traits::Runnable;
    use crate::types::SimResult;

    struct TestComponent {
        ran: Rc<Cell<bool>>,
    }

    #[async_trait(?Send)]
    impl Runnable for TestComponent {
        async fn run(&self) -> SimResult {
            self.ran.set(true);
            Ok(())
        }
    }

    #[test]
    fn spawn_subcomponent_spawns_and_runs_component() {
        let tracker = dev_null_tracker();
        let mut engine = Engine::new(&tracker);
        let spawner = engine.spawner();
        let ran = Rc::new(Cell::new(false));
        let component = RefCell::new(Some(TestComponent { ran: ran.clone() }));

        spawn_subcomponent!(spawner; component);

        engine.run().unwrap();

        assert!(ran.get());
        assert!(component.borrow().is_none());
    }
}
