// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Control and Status Registers.

use std::rc::Rc;

use gwr_engine::traits::Resolver;

/// Interface to a [`Register`]
pub trait Register {
    /// Write to a register and trigger `write` callbacks.
    ///
    /// **Note:** the underlying register value won't change until the
    /// `resolver` [`resolve()`](gwr_engine::traits::Resolve)
    /// is called.
    fn write(&self, resolver: &impl Resolver, value: u64);

    /// Set the value of the register without triggering `write` callbacks.
    ///
    /// **Note:** the underlying register value won't change until the
    /// `resolver` [`resolve()`](gwr_engine::traits::Resolve)
    /// is called.
    fn set(&self, resolver: &impl Resolver, value: u64);

    /// Read the current register value and trigger `read` callbacks.
    fn read(&self) -> u64;

    /// Return the current register value without triggering `read` callbacks.
    fn value(&self) -> u64;

    /// Perform a `synchronous` reset.
    ///
    /// **Note:** the underlying register value won't change until the
    /// `resolver` [`resolve()`](gwr_engine::traits::Resolve)
    /// is called.
    fn reset_sync(&self, resolver: &impl Resolver);

    /// Perform an `asynchronous` reset where the value is instantly changed.
    fn reset_async(&self);
}

pub trait Written {
    fn written(&self, old_value: u64, value_written: u64, new_value: u64);
}

pub trait Read {
    fn read(&self, value_read: u64);
}

pub type WrittenCallback = Rc<dyn Written + 'static>;
pub type ReadCallback = Rc<dyn Read + 'static>;

#[macro_export]
macro_rules! build_register_view {
    (
        $(#[$($reg_attrs:tt)*])*
        $reg:ident, $state:path, $state_perms:path, $priority:ident ;
        $(
            $(#[$($field_attrs:tt)*])*
            $field:ident : $perms:expr
        ),+ $(,)*
    ) => {
    $crate::registers::paste! {
        $(#[$($reg_attrs)*])*
        #[doc=concat!("\n\nFor field details, see [`", stringify!($reg), "`](", stringify!($state), ").")]
        #[doc=concat!("\n\nThis view has the following permissions for each field:\n")]
        $(
            #[doc=concat!("  - ", stringify!($field), ": [`", stringify!($perms), "`](crate::registers::Permission).")]
        )+
        pub struct [< $reg Reg >]  {
            state: std::rc::Rc<$state>,
            perms: $state_perms,
            write_callbacks: Vec<$crate::registers::register::WrittenCallback>,
            read_callbacks: Vec<$crate::registers::register::ReadCallback>,
            priority: $crate::registers::state::UpdatePriority,
        }

        impl [< $reg Reg >] {
            pub fn new(state: std::rc::Rc<$state>) -> Self {
                let perms = $state_perms {
                    $(
                    $field: $crate::registers::Permission::$perms,
                    )+
                };
                Self {
                    state,
                    perms,
                    write_callbacks: Vec::new(),
                    read_callbacks: Vec::new(),
                    priority: $crate::registers::state::UpdatePriority::$priority,
                }
            }

            #[allow(dead_code)]
            /// Install a callback function to be called whenever a `write` completes
            pub fn install_write_cb(&mut self, cb: $crate::registers::register::WrittenCallback) {
                self.write_callbacks.push(cb);
            }

            #[allow(dead_code)]
            /// Install a callback function to be called whenever a `read` completes
            pub fn install_read_cb(&mut self, cb: $crate::registers::register::ReadCallback) {
                self.read_callbacks.push(cb);
            }
        }

        impl $crate::registers::register::Register for [< $reg Reg >] {
            fn write(&self, resolver: &impl gwr_engine::traits::Resolver, value: u64) {
                let old_value = self.state.value();
                let new_value = self.state.write(self.priority, &self.perms, value);

                for cb in &self.write_callbacks {
                    cb.written(old_value, value, new_value);
                }
                resolver.add_resolve(self.state.clone());
            }

            fn set(&self, resolver: &impl gwr_engine::traits::Resolver, value: u64) {
                self.state.set(self.priority, &self.perms, value);
                resolver.add_resolve(self.state.clone());
            }

            fn read(&self) -> u64 {
                let value = self.state.read(&self.perms);
                for cb in &self.read_callbacks {
                    cb.read(value);
                }
                value
            }

            fn value(&self) -> u64 {
                let value = self.state.value();
                value
            }

            fn reset_sync(&self, resolver: &impl gwr_engine::traits::Resolver) {
                self.state.reset_sync();
                resolver.add_resolve(self.state.clone());
            }

            fn reset_async(&self) {
                self.state.reset_async();
            }
        }
    }}
}
