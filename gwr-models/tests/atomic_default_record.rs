// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Old APIs only: migrate and run this recorder BEFORE the atomic module.
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort};
use gwr_engine::traits::Routable;
use gwr_models::memory::memory_access::MemoryAccess;
use gwr_models::memory::{Memory, MemoryConfig};
use gwr_models::test_helpers::{
    create_default_memory_map, create_read, create_write, create_write_np,
};
type RecordedRun = (
    Vec<(u64, usize, String)>,
    usize,
    usize,
    Option<(u64, u64, usize)>,
);

fn run(enabled: bool) -> RecordedRun {
    let mut engine = Engine::new(&gwr_track::tracker::dev_null_tracker());
    let clock = engine.default_clock();
    let old = MemoryConfig::new(0x80000, 65536, 8, 3);
    let config = if enabled {
        old.with_arrival_aware_service(2)
    } else {
        old
    };
    let memory = Memory::new_and_register(&engine, &clock, engine.top(), "memory", config).unwrap();
    let mut tx = OutPort::new(engine.top(), "source");
    tx.connect(memory.port_rx()).unwrap();
    let mut rx = InPort::<MemoryAccess>::new(&engine, &clock, engine.top(), "sink");
    memory.connect_port_tx(rx.state()).unwrap();
    let map = Rc::new(create_default_memory_map());
    let packets = [
        create_read(engine.top(), &map, 4, 0x80000, 0x81000, 0),
        create_write(engine.top(), &map, 4, 0x80004, 0x81000, 0),
        create_write_np(engine.top(), &map, 4, 0x80008, 0x81000, 0),
    ];
    engine.spawner().spawn(async move {
        for p in packets {
            tx.put(p)?.await;
        }
        Ok(())
    });
    let rows = Rc::new(RefCell::new(Vec::new()));
    let r = rows.clone();
    let c = clock.clone();
    engine.spawner().spawn(async move {
        for _ in 0..2 {
            let p = rx.get()?.await;
            r.borrow_mut().push((
                c.tick_now().tick(),
                gwr_models::memory::traits::AccessMemory::access_size_bytes(&p),
                p.access_type().to_string(),
            ));
        }
        Ok(())
    });
    engine.run().unwrap();
    let row = rows.borrow().clone();
    let s = memory
        .service_snapshot()
        .map(|s| (s.accepted_requests, s.retired_requests, s.total));
    (row, memory.bytes_read(), memory.bytes_written(), s)
}
#[test]
#[ignore = "explicit fresh before/after directory, old APIs only"]
fn record_actual_ordinary_memory_default_traces() {
    let out = std::path::PathBuf::from(
        std::env::var_os("GWR_ATOMIC_DEFAULT_OUT").expect("fresh absolute output"),
    );
    assert!(out.is_absolute());
    std::fs::create_dir(&out).unwrap();
    let rows = [run(false), run(true)];
    assert_eq!(rows, [run(false), run(true)]);
    assert!(rows.iter().all(|r| r.1 == 4 && r.2 == 8));
    let bytes = yaml_serde::to_string(&rows).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(out.join("ordinary-memory.yaml"))
        .unwrap();
    f.write_all(bytes.as_bytes()).unwrap();
}
