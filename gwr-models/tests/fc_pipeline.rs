// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use gwr_components::flow_controls::limiter::Limiter;
use gwr_components::sink::Sink;
use gwr_components::source::Source;
use gwr_components::{connect_port, option_box_repeat, rc_limiter};
use gwr_engine::run_simulation;
use gwr_engine::test_helpers::start_test;
use gwr_models::fc_pipeline::{FcPipeline, FcPipelineConfig};
use gwr_track::entity::GetEntity;

fn test_fc_pipeline(buffer_size: usize, data_delay: usize, credit_delay: usize) {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();

    let num_puts = 10;

    // Create a pair of tasks that use a pipeline
    let top = engine.top();
    let source =
        Source::new_and_register(&engine, top, "source", option_box_repeat!(1 ; num_puts)).unwrap();
    let pipe_config = FcPipelineConfig::new(buffer_size, data_delay, credit_delay);
    let pipeline =
        FcPipeline::new_and_register(&engine, &clock, top, "pipe", &pipe_config).unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(source, tx => pipeline, rx).unwrap();
    connect_port!(pipeline, tx => sink, rx).unwrap();

    run_simulation!(engine);

    assert_eq!(sink.num_sunk(), num_puts);
}

#[test]
fn matched() {
    test_fc_pipeline(1, 1, 1);
    test_fc_pipeline(10, 10, 10);
}

#[test]
fn long_credit() {
    test_fc_pipeline(1, 1, 10);
}

#[test]
fn long_data() {
    test_fc_pipeline(1, 10, 1);
}

#[test]
fn instant_credit() {
    test_fc_pipeline(1, 1, 0);
}

#[test]
fn instant_data() {
    test_fc_pipeline(1, 0, 1);
}

#[test]
fn both_instant() {
    test_fc_pipeline(1, 0, 0);
}

fn test_fc_pipeline_throughput(
    buffer_size: usize,
    data_delay: usize,
    credit_delay: usize,
    num_puts: usize,
) -> usize {
    let mut engine = start_test(file!());
    let clock = engine.default_clock();
    let top = engine.top();

    // Set the rate limit such that each packet sent will take one cycle
    let bits_per_tick = 128;
    let rate_limiter = rc_limiter!(&clock, bits_per_tick);
    let limiter = Limiter::new_and_register(&engine, &clock, top, "limiter", rate_limiter).unwrap();

    // Create a pair of tasks that use a pipeline
    let source = Source::new_and_register(&engine, top, "source", None).unwrap();
    let word = 101;
    source.set_generator(option_box_repeat!(word ; num_puts));

    let pipe_config = FcPipelineConfig::new(buffer_size, data_delay, credit_delay);
    let pipeline =
        FcPipeline::new_and_register(&engine, &clock, top, "pipe", &pipe_config).unwrap();
    let sink = Sink::new_and_register(&engine, &clock, top, "sink").unwrap();

    connect_port!(source, tx => limiter, rx).unwrap();
    connect_port!(limiter, tx => pipeline, rx).unwrap();
    connect_port!(pipeline, tx => sink, rx).unwrap();

    run_simulation!(engine);
    assert_eq!(sink.num_sunk(), num_puts);

    clock.tick_now().tick() as usize
}

#[test]
fn throughput() {
    let num_puts = 10;
    let clock_tick = test_fc_pipeline_throughput(1, 0, 0, num_puts);
    assert_eq!(clock_tick, num_puts);

    let clock_tick = test_fc_pipeline_throughput(1, 1, 1, num_puts);
    assert_eq!(clock_tick, num_puts * 2);

    let data_delay = 1;
    let clock_tick = test_fc_pipeline_throughput(2, data_delay, 1, num_puts);
    assert_eq!(clock_tick, num_puts + data_delay);

    let clock_tick = test_fc_pipeline_throughput(1, 1, 2, num_puts);
    assert_eq!(clock_tick, num_puts * 3);
}
