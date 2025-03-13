// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Underlying state of a register

/// Specify priority for pending updates to define which one wins.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpdatePriority {
    Low = 0,
    High,
}

/// The [`RegisterState`] trait defines what functionality the underlying state
/// provides.
pub trait RegisterState<T> {
    /// Write respecting permissions.
    ///
    /// **Note:** The actual state update will not take effect until
    /// [`resolve`](steam_engine::traits::Resolve) has been called.
    fn write(&self, priority: UpdatePriority, perms: &T, value: u64) -> u64;

    /// Set underlying value without checking permissions.
    ///
    /// **Note:** The actual state update will not take effect until
    /// [`resolve`](steam_engine::traits::Resolve) has been called.
    fn set(&self, priority: UpdatePriority, perms: &T, value: u64);

    /// Read and respect permissions.
    fn read(&self, perms: &T) -> u64;

    /// Return underlying value.
    fn value(&self) -> u64;

    /// Perform a synchronous reset.
    ///
    /// **Note:** The actual state update will not take effect until
    /// [`resolve`](steam_engine::traits::Resolve) has been called.
    fn reset_sync(&self);

    /// Perform an asynchronous reset.
    fn reset_async(&self);
}

#[macro_export]
macro_rules! build_register_state {
    (
        $(#[$($reg_attrs:tt)*])*
        $reg:ident, $reg_num_bits:expr ;
        $(
            $(#[$($field_attrs:tt)*])*
            $field:ident : $num_bits:expr, $reset:expr
        ),+ $(,)*
    ) => {
    $crate::registers::paste! {
    #[doc=concat!("Structure used to define the permissions of the  ", stringify!($reg), " register.")]
    pub struct [< $reg StatePerms >] {
        $(
        #[doc=concat!("Permissions for the value for the ", stringify!($field), " field.")]
        pub $field: $crate::registers::Permission,
        )+
    }

    #[derive(Clone)]
    #[doc=concat!("The underlying state for the ", stringify!($reg), " register.")]
    pub struct [< $reg State >] {
        $(
        $(#[$($field_attrs)*])*
        #[doc=concat!("(", stringify!($num_bits), " bits, ", stringify!($reset), " at reset).")]
        pub $field: $crate::registers::field::Field,
        )+

        #[doc=concat!("The value of the ", stringify!($reg), " register.")]
        register_value: std::cell::RefCell<u64>,

        #[doc=concat!("A bitmask of the existing bits in the ", stringify!($reg), " register.")]
        existing_bits_mask: u64,

        pending: std::cell::RefCell<Vec<(UpdatePriority, u64)>>,
    }

    $(#[$($reg_attrs)*])*
    impl [< $reg State >] {
        pub fn new() -> Self {
            let start_bit = 0;
            $(
            let $field = $crate::registers::field::Field::new($num_bits, start_bit, $reset);
            let start_bit = $field.last_bit() + 1;
            )+
            let mask = if start_bit >= $reg_num_bits { !0 } else { (1 << start_bit) - 1 };
            let state = Self {
                register_value: std::cell::RefCell::new(0),
                $( $field, )+
                existing_bits_mask: mask,
                pending: std::cell::RefCell::new(Vec::new()),
            };
            state.reset_async();
            state
        }
    }

    impl RegisterState< [< $reg StatePerms >] > for [< $reg State >] {
        /// Returns the value that will be written to the register by the `resolve()` call
        ///
        /// Registers itself with the [`Resolver`]
        fn write(&self, priority: $crate::registers::state::UpdatePriority, perms: &[< $reg StatePerms >], value: u64) -> u64 {
            let mask = self.existing_bits_mask;
            $(
            let mask = self.$field.apply_write_permissions(mask, &perms.$field);
            )+

            let value = (*self.register_value.borrow() & !mask) | (value & mask);
            self.pending.borrow_mut().push((priority, value));

            value
        }

        fn set(&self, priority: $crate::registers::state::UpdatePriority, perms: &[< $reg StatePerms >], value: u64) {
            let value = value & self.existing_bits_mask;
            $(
            let value = self.$field.apply_set_permissions(value, &perms.$field);
            )+
            self.pending.borrow_mut().push((priority, value));
        }

        fn read(&self, perms: &[< $reg StatePerms >]) -> u64 {
            let value = *self.register_value.borrow();
            $(
            let value = self.$field.apply_read_permissions(value, &perms.$field);
            )+
            value
        }

        fn value(&self) -> u64 {
            *self.register_value.borrow()
        }

        fn reset_sync(&self) {
            let value = 0;
            $(
            let value = self.$field.apply_reset_value(value);
            )+
            self.pending.borrow_mut().push(($crate::registers::state::UpdatePriority::High, value));
        }

        fn reset_async(&self) {
            let value = 0;
            $(
            let value = self.$field.apply_reset_value(value);
            )+
            *self.register_value.borrow_mut() = value;
        }
    }

    impl Resolve for [< $reg State >] {
        fn resolve(&self) {
            // If there are multiple writers then resolve will be called more than once.
            if !self.pending.borrow().is_empty() {
                // No need to do anything if already resolved.
                *self.register_value.borrow_mut() = self.pending.borrow().iter().reduce(|max, x| x.max(max)).unwrap().1;
                self.pending.borrow_mut().clear();
            }
        }
    }

    impl Default for [< $reg State >] {
        fn default() -> Self {
            Self::new()
        }
    }

    }}
}

#[macro_export]
/// Allow the creation of compile-time arrays of state up to 8 elements long
/// without needing the `Copy` trait to be implemented.
///
/// Derived from: <https://danielkeep.github.io/tlborm/book/pat-push-down-accumulation.html>
macro_rules! array {
    (@accum (0, $($_es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@as_expr [$($body)*])};
    (@accum (1, $($es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@accum (0, $($es),*) -> ($($body)* $($es,)*))};
    (@accum (2, $($es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@accum (0, $($es),*) -> ($($body)* $($es,)* $($es,)*))};
    (@accum (3, $($es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@accum (2, $($es),*) -> ($($body)* $($es,)*))};
    (@accum (4, $($es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@accum (2, $($es,)* $($es),*) -> ($($body)*))};
    (@accum (5, $($es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@accum (4, $($es),*) -> ($($body)* $($es,)*))};
    (@accum (6, $($es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@accum (4, $($es),*) -> ($($body)* $($es,)* $($es,)*))};
    (@accum (7, $($es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@accum (4, $($es),*) -> ($($body)* $($es,)* $($es,)* $($es,)*))};
    (@accum (8, $($es:expr),*) -> ($($body:tt)*))
        => {$crate::array!(@accum (4, $($es,)* $($es),*) -> ($($body)*))};

    (@as_expr $e:expr) => {$e};

    [$e:expr; $n:tt] => { $crate::array!(@accum ($n, $e) -> ()) };
}

#[macro_export]
macro_rules! build_register_states {
    (
        $(#[$($rf_attrs:tt)*])*
        $state_name:ident ;
        $(
            $state:ident, $num:expr
        ),+ $(,)*
    ) => {
    $crate::registers::paste! {
        $(#[$($rf_attrs)*])*
        pub struct $state_name {
            $( pub [< $state:lower >] : [ std::rc::Rc< [< $state State >] > ; $num ], )+
        }

        impl $state_name {
            pub fn new() -> Self {
                $(
                let [< $state:lower >] = $crate::array![ std::rc::Rc::new([< $state State >]::new()) ; $num ];
                )+
                Self {
                    $( [< $state:lower >], )+
                }
            }
        }

        impl Default for $state_name {
            fn default() -> Self {
                Self::new()
            }
        }
    }}
}
