// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

mod generated {
    gwr_components::create_state_machine!(
        pub PublicMachine {
            states: [Idle, Busy, Fault],
            default: Idle,
            transitions: [
                start: [Idle] => Busy,
                fail: [Idle, Busy] => Fault,
                reset: [Fault] => Idle,
            ],
        }
    );

    gwr_components::create_state_machine!(
        /// A disabled state machine.
        #[cfg(any())]
        pub DisabledMachine {
            states: [Idle, Busy],
            default: Idle,
            transitions: [
                start: [Idle] => Busy,
            ],
        }
    );
}

use generated::{
    PublicMachine, PublicMachineBusy, PublicMachineFailTransition, PublicMachineFault,
    PublicMachineIdle, PublicMachineResetTransition, PublicMachineStartTransition,
    PublicMachineTransition, PublicMachineTypestate,
};

#[test]
fn public_machine_api_supports_runtime_transition_updates() {
    fn assert_transition<TTransition>(from: PublicMachine, expected: Option<PublicMachine>)
    where
        TTransition: PublicMachineTransition,
    {
        let mut state = from;

        assert_eq!(state.apply::<TTransition>(), expected.is_some());
        assert_eq!(state, expected.unwrap_or(from));
    }

    assert!(PublicMachine::transition_allowed(
        PublicMachine::Idle,
        PublicMachine::Busy
    ));
    assert!(PublicMachine::transition_allowed(
        PublicMachine::Idle,
        PublicMachine::Fault
    ));
    assert!(PublicMachine::transition_allowed(
        PublicMachine::Busy,
        PublicMachine::Fault
    ));
    assert!(PublicMachine::transition_allowed(
        PublicMachine::Fault,
        PublicMachine::Idle
    ));

    assert!(!PublicMachine::transition_allowed(
        PublicMachine::Idle,
        PublicMachine::Idle
    ));
    assert!(!PublicMachine::transition_allowed(
        PublicMachine::Busy,
        PublicMachine::Idle
    ));
    assert!(!PublicMachine::transition_allowed(
        PublicMachine::Busy,
        PublicMachine::Busy
    ));
    assert!(!PublicMachine::transition_allowed(
        PublicMachine::Fault,
        PublicMachine::Busy
    ));
    assert!(!PublicMachine::transition_allowed(
        PublicMachine::Fault,
        PublicMachine::Fault
    ));

    assert_transition::<PublicMachineStartTransition>(
        PublicMachine::Idle,
        Some(PublicMachine::Busy),
    );
    assert_transition::<PublicMachineStartTransition>(PublicMachine::Busy, None);
    assert_transition::<PublicMachineStartTransition>(PublicMachine::Fault, None);

    assert_transition::<PublicMachineFailTransition>(
        PublicMachine::Idle,
        Some(PublicMachine::Fault),
    );
    assert_transition::<PublicMachineFailTransition>(
        PublicMachine::Busy,
        Some(PublicMachine::Fault),
    );
    assert_transition::<PublicMachineFailTransition>(PublicMachine::Fault, None);

    assert_transition::<PublicMachineResetTransition>(PublicMachine::Idle, None);
    assert_transition::<PublicMachineResetTransition>(PublicMachine::Busy, None);
    assert_transition::<PublicMachineResetTransition>(
        PublicMachine::Fault,
        Some(PublicMachine::Idle),
    );
}

#[test]
fn public_machine_api_supports_typestate_construction() {
    fn expect_idle(_: PublicMachineTypestate<PublicMachineIdle>) {}
    fn expect_busy(_: PublicMachineTypestate<PublicMachineBusy>) {}
    fn expect_fault(_: PublicMachineTypestate<PublicMachineFault>) {}

    let idle = PublicMachineTypestate::default();
    assert_eq!(*idle.state(), PublicMachineIdle);
    expect_idle(idle);

    let busy = PublicMachineTypestate::new(PublicMachineIdle).start();
    assert_eq!(busy.into_state(), PublicMachineBusy);
    expect_busy(busy);

    let fault = PublicMachineTypestate::new(PublicMachineBusy).fail();
    assert_eq!(*fault.state(), PublicMachineFault);
    expect_fault(fault);
}
