// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Helper connection macros

#[doc(hidden)]
pub use paste::paste;

/// Connect an [OutPort](gwr_engine::port::OutPort) port to an
/// [InPort](gwr_engine::port::InPort)
#[macro_export]
macro_rules! connect_port {
    ($from:expr, $from_port_name:ident => $to:expr, $to_port_name:ident) => {
        {
            use gwr_track::entity::GetEntity;
            gwr_track::debug!($from.entity() ; "Connect {}.{} => {}.{}", $from, stringify!($from_port_name), $to, stringify!($to_port_name));
            $crate::connect::paste! {
                $from.[< connect_port_ $from_port_name >]($to.[< port_ $to_port_name >]())
            }
        }
    };
    ($from:expr, $from_port_name:ident, $from_index:expr => $to:expr, $to_port_name:ident) => {
        {
            use gwr_track::entity::GetEntity;
            let from_index: usize = $from_index;
            gwr_track::debug!($from.entity() ; "Connect {}.{}[{}] => {}.{}", $from, stringify!($from_port_name), from_index, $to, stringify!($to_port_name));
            $crate::connect::paste! {
                $from.[< connect_port_ $from_port_name _i >](from_index, $to.[< port_ $to_port_name >]())
            }
        }
    };
    ($from:expr, $from_port_name:ident => $to:expr, $to_port_name:ident, $to_index:expr) => {
        {
            use gwr_track::entity::GetEntity;
            let to_index: usize = $to_index;
            gwr_track::debug!($from.entity() ; "Connect {}.{} => {}.{}[{}]", $from, stringify!($from_port_name), $to, stringify!($to_port_name), to_index);
            $crate::connect::paste! {
                $from.[< connect_port_ $from_port_name >]($to.[< port_ $to_port_name _i >](to_index))
            }
        }
    };
    ($from:expr, $from_port_name:ident, $from_index:expr => $to:expr, $to_port_name:ident, $to_index:expr) => {
        {
            use gwr_track::entity::GetEntity;
            let from_index: usize = $from_index;
            let to_index: usize = $to_index;
            gwr_track::debug!($from.entity() ; "Connect {}.{}[{}] => {}.{}[{}]", $from, stringify!($from_port_name), from_index, $to, stringify!($to_port_name), to_index);
            $crate::connect::paste! {
                $from.[< connect_port_ $from_port_name _i >](from_index, $to.[< port_ $to_port_name _i >](to_index))
            }
        }
    };
}

/// Create and connect a dummy RX port
#[macro_export]
macro_rules! connect_dummy_rx {
    ($from:expr, $from_port_name:ident => $engine:expr, $clock:expr, $entity:expr) => {
        {
            use gwr_track::entity::GetEntity;
            let rx_port = gwr_engine::port::InPort::new($engine, $clock, $entity, "dummy");

            gwr_track::debug!($from.entity() ; "Connect {}.{} => {}", $from, stringify!($from_port_name), rx_port);
            $crate::connect::paste! {
                $from.[< connect_port_ $from_port_name >](rx_port.state())
            }
        }
    };
    ($from:expr, $from_port_name:ident, $from_index:expr => $engine:expr, $clock:expr, $entity:expr) => {
        {
            use gwr_track::entity::GetEntity;
            let rx_port = gwr_engine::port::InPort::new($engine, $clock, $entity, "dummy");

            let from_index: usize = $from_index;
            gwr_track::debug!($from.entity() ; "Connect {}.{}[{}] => {}", $from, stringify!($from_port_name), from_index, rx_port);
            $crate::connect::paste! {
                $from.[< connect_port_ $from_port_name _i >](from_index, rx_port.state())
            }
        }
    };
}

/// Create and connect a dummy TX port
#[macro_export]
macro_rules! connect_dummy_tx {
    ($entity:expr => $to:expr, $to_port_name:ident) => {
        {
            let mut tx_port = gwr_engine::port::OutPort::new($entity, "dummy");

            gwr_track::debug!($entity ; "Connect {} => {}.{}", tx_port, $to, stringify!($to_port_name));
            $crate::connect::paste! {
                tx_port.connect($to.[< port_ $to_port_name >]())
            }
        }
    };
    ($entity:expr => $to:expr, $to_port_name:ident, $to_index:expr) => {
        {
            let mut tx_port = gwr_engine::port::OutPort::new($entity, "dummy");

            let to_index: usize = $to_index;
            gwr_track::debug!($entity ; "Connect {} => {}.{}[{}]", tx_port, $to, stringify!($to_port_name), to_index);
            $crate::connect::paste! {
                tx_port.connect($to.[< port_ $to_port_name _i >](to_index))
            }
        }
    };
}

/// Connect a tx port for a subcomponent.
///
/// The subcomponent is expected to be stored in a `RefCell<Option<>>`
#[macro_export]
macro_rules! connect_tx {
    ($component:expr, $fn:ident ; $port_state:ident) => {
        $crate::connect::paste! {
            $component
                .borrow_mut()
                .as_mut()
                .unwrap()
                .$fn($port_state)
        }
    };
}

/// Connect a tx port for a subcomponent where the port is one of an array.
///
/// The subcomponent is expected to be stored in a `RefCell<Option<>>`
#[macro_export]
macro_rules! connect_tx_i {
    ($component:expr, $fn:ident, $index:expr ; $port_state:ident) => {
        $component
            .borrow_mut()
            .as_mut()
            .unwrap()
            .$fn($index, $port_state)
    };
}

/// Access rx port for a subcomponent.
///
/// The subcomponent is expected to be stored in a `RefCell<Option<>>`
#[macro_export]
macro_rules! port_rx {
    ($component:expr, $fn:ident) => {
        $component.borrow().as_ref().unwrap().$fn()
    };
}

/// Access an individual index of an rx port array for a subcomponent.
///
/// The subcomponent is expected to be stored in a `RefCell<Option<>>`
#[macro_export]
macro_rules! port_rx_i {
    ($component:expr, $fn:ident, $index:expr) => {
        $component.borrow().as_ref().unwrap().$fn($index)
    };
}

/// Get a reference to a variable stored in a `RefCell<Option<>>`.
#[macro_export]
macro_rules! borrow_option {
    ($var:expr) => {
        $var.borrow().as_ref().unwrap()
    };
}

/// Get a mutable reference to a variable stored in a `RefCell<Option<>>`.
#[macro_export]
macro_rules! borrow_option_mut {
    ($var:expr) => {
        $var.borrow_mut().as_mut().unwrap()
    };
}

/// Take a variable out of a `RefCell<Option<>>`.
#[macro_export]
macro_rules! take_option {
    ($var:expr) => {
        $var.borrow_mut().take().unwrap()
    };
}
