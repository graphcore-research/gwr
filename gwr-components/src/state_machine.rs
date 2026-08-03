// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

//! State-machine builders.

#[doc(hidden)]
pub use paste::paste;

/// Build a state enum, transition metadata, runtime transition checks, and
/// typestate proof helpers.
///
/// The macro takes a list of states, the default state, and a set of named
/// transitions. Each transition declares one or more source states and a single
/// destination state.
///
/// ```rust
/// gwr_components::create_state_machine!(
///     pub JobMachine {
///         states: [Queued, Running, Complete, Failed],
///         default: Queued,
///         transitions: [
///             start: [Queued] => Running,
///             finish: [Running] => Complete,
///             fail: [Queued, Running] => Failed,
///             retry: [Failed] => Queued,
///         ],
///     }
/// );
/// ```
///
/// The generated state enum can be used as a runtime state machine. Transition
/// marker types are named from the machine, action, and `Transition` suffix.
///
/// ```rust
/// # gwr_components::create_state_machine!(
/// #     JobMachine {
/// #         states: [Queued, Running, Complete, Failed],
/// #         default: Queued,
/// #         transitions: [
/// #             start: [Queued] => Running,
/// #             finish: [Running] => Complete,
/// #             fail: [Queued, Running] => Failed,
/// #             retry: [Failed] => Queued,
/// #         ],
/// #     }
/// # );
/// #
/// let mut state = JobMachine::default();
/// assert_eq!(state, JobMachine::Queued);
/// assert!(state.apply::<JobMachineStartTransition>());
/// assert_eq!(state, JobMachine::Running);
/// assert!(!state.apply::<JobMachineRetryTransition>());
/// assert_eq!(state, JobMachine::Running);
/// ```
///
/// The macro also generates typestate wrappers. Valid transition methods are
/// only implemented for the source states listed in the transition declaration,
/// so invalid transition chains fail to compile.
///
/// ```rust
/// # gwr_components::create_state_machine!(
/// #     JobMachine {
/// #         states: [Queued, Running, Complete, Failed],
/// #         default: Queued,
/// #         transitions: [
/// #             start: [Queued] => Running,
/// #             finish: [Running] => Complete,
/// #             fail: [Queued, Running] => Failed,
/// #             retry: [Failed] => Queued,
/// #         ],
/// #     }
/// # );
/// #
/// let queued = JobMachineTypestate::default();
/// let running = queued.start();
/// let complete = running.finish();
/// assert_eq!(complete.into_state(), JobMachineComplete);
/// ```
///
/// `cfg_attr` is rejected because it can expand to a `cfg` attribute after
/// macro parsing, which would allow the generated enum to be disabled without
/// disabling the rest of the generated state-machine items.
///
/// ```compile_fail
/// gwr_components::create_state_machine!(
///     #[cfg_attr(all(), cfg(any()))]
///     pub DisabledByCfgAttr {
///         states: [Idle, Busy],
///         default: Idle,
///         transitions: [
///             start: [Idle] => Busy,
///         ],
///     }
/// );
/// ```
#[macro_export]
macro_rules! create_state_machine {
    (
        @impl
        [$cfg:meta]
        [$(#[$($machine_attrs:tt)*])*]
        $vis:vis $machine:ident {
            states: [ $( $state:ident ),+ $(,)? ],
            default: $default:ident,
            transitions: [
                $(
                    $action:ident: [ $( $from:ident ),+ $(,)? ] => $to:ident
                ),+ $(,)?
            ],
        }
    ) => {
        $crate::state_machine::paste! {
            #[cfg($cfg)]
            $(#[$($machine_attrs)*])*
            #[derive(Clone, Copy, Debug, Eq, PartialEq)]
            $vis enum $machine {
                $( $state, )+
            }

            #[cfg($cfg)]
            impl ::std::default::Default for $machine {
                fn default() -> Self {
                    Self::$default
                }
            }

            #[cfg($cfg)]
            impl ::std::fmt::Display for $machine {
                fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                    f.write_str(self.name())
                }
            }

            #[cfg($cfg)]
            #[allow(dead_code)]
            impl $machine {
                $vis const STATES: &'static [&'static str] = &[
                    $( stringify!($state), )+
                ];

                $vis const TRANSITIONS: &'static [(&'static str, &'static str)] = &[
                    $(
                        $( (stringify!($from), stringify!($to)), )+
                    )+
                ];

                #[must_use]
                $vis fn name(self) -> &'static str {
                    match self {
                        $( Self::$state => stringify!($state), )+
                    }
                }

                #[must_use]
                $vis fn states() -> &'static [&'static str] {
                    Self::STATES
                }

                #[must_use]
                $vis fn transitions() -> &'static [(&'static str, &'static str)] {
                    Self::TRANSITIONS
                }

                #[must_use]
                #[allow(unreachable_patterns)]
                $vis fn transition_allowed(from: Self, to: Self) -> bool {
                    matches!(
                        (from, to),
                        $(
                            $( (Self::$from, Self::$to) )|+
                        )|+
                    )
                }

                $vis fn apply<TTransition>(&mut self) -> bool
                where
                    TTransition: [< $machine Transition >],
                {
                    if !TTransition::FROM.contains(self)
                        || !Self::transition_allowed(*self, TTransition::TO)
                    {
                        return false;
                    }

                    *self = TTransition::TO;
                    true
                }

                #[must_use]
                $vis fn as_str(self) -> &'static str {
                    self.name()
                }

                #[must_use]
                $vis fn mermaid() -> ::std::string::String {
                    let mut diagram = ::std::string::String::from("stateDiagram-v2\n");
                    diagram.push_str("    [*] --> ");
                    diagram.push_str(stringify!($default));
                    diagram.push('\n');

                    $(
                        $(
                            diagram.push_str("    ");
                            diagram.push_str(stringify!($from));
                            diagram.push_str(" --> ");
                            diagram.push_str(stringify!($to));
                            diagram.push_str(": ");
                            diagram.push_str(stringify!($action));
                            diagram.push('\n');
                        )+
                    )+

                    diagram
                }
            }

            $(
                #[cfg($cfg)]
                #[allow(non_camel_case_types)]
                $vis struct [< $machine $action:camel Transition >];
            )+

            #[cfg($cfg)]
            mod [< $machine:snake _transition_seal >] {
                pub trait Sealed {}
            }

            #[cfg($cfg)]
            #[allow(private_bounds)]
            $vis trait [< $machine Transition >]: [< $machine:snake _transition_seal >]::Sealed {
                const FROM: &'static [$machine];
                const TO: $machine;
            }

            $(
                #[cfg($cfg)]
                impl [< $machine:snake _transition_seal >]::Sealed
                    for [< $machine $action:camel Transition >]
                {
                }

                #[cfg($cfg)]
                impl [< $machine Transition >] for [< $machine $action:camel Transition >] {
                    const FROM: &'static [$machine] = &[
                        $( $machine::$from, )+
                    ];
                    const TO: $machine = $machine::$to;
                }
            )+

            $(
                #[cfg($cfg)]
                #[derive(Clone, Copy, Debug, Eq, PartialEq)]
                $vis struct [< $machine $state >];
            )+

            #[cfg($cfg)]
            #[derive(Clone, Copy, Debug, Eq, PartialEq)]
            $vis struct [< $machine Typestate >]<S> {
                state: S,
            }

            #[cfg($cfg)]
            #[allow(dead_code)]
            impl<S> [< $machine Typestate >]<S> {
                #[must_use]
                $vis const fn new(state: S) -> Self {
                    Self {
                        state,
                    }
                }

                #[must_use]
                $vis fn state(&self) -> &S {
                    &self.state
                }

                #[must_use]
                $vis fn into_state(self) -> S {
                    self.state
                }
            }

            #[cfg($cfg)]
            impl ::std::default::Default for [< $machine Typestate >]<[< $machine $default >]> {
                fn default() -> Self {
                    Self::new([< $machine $default >])
                }
            }

            #[cfg($cfg)]
            const _: () = {
                $(
                    let _ = [< $machine $state >];
                    let _ = [< $machine Typestate >]::new([< $machine $state >]);
                )+

                $(
                    let _ = [< $machine $action:camel Transition >];
                )+
            };

            $(
                $(
                    #[cfg($cfg)]
                    #[allow(dead_code, non_snake_case)]
                    impl [< $machine Typestate >]<[< $machine $from >]> {
                        #[must_use]
                        $vis fn $action(self) -> [< $machine Typestate >]<[< $machine $to >]> {
                            [< $machine Typestate >]::new([< $machine $to >])
                        }
                    }
                )+
            )+
        }
    };

    (
        @parse
        [$($cfgs:meta),*]
        [$($machine_attrs:tt)*]
        #[cfg($cfg:meta)]
        $($rest:tt)*
    ) => {
        $crate::create_state_machine!(
            @parse
            [$($cfgs,)* $cfg]
            [$($machine_attrs)*]
            $($rest)*
        );
    };

    (
        @parse
        [$($cfgs:meta),*]
        [$($machine_attrs:tt)*]
        #[cfg_attr($($cfg_attr:tt)*)]
        $($rest:tt)*
    ) => {
        ::std::compile_error!(
            "`create_state_machine!` does not support `cfg_attr`; use explicit `cfg` attributes instead"
        );
    };

    (
        @parse
        [$($cfgs:meta),*]
        [$($machine_attrs:tt)*]
        #[$($machine_attr:tt)*]
        $($rest:tt)*
    ) => {
        $crate::create_state_machine!(
            @parse
            [$($cfgs),*]
            [$($machine_attrs)* #[$($machine_attr)*]]
            $($rest)*
        );
    };

    (
        @parse
        [$($cfgs:meta),*]
        [$($machine_attrs:tt)*]
        $vis:vis $machine:ident {
            states: [ $( $state:ident ),+ $(,)? ],
            default: $default:ident,
            transitions: [
                $(
                    $action:ident: [ $( $from:ident ),+ $(,)? ] => $to:ident
                ),+ $(,)?
            ],
        }
    ) => {
        $crate::create_state_machine!(
            @impl
            [all($($cfgs),*)]
            [$($machine_attrs)*]
            $vis $machine {
                states: [ $( $state ),+ ],
                default: $default,
                transitions: [
                    $(
                        $action: [ $( $from ),+ ] => $to
                    ),+
                ],
            }
        );
    };

    ($($input:tt)*) => {
        $crate::create_state_machine!(@parse [] [] $($input)*);
    };
}

#[cfg(test)]
mod tests {
    create_state_machine!(
        TestMachine {
            states: [Idle, Busy, Fault],
            default: Idle,
            transitions: [
                start: [Idle] => Busy,
                fail: [Idle, Busy] => Fault,
                finish: [Busy] => Idle,
                reset: [Fault] => Idle,
            ],
        }
    );

    create_state_machine!(
        pub(crate) DoorMachine {
            states: [Closed, Open, Locked],
            default: Closed,
            transitions: [
                open: [Closed] => Open,
                close: [Open] => Closed,
                lock: [Closed] => Locked,
                reset: [Open, Locked] => Closed,
            ],
        }
    );

    create_state_machine!(
        pub(crate) DeviceMachine {
            states: [Reset, Ready],
            default: Reset,
            transitions: [
                reset: [Ready] => Reset,
                ready: [Reset] => Ready,
            ],
        }
    );

    #[test]
    fn generated_state_machine_exports_metadata() {
        assert_eq!(TestMachine::states(), &["Idle", "Busy", "Fault"]);
        assert_eq!(TestMachine::default(), TestMachine::Idle);
        assert_eq!(TestMachine::Busy.name(), "Busy");
        assert_eq!(TestMachine::Fault.as_str(), "Fault");
        assert_eq!(TestMachine::Idle.to_string(), "Idle");
        assert_eq!(
            TestMachine::transitions(),
            &[
                ("Idle", "Busy"),
                ("Idle", "Fault"),
                ("Busy", "Fault"),
                ("Busy", "Idle"),
                ("Fault", "Idle"),
            ]
        );
    }

    #[test]
    fn generated_state_machine_checks_transition_matrix() {
        assert!(TestMachine::transition_allowed(
            TestMachine::Idle,
            TestMachine::Busy
        ));
        assert!(TestMachine::transition_allowed(
            TestMachine::Idle,
            TestMachine::Fault
        ));
        assert!(TestMachine::transition_allowed(
            TestMachine::Busy,
            TestMachine::Fault
        ));
        assert!(TestMachine::transition_allowed(
            TestMachine::Busy,
            TestMachine::Idle
        ));
        assert!(TestMachine::transition_allowed(
            TestMachine::Fault,
            TestMachine::Idle
        ));

        assert!(!TestMachine::transition_allowed(
            TestMachine::Fault,
            TestMachine::Busy
        ));
        assert!(!TestMachine::transition_allowed(
            TestMachine::Idle,
            TestMachine::Idle
        ));
        assert!(!TestMachine::transition_allowed(
            TestMachine::Busy,
            TestMachine::Busy
        ));
    }

    #[test]
    fn generated_transition_markers_support_runtime_state_updates() {
        let mut state = TestMachine::Idle;

        assert!(!state.apply::<TestMachineFinishTransition>());
        assert_eq!(state, TestMachine::Idle);

        assert!(state.apply::<TestMachineStartTransition>());
        assert_eq!(state, TestMachine::Busy);

        assert!(state.apply::<TestMachineFailTransition>());
        assert_eq!(state, TestMachine::Fault);

        assert!(state.apply::<TestMachineResetTransition>());
        assert_eq!(state, TestMachine::Idle);
    }

    #[test]
    fn generated_transition_markers_export_source_and_target_metadata() {
        assert_eq!(TestMachineStartTransition::FROM, &[TestMachine::Idle]);
        assert_eq!(TestMachineStartTransition::TO, TestMachine::Busy);

        assert_eq!(
            TestMachineFailTransition::FROM,
            &[TestMachine::Idle, TestMachine::Busy]
        );
        assert_eq!(TestMachineFailTransition::TO, TestMachine::Fault);
    }

    #[test]
    fn generated_typestate_helpers_chain_valid_transitions() {
        fn expect_idle(_: TestMachineTypestate<TestMachineIdle>) {}
        fn expect_busy(_: TestMachineTypestate<TestMachineBusy>) {}
        fn expect_fault(_: TestMachineTypestate<TestMachineFault>) {}

        let idle = TestMachineTypestate::default();
        assert_eq!(*idle.state(), TestMachineIdle);

        let busy = idle.start();
        expect_busy(busy);

        let busy = TestMachineTypestate::new(TestMachineBusy);
        let fault = busy.fail();
        expect_fault(fault);

        let fault = TestMachineTypestate::new(TestMachineFault);
        let idle = fault.reset();
        assert_eq!(idle.into_state(), TestMachineIdle);
        expect_idle(idle);
    }

    #[test]
    fn generated_names_do_not_collide_when_events_repeat_across_machines() {
        assert_eq!(
            DoorMachineResetTransition::FROM,
            &[DoorMachine::Open, DoorMachine::Locked]
        );
        assert_eq!(DoorMachineResetTransition::TO, DoorMachine::Closed);
        assert_eq!(DeviceMachineResetTransition::FROM, &[DeviceMachine::Ready]);
        assert_eq!(DeviceMachineResetTransition::TO, DeviceMachine::Reset);
        assert!(DeviceMachine::transition_allowed(
            DeviceMachine::Ready,
            DeviceMachine::Reset
        ));
    }

    #[test]
    fn generated_public_visibility_exports_machine_api() {
        assert_eq!(
            DoorMachine::transitions(),
            &[
                ("Closed", "Open"),
                ("Open", "Closed"),
                ("Closed", "Locked"),
                ("Open", "Closed"),
                ("Locked", "Closed"),
            ]
        );
        assert_eq!(DoorMachine::default(), DoorMachine::Closed);
        assert_eq!(DoorMachine::Locked.to_string(), "Locked");
    }

    #[test]
    fn generated_mermaid_diagram_uses_default_state_and_action_labels() {
        assert_eq!(
            TestMachine::mermaid(),
            concat!(
                "stateDiagram-v2\n",
                "    [*] --> Idle\n",
                "    Idle --> Busy: start\n",
                "    Idle --> Fault: fail\n",
                "    Busy --> Fault: fail\n",
                "    Busy --> Idle: finish\n",
                "    Fault --> Idle: reset\n",
            )
        );
    }

    #[test]
    fn generated_mermaid_diagram_expands_shared_events_in_declaration_order() {
        assert_eq!(
            DoorMachine::mermaid(),
            concat!(
                "stateDiagram-v2\n",
                "    [*] --> Closed\n",
                "    Closed --> Open: open\n",
                "    Open --> Closed: close\n",
                "    Closed --> Locked: lock\n",
                "    Open --> Closed: reset\n",
                "    Locked --> Closed: reset\n",
            )
        );
    }
}
