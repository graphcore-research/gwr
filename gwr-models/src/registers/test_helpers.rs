// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

use std::cell::RefCell;
use std::rc::Rc;

use gwr_engine::traits::{Resolve, Resolver};

pub struct TestResolver {
    to_resolve: RefCell<Vec<Rc<dyn Resolve>>>,
}

impl TestResolver {
    #[must_use]
    pub fn new() -> Self {
        Self {
            to_resolve: RefCell::new(Vec::new()),
        }
    }
}

impl Default for TestResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver for TestResolver {
    fn add_resolve(&self, resolve: Rc<dyn Resolve + 'static>) {
        self.to_resolve.borrow_mut().push(resolve);
    }
}

impl Resolve for TestResolver {
    fn resolve(&self) {
        for r in self.to_resolve.borrow_mut().drain(..) {
            r.resolve();
        }
    }
}
