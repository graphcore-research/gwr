// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Control and Status Registers.

#[macro_export]
macro_rules! build_register_file {
    (
        $(#[$($rf_attrs:tt)*])*
        $regfile:ident, $states:path ;
        $(
            $reg_name:ident: $index:expr, $reg_view:ident, $state:ident
        ),+ $(,)*
    ) => {
    $crate::registers::paste! {
        pub mod [< $regfile:lower _indices >] {
            $( pub const [< $state:upper >]: u64 = $index; )+
        }

        $(#[$($rf_attrs)*])*
        pub struct [< $regfile Regs >] {
            $( pub [< $reg_name:lower >] : [< $reg_view Reg >], )+
        }

        impl [< $regfile Regs >] {
            #[must_use] pub fn new(states: &$states, index: usize) -> Self {
                $(
                let [< $reg_view:lower _state >] = states.[< $state:lower >][index].clone();
                let [< $reg_name:lower >] = [< $reg_view Reg >]::new([< $reg_view:lower _state >]);
                )+
                Self {
                    $( [< $reg_name:lower >], )+
                }
            }

            #[allow(dead_code)]
            pub fn write(&self, resolver: &impl tramway_engine::traits::Resolver, index: u64, value: u64) {
                match index {
                    $( $index => self.[< $reg_name:lower >].write(resolver, value), )+
                    // ignore missing indices
                    _ => {},
                }
            }

            #[must_use] pub fn read(&self, index: u64) -> u64 {
                match index {
                    $( $index => self.[< $reg_name:lower >].read(), )+
                    // ignore missing indices
                    _ => {0},
                }
            }

            /// Perform a `synchronous` reset
            pub fn reset_sync(&self, resolver: &impl tramway_engine::traits::Resolver) {
                $(
                self.[< $reg_name:lower >].reset_sync(resolver);
                )+
            }
        }
    }}
}
