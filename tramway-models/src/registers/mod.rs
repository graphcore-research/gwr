// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Control and Status Registers builders.

pub mod field;
pub mod regfile;
pub mod register;
pub mod state;

pub mod test_helpers;

pub use paste::paste;

/// Register access permissions
pub enum Permission {
    /// Writes ignored. Reads return constant value.
    ReadOnly,

    /// Writes ignored. Reads return current value of dynamic state.
    ReadVolatileOnly,

    /// Writes value committed to state. Reads return last value written, or
    /// reset value if not written yet.
    ReadWrite,

    /// Writes value committed to state. Reads return current value of dynamic
    /// state.
    ReadVolatileWrite,

    /// Writes value committed to state. Reads return 0.
    WriteCommits,

    /// Writes of `0b1` initiate background operation. Reads return 0.
    WriteOneCommits,

    /// Writes value commited to state. Reads not possible. There is no
    /// mechanism available to directly read this state. It may be possible
    /// to read the state indirectly.
    WriteOnly,

    /// All writes ignored. Reads return 0.
    WriteIgnore,

    /// Writes ignored after first transition from `WaitForExchange` to
    /// `Executing`. Reads yield last value successfully written or reset
    /// value if never written to.
    WriteIgnoredAfterBoot,

    /// Writes ignored. Reads return 0.
    Reserved,
}

#[cfg(test)]
pub mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use tramway_engine::traits::Resolve;

    use super::register::{Read, Register, Written};
    use super::state::{RegisterState, UpdatePriority};
    use crate::registers::test_helpers::TestResolver;
    use crate::{
        build_register_file, build_register_state, build_register_states, build_register_view,
    };

    pub struct TestCallbackHandler {
        pub written_count: RefCell<usize>,
        pub read_count: RefCell<usize>,
        pub last_write: RefCell<Option<(u64, u64, u64)>>,
    }

    impl TestCallbackHandler {
        #[must_use]
        pub fn new() -> Self {
            Self {
                written_count: RefCell::new(0),
                read_count: RefCell::new(0),
                last_write: RefCell::new(None),
            }
        }
    }

    impl Default for TestCallbackHandler {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Written for TestCallbackHandler {
        fn written(&self, old_value: u64, value_written: u64, new_value: u64) {
            *self.written_count.borrow_mut() += 1;
            *self.last_write.borrow_mut() = Some((old_value, value_written, new_value));
        }
    }

    impl Read for TestCallbackHandler {
        fn read(&self, _value_read: u64) {
            *self.read_count.borrow_mut() += 1;
        }
    }

    // Reset values of different field types.
    pub const CSR_RESET_VALUE: u64 = 0x00cc01;
    // ReadVolatileOnly fields can't be changed by a write.
    pub const CSR_WRITE_VALUE: u64 = 0x00ccff;
    // ReadVolatileOnly can be changed by a set.
    pub const CSR_SET_VALUE: u64 = 0xffccff;

    // An underlying register state.
    build_register_state!(
        /// The Control Status Register Example (multiple views)
        Csr, 32 ;
        /// A per-thread `enable` bit
        enabled: 8, 0x1,
        /// Reserved
        reserved: 8, 0xcc,
        /// A per-thread `excepted` bit
        excepted: 8, 0,
        /// A single trigger bit
        trigger: 1, 0,
    );

    // A set of test states.
    build_register_states!(
        /// All register state
        TestCsrStates ; Csr, 1,
    );

    // A ReadWrite register view.
    build_register_view!(
        /// Read-only view of the Control Status Register.
        CsrRw, CsrState, CsrStatePerms, High ;
        /// The `enable` field is Read-only in this view
        enabled: ReadWrite,
        /// Reserved
        reserved: Reserved,
        /// The `excepted` field is Read-only in this view.
        excepted: ReadVolatileOnly,
        /// Trigger
        trigger: WriteOneCommits,
    );

    // A ReadOnly register view.
    build_register_view!(
        /// Read-only view of the Control Status Register.
        CsrRo, CsrState, CsrStatePerms, High ;
        /// The `enable` field is Read-only in this view
        enabled: ReadOnly,
        /// Reserved
        reserved: Reserved,
        /// The `excepted` field is Read-only in this view.
        excepted: ReadVolatileOnly,
        /// Trigger
        trigger: ReadOnly,
    );

    // A test register file with ReadWrite view.
    build_register_file!(
        /// Test Register File
        TestCsrsRw, TestCsrStates ;
        Csr : 0x00, CsrRw, Csr,
    );

    // A test register file with ReadOnly view.
    build_register_file!(
        /// Test Register File
        TestCsrsRo, TestCsrStates ;
        Csr : 0x10, CsrRo, Csr,
    );

    #[test]
    fn basics() {
        let resolver = TestResolver::new();
        let csr_state = Rc::new(CsrState::new());

        let csr_ro = CsrRoReg::new(csr_state.clone());
        let csr_rw = CsrRwReg::new(csr_state);

        assert_eq!(csr_ro.read(), CSR_RESET_VALUE);
        assert_eq!(csr_rw.read(), CSR_RESET_VALUE);

        csr_ro.write(&resolver, 0xffffffff);
        resolver.resolve();
        assert_eq!(csr_ro.read(), CSR_RESET_VALUE);
        assert_eq!(csr_rw.read(), CSR_RESET_VALUE);

        csr_rw.write(&resolver, 0xffffffff);
        resolver.resolve();
        assert_eq!(csr_ro.read(), CSR_WRITE_VALUE);
        assert_eq!(csr_rw.read(), CSR_WRITE_VALUE);
    }

    #[test]
    fn reset() {
        let resolver = TestResolver::new();
        let csr_state = Rc::new(CsrState::new());
        let csr_rw = CsrRwReg::new(csr_state);

        csr_rw.write(&resolver, 0xffff);
        assert_eq!(csr_rw.value(), CSR_RESET_VALUE);
        resolver.resolve();
        assert_eq!(csr_rw.value(), CSR_WRITE_VALUE);

        // Test the sync reset
        csr_rw.reset_sync(&resolver);
        assert_eq!(csr_rw.value(), CSR_WRITE_VALUE);

        resolver.resolve();
        assert_eq!(csr_rw.value(), CSR_RESET_VALUE);

        // Test the async reset
        csr_rw.write(&resolver, 0xffff);
        resolver.resolve();
        assert_eq!(csr_rw.value(), CSR_WRITE_VALUE);

        csr_rw.reset_async();
        assert_eq!(csr_rw.value(), CSR_RESET_VALUE);
    }

    #[test]
    fn reg_file() {
        let resolver = TestResolver::new();
        let csr_states = TestCsrStates::new();
        let csrs = TestCsrsRwRegs::new(&csr_states, 0);
        assert_eq!(csrs.csr.value(), CSR_RESET_VALUE);

        csrs.csr.write(&resolver, 0xffff);
        assert_eq!(csrs.csr.value(), CSR_RESET_VALUE);

        resolver.resolve();
        assert_eq!(csrs.csr.value(), CSR_WRITE_VALUE);

        csrs.reset_sync(&resolver);
        assert_eq!(csrs.csr.value(), CSR_WRITE_VALUE);

        resolver.resolve();
        assert_eq!(csrs.csr.value(), CSR_RESET_VALUE);
    }

    #[test]
    fn reg_file_by_index() {
        let resolver = TestResolver::new();
        let csr_states = TestCsrStates::new();
        let csrs_rw = TestCsrsRwRegs::new(&csr_states, 0);
        let csrs_ro = TestCsrsRoRegs::new(&csr_states, 0);
        assert_eq!(csrs_rw.csr.value(), CSR_RESET_VALUE);

        const CSR_RW_INDEX: u64 = testcsrsrw_indices::CSR;
        const CSR_RO_INDEX: u64 = testcsrsro_indices::CSR;

        csrs_rw.write(&resolver, testcsrsrw_indices::CSR, 0xffff);
        assert_eq!(csrs_rw.csr.value(), CSR_RESET_VALUE);

        resolver.resolve();
        assert_eq!(csrs_rw.csr.value(), CSR_WRITE_VALUE);
        assert_eq!(csrs_rw.read(CSR_RW_INDEX), CSR_WRITE_VALUE);
        assert_eq!(csrs_ro.read(CSR_RO_INDEX), CSR_WRITE_VALUE);
    }

    #[test]
    fn reserved_stays_on_set() {
        let resolver = TestResolver::new();
        let state = Rc::new(CsrState::new());
        let reg = CsrRwReg::new(state);

        assert_eq!(reg.value(), CSR_RESET_VALUE);

        reg.set(&resolver, 0xffffffff);
        assert_eq!(reg.value(), CSR_RESET_VALUE);

        resolver.resolve();
        assert_eq!(reg.value(), CSR_SET_VALUE);
    }

    #[test]
    fn alias() {
        let resolver = TestResolver::new();
        let state = Rc::new(TestCsrStates::new());
        let regs_ro = TestCsrsRoRegs::new(&state, 0);
        let regs_rw = TestCsrsRwRegs::new(&state, 0);

        assert_eq!(regs_ro.csr.value(), CSR_RESET_VALUE);
        assert_eq!(regs_rw.csr.value(), CSR_RESET_VALUE);

        regs_ro.csr.write(&resolver, 0xffffffff);
        resolver.resolve();
        assert_eq!(regs_ro.csr.value(), CSR_RESET_VALUE);
        assert_eq!(regs_rw.csr.value(), CSR_RESET_VALUE);

        regs_rw.csr.write(&resolver, 0xffffffff);
        assert_eq!(regs_ro.csr.value(), CSR_RESET_VALUE);
        assert_eq!(regs_rw.csr.value(), CSR_RESET_VALUE);

        resolver.resolve();
        assert_eq!(regs_ro.csr.value(), CSR_WRITE_VALUE);
        assert_eq!(regs_rw.csr.value(), CSR_WRITE_VALUE);

        regs_ro.reset_sync(&resolver);
        assert_eq!(regs_ro.csr.value(), CSR_WRITE_VALUE);
        assert_eq!(regs_rw.csr.value(), CSR_WRITE_VALUE);

        resolver.resolve();
        assert_eq!(regs_ro.csr.value(), CSR_RESET_VALUE);
        assert_eq!(regs_rw.csr.value(), CSR_RESET_VALUE);
    }

    #[test]
    fn write_callback() {
        let resolver = TestResolver::new();
        let state = Rc::new(CsrState::new());
        let mut reg: CsrRwReg = CsrRwReg::new(state);

        let cb_handler = Rc::new(TestCallbackHandler::new());
        reg.install_read_cb(cb_handler.clone());
        reg.install_write_cb(cb_handler.clone());

        reg.write(&resolver, 0xffffffff);
        resolver.resolve();

        assert_eq!(*cb_handler.read_count.borrow(), 0);
        assert_eq!(*cb_handler.written_count.borrow(), 1);
    }

    #[test]
    fn read_callback() {
        let state = Rc::new(CsrState::new());
        let mut reg = CsrRwReg::new(state);

        let cb_handler = Rc::new(TestCallbackHandler::new());
        reg.install_read_cb(cb_handler.clone());
        reg.install_write_cb(cb_handler.clone());

        let _ = reg.read();
        assert_eq!(*cb_handler.read_count.borrow(), 1);
        assert_eq!(*cb_handler.written_count.borrow(), 0);
    }

    #[test]
    fn write_one_commit() {
        // Ensure that the `WriteOneCommit` field doesn't get changed, but that a
        // callback handler would see the value written to act on it.
        let resolver = TestResolver::new();
        let state = Rc::new(CsrState::new());
        let mut reg: CsrRwReg = CsrRwReg::new(state);

        let cb_handler = Rc::new(TestCallbackHandler::new());
        reg.install_read_cb(cb_handler.clone());
        reg.install_write_cb(cb_handler.clone());

        reg.write(&resolver, 0xffffffff);
        resolver.resolve();

        assert_eq!(*cb_handler.read_count.borrow(), 0);
        assert_eq!(*cb_handler.written_count.borrow(), 1);

        assert_eq!(reg.read(), CSR_WRITE_VALUE);

        let (old_value, written_value, new_value) = cb_handler.last_write.borrow().unwrap();

        assert_eq!(old_value, CSR_RESET_VALUE);
        assert_eq!(written_value, 0xffffffff);
        assert_eq!(new_value, CSR_WRITE_VALUE);
    }
}
