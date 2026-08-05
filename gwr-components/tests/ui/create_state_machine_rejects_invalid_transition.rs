// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

gwr_components::create_state_machine!(
    JobMachine {
        states: [Queued, Running, Complete, Failed],
        default: Queued,
        transitions: [
            start: [Queued] => Running,
            finish: [Running] => Complete,
            fail: [Queued, Running] => Failed,
            retry: [Failed] => Queued,
        ],
    }
);

fn main() {
    let queued = JobMachineTypestate::default();
    let complete = queued.finish();

    let _ = complete;
}
