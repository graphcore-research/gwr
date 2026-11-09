// Copyright (c) 2026 Graphcore Ltd. All rights reserved.
//! Independent routed mesh planes with bounded directional service and address
//! affinity.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::rc::Rc;

use async_trait::async_trait;
use gwr_components::arbiter::Arbiter;
use gwr_components::arbiter::policy::RoundRobin;
use gwr_engine::engine::Engine;
use gwr_engine::port::{InPort, OutPort, PortStateResult};
use gwr_engine::time::clock::Clock;
use gwr_engine::traits::{Routable, Runnable, SimObject, TotalBytes};
use gwr_engine::types::{SimError, SimResult};
use gwr_model_builder::{EntityDisplay, EntityGet};
use gwr_track::Id;
use gwr_track::entity::Entity;
use gwr_track::id::Unique;

use super::node::FabricRoutingAlgorithm;
use super::paths::{FabricAddressKey, FabricPathConfig, FabricPathSelector};
use super::service::{ForwardingTiming, RouteService, RouteServiceSnapshot};
use super::{Fabric, FabricConfig, FabricPortSelection};

/// Extract the canonical final-memory identity and offset from a protocol
/// packet.
pub type FabricAddressProvider<T> = Rc<dyn Fn(&T) -> Result<FabricAddressKey, SimError>>;

#[derive(Clone, Debug)]
struct Transit<T> {
    payload: T,
    route: Rc<Vec<usize>>,
    cursor: usize,
    timing: ForwardingTiming,
}
impl<T: SimObject> Unique for Transit<T> {
    fn id(&self) -> Id {
        self.payload.id()
    }
}
impl<T: SimObject> TotalBytes for Transit<T> {
    fn payload_bytes(&self) -> Option<usize> {
        self.payload.payload_bytes()
    }

    fn total_bytes(&self) -> usize {
        self.payload.total_bytes()
    }
}
impl<T: SimObject> fmt::Display for Transit<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.payload, f)
    }
}
impl<T: SimObject> SimObject for Transit<T> {}
struct Relay<T: SimObject> {
    rx: InPort<Transit<T>>,
    output: Option<OutPort<T>>,
}

/// Independently queued mesh planes. External ports add no bandwidth limit.
#[derive(EntityGet, EntityDisplay)]
pub struct RoutedFabric<T: SimObject + Routable> {
    entity: Rc<Entity>,
    clock: Clock,
    config: Rc<FabricConfig>,
    paths: FabricPathConfig,
    algorithm: FabricRoutingAlgorithm,
    key: FabricAddressProvider<T>,
    selector: Rc<dyn FabricPathSelector>,
    ingress: HashMap<usize, Rc<RouteService<T>>>,
    egress: HashMap<usize, Rc<RouteService<T>>>,
    inputs: RefCell<Vec<(usize, InPort<T>)>>,
    relays: RefCell<Vec<Relay<T>>>,
    services: Rc<Vec<Rc<RouteService<Transit<T>>>>>,
    edges: Rc<BTreeMap<(usize, usize, usize), usize>>,
    deliveries: Rc<HashMap<(usize, usize), usize>>,
}
impl<T: SimObject + Routable> RoutedFabric<T> {
    /// Construct with explicit path service and a caller-supplied address-only
    /// hash.
    #[expect(clippy::too_many_arguments)]
    #[expect(
        clippy::too_many_lines,
        reason = "Register all route directions atomically before publishing the fabric"
    )]
    pub fn new_and_register_with_paths(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: Rc<FabricConfig>,
        algorithm: FabricRoutingAlgorithm,
        paths: FabricPathConfig,
        key: FabricAddressProvider<T>,
        selector: Rc<dyn FabricPathSelector>,
    ) -> Result<Rc<Self>, SimError> {
        paths.validate()?;
        if config.port_selection() == FabricPortSelection::SourceIdModulo {
            return Err(SimError(
                "source-dependent gateway selection cannot preserve routed address affinity".into(),
            ));
        }
        config
            .max_num_ports()
            .checked_mul(paths.count)
            .ok_or_else(|| SimError("fabric plane geometry overflows".into()))?;
        let entity = Rc::new(Entity::new(parent, name));
        let mut services = Vec::new();
        let mut relays = Vec::new();
        let mut edges = BTreeMap::new();
        let mut deliveries = HashMap::new();
        let mut ingress = HashMap::new();
        let mut egress = HashMap::new();
        let mut inputs = Vec::new();
        let mut mergers = HashMap::new();
        for &port in config.port_indices() {
            let rx_queue = RouteService::new_queue_and_register(
                engine,
                clock,
                &entity,
                &format!("ingress_{port}"),
                config.rx_buffer_bytes,
                paths.queue_packets,
            )?;
            let input = InPort::new(engine, clock, &entity, &format!("classify_{port}"));
            rx_queue.connect_port_tx(input.state())?;
            ingress.insert(port, rx_queue);
            inputs.push((port, input));
            let merge = Arbiter::new_and_register(
                engine,
                clock,
                &entity,
                &format!("merge_{port}"),
                paths.count,
                Box::new(RoundRobin::new()),
            );
            let tx_queue = RouteService::new_queue_and_register(
                engine,
                clock,
                &entity,
                &format!("egress_{port}"),
                config.tx_buffer_bytes,
                paths.queue_packets,
            )?;
            merge.connect_port_tx(tx_queue.port_rx())?;
            egress.insert(port, tx_queue);
            mergers.insert(port, merge);
        }
        for plane in 0..paths.count {
            let plane_entity = Rc::new(Entity::new(&entity, &format!("plane_{plane}")));
            let mut add = |name: String,
                           delay: usize,
                           output: Option<OutPort<T>>|
             -> Result<usize, SimError> {
                let service = RouteService::new_forwarding_and_register(
                    engine,
                    clock,
                    &plane_entity,
                    &name,
                    paths.bw_bits_per_tick,
                    delay,
                    paths.queue_bytes,
                    paths.queue_packets,
                    |packet: &mut Transit<T>| &mut packet.timing,
                )?;
                gwr_components::connect_dummy_tx!(&plane_entity => service, rx)?;
                let rx = InPort::new(engine, clock, &plane_entity, &format!("forward_{name}"));
                service.connect_port_tx(rx.state())?;
                let id = services.len();
                services.push(service);
                relays.push(Relay { rx, output });
                Ok(id)
            };
            for x in 0..config.num_columns() {
                for y in 0..config.num_rows() {
                    let from = x * config.num_rows() + y;
                    for (nx, ny) in [
                        (x.checked_sub(1), Some(y)),
                        (
                            x.checked_add(1).filter(|v| *v < config.num_columns()),
                            Some(y),
                        ),
                        (Some(x), y.checked_sub(1)),
                        (Some(x), y.checked_add(1).filter(|v| *v < config.num_rows())),
                    ] {
                        if let (Some(nx), Some(ny)) = (nx, ny) {
                            let to = nx * config.num_rows() + ny;
                            let id =
                                add(format!("{x}_{y}_to_{nx}_{ny}"), config.ticks_per_hop, None)?;
                            edges.insert((plane, from, to), id);
                        }
                    }
                }
            }
            for &port in config.port_indices() {
                let mut output = OutPort::new(&plane_entity, &format!("deliver_{port}"));
                output.connect(mergers[&port].port_rx_i(plane))?;
                let id = add(format!("local_{port}"), config.ticks_overhead, Some(output))?;
                deliveries.insert((plane, port), id);
            }
        }
        let fabric = Rc::new(Self {
            entity,
            clock: clock.clone(),
            config,
            paths,
            algorithm,
            key,
            selector,
            ingress,
            egress,
            inputs: RefCell::new(inputs),
            relays: RefCell::new(relays),
            services: Rc::new(services),
            edges: Rc::new(edges),
            deliveries: Rc::new(deliveries),
        });
        engine.register(fabric.clone());
        Ok(fabric)
    }
    /// Construct a single-plane fabric using the supplied common configuration.
    /// Multi-plane protocol clients should supply a canonical address provider.
    pub fn new_and_register(
        engine: &Engine,
        clock: &Clock,
        parent: &Rc<Entity>,
        name: &str,
        config: Rc<FabricConfig>,
        algorithm: FabricRoutingAlgorithm,
    ) -> Result<Rc<Self>, SimError> {
        let paths = config.paths.clone().unwrap_or_default();
        if paths.count != 1 {
            return Err(SimError(
                "multiple fabric paths require an explicit canonical address provider".into(),
            ));
        }
        let selector = Rc::new(paths.selection);
        Self::new_and_register_with_paths(
            engine,
            clock,
            parent,
            name,
            config,
            algorithm,
            paths,
            Rc::new(|p: &T| {
                Ok(FabricAddressKey {
                    memory_index: p.dst_device().0,
                    offset_in_memory: p.dst_addr(),
                })
            }),
            selector,
        )
    }
    /// Observe every directed route, ordered by plane, edge, and local
    /// delivery.
    pub fn route_snapshots(&self) -> Vec<RouteServiceSnapshot> {
        let mut snapshots: Vec<_> = self.services.iter().map(|s| s.snapshot()).collect();
        for port in self.config.port_indices() {
            snapshots.push(self.ingress[port].snapshot());
            snapshots.push(self.egress[port].snapshot());
        }
        snapshots
    }
}

fn route(
    config: &FabricConfig,
    algorithm: FabricRoutingAlgorithm,
    edges: &BTreeMap<(usize, usize, usize), usize>,
    deliveries: &HashMap<(usize, usize), usize>,
    plane: usize,
    source: usize,
    destination: usize,
) -> Result<Vec<usize>, SimError> {
    let (mut x, mut y, _) = config.fabric_port_index_to_col_row_port(source);
    let (dx, dy, _) = config.fabric_port_index_to_col_row_port(destination);
    let mut hops = Vec::new();
    while x != dx || y != dy {
        let from = x * config.num_rows() + y;
        if x != dx && (y == dy || algorithm == FabricRoutingAlgorithm::ColumnFirst) {
            if x < dx { x += 1 } else { x -= 1 }
        } else if y < dy {
            y += 1;
        } else {
            y -= 1;
        }
        let to = x * config.num_rows() + y;
        hops.push(
            *edges
                .get(&(plane, from, to))
                .ok_or_else(|| SimError("missing directed fabric route".into()))?,
        );
    }
    hops.push(
        *deliveries
            .get(&(plane, destination))
            .ok_or_else(|| SimError("unpopulated fabric destination".into()))?,
    );
    Ok(hops)
}
#[async_trait(?Send)]
impl<T: SimObject + Routable> Runnable for RoutedFabric<T> {
    async fn run(&self) -> SimResult {
        let forwards = self
            .relays
            .borrow_mut()
            .drain(..)
            .map(|mut relay| {
                let services = self.services.clone();
                let clock = self.clock.clone();
                let rate = self.paths.bw_bits_per_tick;
                async move {
                    loop {
                        let mut packet = relay.rx.start_get()?.await;
                        packet.cursor += 1;
                        if packet.cursor < packet.route.len() {
                            services[packet.route[packet.cursor]]
                                .enqueue(packet)
                                .await?;
                        } else {
                            // Protocol clients receive whole packets, never a
                            // head whose payload is still in flight.
                            let tail = packet.timing.tail_tick(rate)?;
                            clock
                                .wait_ticks(tail.saturating_sub(clock.tick_now().tick()))?
                                .await;
                            relay
                                .output
                                .as_mut()
                                .expect("local route has recipient")
                                .put(packet.payload)?
                                .await;
                        }
                        relay.rx.finish_get();
                    }
                    #[allow(unreachable_code)]
                    Ok::<(), SimError>(())
                }
            })
            .collect::<Vec<_>>();
        let inputs = self
            .inputs
            .borrow_mut()
            .drain(..)
            .map(|(source, mut input)| {
                let config = self.config.clone();
                let key = self.key.clone();
                let selector = self.selector.clone();
                let count = self.paths.count;
                let rate = self.paths.bw_bits_per_tick;
                let clock = self.clock.clone();
                let shift = self.paths.selection.address_shift();
                let edges = self.edges.clone();
                let deliveries = self.deliveries.clone();
                let algorithm = self.algorithm;
                let services = self.services.clone();
                async move {
                    loop {
                        let payload = input.start_get()?.await;
                        let address = key(&payload)?;
                        let plane = selector.select(address, count)?;
                        if plane >= count {
                            return Err(SimError(
                                "fabric selector returned an invalid path index".into(),
                            ));
                        }
                        let destination = config.resolve_canonical_destination_port(
                            payload.dst_device().0,
                            address,
                            shift,
                        )?;
                        let hops = Rc::new(route(
                            &config,
                            algorithm,
                            &edges,
                            &deliveries,
                            plane,
                            source,
                            destination,
                        )?);
                        let first = hops[0];
                        let arrival = u128::from(clock.tick_now().tick()) * rate as u128;
                        services[first]
                            .enqueue(Transit {
                                payload,
                                route: hops,
                                cursor: 0,
                                timing: ForwardingTiming {
                                    head: arrival,
                                    tail: arrival,
                                },
                            })
                            .await?;
                        input.finish_get();
                    }
                    #[allow(unreachable_code)]
                    Ok::<(), SimError>(())
                }
            })
            .collect::<Vec<_>>();
        futures::try_join!(
            futures::future::try_join_all(forwards),
            futures::future::try_join_all(inputs)
        )?;
        Ok(())
    }
}
impl<T: SimObject + Routable> Fabric<T> for RoutedFabric<T> {
    fn connect_port_egress_i(&self, i: usize, port: PortStateResult<T>) -> SimResult {
        self.egress
            .get(&i)
            .ok_or_else(|| SimError(format!("unpopulated fabric egress {i}")))?
            .connect_port_tx(port)
    }
    fn port_ingress_i(&self, i: usize) -> PortStateResult<T> {
        self.ingress
            .get(&i)
            .ok_or_else(|| SimError(format!("unpopulated fabric ingress {i}")))?
            .port_rx()
    }
    fn col_row_port_to_fabric_port_index(&self, c: usize, r: usize, p: usize) -> usize {
        self.config.col_row_port_to_fabric_port_index(c, r, p)
    }
    fn fabric_port_index_to_col_row_port(&self, i: usize) -> (usize, usize, usize) {
        self.config.fabric_port_index_to_col_row_port(i)
    }
    fn destination_port_map(&self) -> &HashMap<u64, Vec<usize>> {
        self.config.destination_port_map()
    }
    fn port_selection(&self) -> FabricPortSelection {
        self.config.port_selection()
    }
    fn route_snapshots(&self) -> Vec<RouteServiceSnapshot> {
        self.route_snapshots()
    }
}
