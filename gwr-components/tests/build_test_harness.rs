// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use std::marker::PhantomData;
use std::rc::Rc;

use gwr_components::build_component_harness;
use gwr_engine::port::PortStateResult;
use gwr_engine::traits::SimObject;
use gwr_engine::types::SimResult;

#[allow(dead_code)]
struct HarnessPortShapes<T>(PhantomData<T>);

#[allow(dead_code)]
impl<T> HarnessPortShapes<T>
where
    T: SimObject,
{
    pub fn port_rx(&self) -> PortStateResult<T> {
        unimplemented!("compile-only harness shape")
    }

    pub fn connect_port_tx(&self, _port_state: PortStateResult<T>) -> SimResult {
        unimplemented!("compile-only harness shape")
    }

    pub fn port_indexed_rx_i(&self, _index: usize) -> PortStateResult<T> {
        unimplemented!("compile-only harness shape")
    }

    pub fn connect_port_indexed_tx_i(
        &self,
        _index: usize,
        _port_state: PortStateResult<T>,
    ) -> SimResult {
        unimplemented!("compile-only harness shape")
    }
}

mod rx_only {
    use super::*;

    build_component_harness! {
        harness RxOnlyShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx ports: {
                Rx<T> => rx
            },
        }
    }
}

mod tx_only {
    use super::*;

    build_component_harness! {
        harness TxOnlyShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            tx ports: {
                Tx<T> => tx
            },
        }
    }
}

mod rx_array_only {
    use super::*;

    build_component_harness! {
        harness RxArrayOnlyShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx port arrays: {
                Rx<T> => indexed_rx {
                    count: num_rx
                }
            },
        }
    }
}

mod tx_array_only {
    use super::*;

    build_component_harness! {
        harness TxArrayOnlyShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            tx port arrays: {
                Tx<T> => indexed_tx {
                    count: num_tx
                }
            },
        }
    }
}

mod rx_tx {
    use super::*;

    build_component_harness! {
        harness RxTxShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
        }
    }
}

mod rx_rx_array {
    use super::*;

    build_component_harness! {
        harness RxRxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx ports: {
                Rx<T> => rx
            },
            rx port arrays: {
                IndexedRx<T> => indexed_rx {
                    count: num_rx
                }
            },
        }
    }
}

mod rx_tx_array {
    use super::*;

    build_component_harness! {
        harness RxTxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx port arrays: {
                IndexedTx<T> => indexed_tx {
                    count: num_tx
                }
            },
        }
    }
}

mod tx_rx_array {
    use super::*;

    build_component_harness! {
        harness TxRxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            tx ports: {
                Tx<T> => tx
            },
            rx port arrays: {
                IndexedRx<T> => indexed_rx {
                    count: num_rx
                }
            },
        }
    }
}

mod tx_tx_array {
    use super::*;

    build_component_harness! {
        harness TxTxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            tx ports: {
                Tx<T> => tx
            },
            tx port arrays: {
                IndexedTx<T> => indexed_tx {
                    count: num_tx
                }
            },
        }
    }
}

mod rx_array_tx_array {
    use super::*;

    build_component_harness! {
        harness RxArrayTxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx port arrays: {
                IndexedRx<T> => indexed_rx {
                    count: num_rx
                }
            },
            tx port arrays: {
                IndexedTx<T> => indexed_tx {
                    count: num_tx
                }
            },
        }
    }
}

mod rx_tx_rx_array {
    use super::*;

    build_component_harness! {
        harness RxTxRxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
            rx port arrays: {
                IndexedRx<T> => indexed_rx {
                    count: num_rx
                }
            },
        }
    }
}

mod rx_tx_tx_array {
    use super::*;

    build_component_harness! {
        harness RxTxTxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
            tx port arrays: {
                IndexedTx<T> => indexed_tx {
                    count: num_tx
                }
            },
        }
    }
}

mod rx_rx_array_tx_array {
    use super::*;

    build_component_harness! {
        harness RxRxArrayTxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx ports: {
                Rx<T> => rx
            },
            rx port arrays: {
                IndexedRx<T> => indexed_rx {
                    count: num_rx
                }
            },
            tx port arrays: {
                IndexedTx<T> => indexed_tx {
                    count: num_tx
                }
            },
        }
    }
}

mod tx_rx_array_tx_array {
    use super::*;

    build_component_harness! {
        harness TxRxArrayTxArrayShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            tx ports: {
                Tx<T> => tx
            },
            rx port arrays: {
                IndexedRx<T> => indexed_rx {
                    count: num_rx
                }
            },
            tx port arrays: {
                IndexedTx<T> => indexed_tx {
                    count: num_tx
                }
            },
        }
    }
}

mod all_ports {
    use super::*;

    build_component_harness! {
        harness AllPortShapeHarness<T> {
            component: component: Rc<HarnessPortShapes<T>>,
            rx ports: {
                Rx<T> => rx
            },
            tx ports: {
                Tx<T> => tx
            },
            rx port arrays: {
                IndexedRx<T> => indexed_rx {
                    count: num_rx
                }
            },
            tx port arrays: {
                IndexedTx<T> => indexed_tx {
                    count: num_tx
                }
            },
        }
    }
}
