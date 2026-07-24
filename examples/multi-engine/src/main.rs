// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! This example demonstrates how to run two engines in parallel. Each engine
//! runs in its own thread, exchanges a message with the other engine, and then
//! synchronizes to the later of the two engine times before continuing.
//!
//! Run with:
//!
//! ```bash
//! cargo run -p multi-engine
//! ```

use gwr_engine::engine::Engine;
use gwr_engine::multi_engine::{
    MultiEngineChannel, MultiEngineChannelReceiver, MultiEngineChannelSender, MultiEngineSync,
    MultiEngineSyncParticipant,
};
use gwr_engine::time::clock::Clock;
use gwr_engine::types::{SimError, SimResult};
use gwr_track::tracker::dev_null_tracker;

const WORK_CHUNKS: usize = 2;
const ENGINE_0_WORK_TICKS: u64 = 10;
const ENGINE_1_WORK_TICKS: u64 = 20;
const EXPECTED_FINAL_TIME_NS: f64 = 40.0;

#[derive(Debug, PartialEq, Eq)]
struct EngineMessage {
    from_engine: usize,
    sequence: usize,
}

async fn wait_until(clock: &Clock, time_ns: f64) {
    let now_ns = clock.time_now_ns();
    if time_ns > now_ns {
        let ticks = ((time_ns - now_ns) * clock.freq_mhz() / 1000.0).ceil() as u64;
        clock.wait_ticks(ticks).await;
    }
}

fn run_engine(
    engine_id: usize,
    ticks: u64,
    sync: MultiEngineSyncParticipant,
    tx: MultiEngineChannelSender<EngineMessage>,
    rx: MultiEngineChannelReceiver<EngineMessage>,
) -> Result<f64, SimError> {
    let tracker = dev_null_tracker();
    let mut engine = Engine::new(&tracker);
    let clock = engine.default_clock();

    println!("starting engine {engine_id}");

    let wait_handle = engine.external_wait_handle();

    engine.spawn(async move {
        for sequence in 0..WORK_CHUNKS {
            clock.wait_ticks(ticks).await;

            tx.send(EngineMessage {
                from_engine: engine_id,
                sequence,
            })
            .expect("send should succeed");

            let message = rx
                .recv(wait_handle.clone())
                .await
                .expect("sender should still exist");
            assert_eq!(message.from_engine, 1 - engine_id);
            assert_eq!(message.sequence, sequence);

            let local_time_ns = clock.time_now_ns();
            let synced_time_ns = sync.sync(local_time_ns, wait_handle.clone()).await;
            wait_until(&clock, synced_time_ns).await;
        }

        Ok(())
    });

    engine.run()?;

    Ok(engine.time_now_ns())
}

fn main() -> SimResult {
    let sync = MultiEngineSync::new(2);
    let (tx_0, rx_0) = MultiEngineChannel::channel();
    let (tx_1, rx_1) = MultiEngineChannel::channel();

    let sync_0 = sync.participant(0);
    let sync_1 = sync.participant(1);

    let handles = [
        (
            0,
            std::thread::spawn(move || run_engine(0, ENGINE_0_WORK_TICKS, sync_0, tx_1, rx_0)),
        ),
        (
            1,
            std::thread::spawn(move || run_engine(1, ENGINE_1_WORK_TICKS, sync_1, tx_0, rx_1)),
        ),
    ];

    for (engine_id, handle) in handles {
        let final_time_ns = handle.join().expect("engine thread panicked")?;
        assert_eq!(
            final_time_ns, EXPECTED_FINAL_TIME_NS,
            "engine {engine_id} finished at an unexpected time"
        );
        println!("engine {engine_id} finished at {final_time_ns:.2} ns");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_engine_advances_to_sync_time() {
        let sync_0 = MultiEngineSync::new(2);
        let sync_1 = sync_0.participant(1);
        let sync_0 = sync_0.participant(0);
        let (tx_0, rx_0) = MultiEngineChannel::channel();
        let (tx_1, rx_1) = MultiEngineChannel::channel();

        let handle_0 =
            std::thread::spawn(move || run_engine(0, ENGINE_0_WORK_TICKS, sync_0, tx_1, rx_0));
        let handle_1 =
            std::thread::spawn(move || run_engine(1, ENGINE_1_WORK_TICKS, sync_1, tx_0, rx_1));

        let thread_0_time_ns = handle_0
            .join()
            .expect("engine thread panicked")
            .expect("engine run failed");

        let thread_1_time_ns = handle_1
            .join()
            .expect("engine thread panicked")
            .expect("engine run failed");

        assert_eq!(thread_0_time_ns, thread_1_time_ns);
        assert_eq!(thread_0_time_ns, EXPECTED_FINAL_TIME_NS);
    }
}
