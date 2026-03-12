// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use gwr_engine::test_helpers::start_test;
use gwr_resources::base::{Resource, ResourceGuard};

#[test]
fn resource_empty() {
    const CAPACITY: usize = 2;
    const NUM_RESOURCE_REQUESTS: usize = 5;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let resource = Resource::new(CAPACITY);

    for i in 0..NUM_RESOURCE_REQUESTS {
        let clock = clock.clone();
        let resource = resource.clone();
        engine.spawn(async move {
            println!("RESOURCE REQUEST {i} start @ {}", clock.tick_now());
            resource.request().await;
            clock.wait_ticks(10).await;
            println!("RESOURCE REQUEST {i} done @ {}", clock.tick_now());
            resource.release().await?;
            println!("RESOURCE RELEASE {i} @ {}", clock.tick_now());
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!(resource.count(), 0);
}

#[test]
#[should_panic(
    expected = "called `Result::unwrap()` on an `Err` value: SimError(\"Invalid release\")"
)]
fn resource_more_releases() {
    const CAPACITY: usize = 2;
    const NUM_RESOURCE_REQUESTS: usize = 3;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let resource = Resource::new(CAPACITY);

    for i in 0..NUM_RESOURCE_REQUESTS {
        let clock = clock.clone();
        let resource = resource.clone();
        engine.spawn(async move {
            println!("RESOURCE REQUEST {i} start @ {}", clock.tick_now());
            resource.request().await;
            clock.wait_ticks(10).await;
            println!("RESOURCE REQUEST {i} done @ {}", clock.tick_now());
            resource.release().await?;
            println!("RESOURCE RELEASE {i} @ {}", clock.tick_now());
            resource.release().await?;
            Ok(())
        });
    }

    engine.run().unwrap();
}

#[test]
fn resource_no_release() {
    const CAPACITY: usize = 2;
    const NUM_RESOURCE_REQUESTS: usize = 5;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let resource = Resource::new(CAPACITY);

    for i in 0..NUM_RESOURCE_REQUESTS {
        let clock = clock.clone();
        let resource = resource.clone();
        engine.spawn(async move {
            println!("RESOURCE REQUEST {i} start @ {}", clock.tick_now());
            resource.request().await;
            clock.wait_ticks(10).await;
            println!("RESOURCE REQUEST {i} done @ {}", clock.tick_now());
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!(resource.count(), CAPACITY);
}

#[test]
fn resource_guard() {
    const CAPACITY: usize = 2;
    const NUM_RESOURCE_REQUESTS: usize = 5;

    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let resource = Resource::new(CAPACITY);

    for i in 0..NUM_RESOURCE_REQUESTS {
        let clock = clock.clone();
        let resource = resource.clone();
        engine.spawn(async move {
            println!("RESOURCE GUARD {i} start @ {}", clock.tick_now());
            ResourceGuard::new(resource).await;
            clock.wait_ticks(10).await;
            println!("RESOURCE GUARD {i} done @ {}", clock.tick_now());
            Ok(())
        });
    }

    engine.run().unwrap();

    assert_eq!(resource.count(), 0);
}
