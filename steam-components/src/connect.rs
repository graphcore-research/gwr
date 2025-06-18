// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Helper connection macros

pub use paste::paste;

#[macro_export]
/// Connect an [OutPort](steam_engine::port::OutPort) port to an
/// [InPort](steam_engine::port::InPort)
macro_rules! connect_port {
    ($from:expr, $from_port_name:ident => $to:expr, $to_port_name:ident) => {
        steam_track::debug!($from.entity ; "Connect {}.{} => {}.{}", $from, stringify!($from_port_name), $to, stringify!($to_port_name));
        $crate::connect::paste! {
            $from.[< connect_port_ $from_port_name >]($to.[< port_ $to_port_name >]());
        }
    };
    ($from:expr, $from_port_name:ident, $from_index:expr => $to:expr, $to_port_name:ident) => {
        let from_index: usize = $from_index;
        steam_track::debug!($from.entity ; "Connect {}.{}[{}] => {}.{}", $from, stringify!($from_port_name), from_index, $to, stringify!($to_port_name));
        $crate::connect::paste! {
            $from.[< connect_port_ $from_port_name _i >](from_index, $to.[< port_ $to_port_name >]());
        }
    };
    ($from:expr, $from_port_name:ident => $to:expr, $to_port_name:ident, $to_index:expr) => {
        let to_index: usize = $to_index;
        steam_track::debug!($from.entity ; "Connect {}.{} => {}.{}[{}]", $from, stringify!($from_port_name), $to, stringify!($to_port_name), to_index);
        $crate::connect::paste! {
            $from.[< connect_port_ $from_port_name >]($to.[< port_ $to_port_name _i >](to_index));
        }
    };
    ($from:expr, $from_port_name:ident, $from_index:expr => $to:expr, $to_port_name:ident, $to_index:expr) => {
        let from_index: usize = $from_index;
        let to_index: usize = $to_index;
        steam_track::debug!($from.entity ; "Connect {}.{}[{}] => {}.{}[{}]", $from, stringify!($from_port_name), from_index, $to, stringify!($to_port_name), to_index);
        $crate::connect::paste! {
            $from.[< connect_port_ $from_port_name _i >](from_index, $to.[< port_ $to_port_name _i >](to_index));
        }
    };
}

#[macro_export]
/// Connect a tx port for a subcomponent.
///
/// The subcomponent is expected to be stored in a `RefCell<Option<>>`
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

#[macro_export]
/// Connect a tx port for a subcomponent where the port is one of an array.
///
/// The subcomponent is expected to be stored in a `RefCell<Option<>>`
macro_rules! connect_tx_i {
    ($component:expr, $fn:ident, $index:expr ; $port_state:ident) => {
        $component
            .borrow_mut()
            .as_mut()
            .unwrap()
            .$fn($index, $port_state)
    };
}

#[macro_export]
/// Access rx port for a subcomponent.
///
/// The subcomponent is expected to be stored in a `RefCell<Option<>>`
macro_rules! port_rx {
    ($component:expr, $fn:ident) => {
        $component.borrow().as_ref().unwrap().$fn()
    };
}

#[macro_export]
/// Access an individual index of an rx port array for a subcomponent.
///
/// The subcomponent is expected to be stored in a `RefCell<Option<>>`
macro_rules! port_rx_i {
    ($component:expr, $fn:ident, $index:expr) => {
        $component.borrow().as_ref().unwrap().$fn($index)
    };
}

#[macro_export]
/// Get a reference to a variable stored in a `RefCell<Option<>>`.
macro_rules! borrow_option {
    ($var:expr) => {
        $var.borrow().as_ref().unwrap()
    };
}

#[macro_export]
/// Get a mutable reference to a variable stored in a `RefCell<Option<>>`.
macro_rules! borrow_option_mut {
    ($var:expr) => {
        $var.borrow_mut().as_mut().unwrap()
    };
}

#[macro_export]
/// Take a variable out of a `RefCell<Option<>>`.
macro_rules! take_option {
    ($var:expr) => {
        $var.borrow_mut().take().unwrap()
    };
}
