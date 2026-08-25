// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Also Known As (AKA) - an alternative name manager.
//!
//! `Aka` lets a composite model expose stable public names while delegating the
//! actual entity or port implementation to child components. Constructors named
//! `new_and_register_with_renames` accept an `Option<&Aka>` and pass it to
//! [`Entity::new_with_renames`](crate::entity::Entity::new_with_renames), port
//! constructors such as `InPort::new_with_renames` and
//! `OutPort::new_with_renames`, or child constructors so the tracking layer can
//! record both the local implementation name and the parent-visible name.
//!
//! The usual public constructor should remain `new_and_register`; it normally
//! calls the rename-aware constructor with `None` so users do not need to know
//! about `Aka`. Add a rename-aware constructor when a component delegates
//! public ports to internal subcomponents, composes other rename-aware
//! children, or needs alternate names for tracking, filtering, or monitor
//! configuration. Leaf components whose local ports are already their public
//! API can stay with `new_and_register` until a real composition use case
//! appears.

use std::collections::HashMap;
use std::fmt::Display;
use std::rc::Rc;

use crate::entity::Entity;

/// Helper function for creating local Aka derived from incoming Aka
#[macro_export]
macro_rules! build_aka {
    ($aka:ident, $parent:expr, $renames:expr) => {{
        let mut tmp_aka = gwr_track::tracker::aka::Aka::default();
        gwr_track::tracker::aka::populate_aka($aka, Some(&mut tmp_aka), $parent, $renames);
        tmp_aka
    }};
}

/// Type alias for the optional list of alternative names
pub type AlternativeNames<'a> = Option<&'a Vec<String>>;

/// A structure to manage alternative names for entities
#[derive(Default)]
pub struct Aka {
    names: HashMap<String, Vec<String>>,
}

impl Aka {
    /// Get the list of alternative names for an entity
    #[must_use]
    pub fn get_alternative_names<'a>(&'a self, name: &str) -> AlternativeNames<'a> {
        self.names.get(name)
    }
}

impl Display for Aka {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#?}", self.names)
    }
}

/// Lookup the renames for a given port name
#[must_use]
pub fn get_alternative_names<'a>(aka: Option<&'a Aka>, name: &str) -> AlternativeNames<'a> {
    if let Some(aka) = aka {
        return aka.get_alternative_names(name);
    }
    None
}

/// Build up a new set of alternative names with a new set of renames
pub fn populate_aka_from_string(
    aka: Option<&Aka>,
    new_aka: Option<&mut Aka>,
    entity: &Rc<Entity>,
    renames: &[(String, String)],
) {
    let ref_names: Vec<(&str, &str)> = renames
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    populate_aka(aka, new_aka, entity, &ref_names);
}

/// Build up a new set of alternative names with a new set of renames.
///
/// Each tuple maps a name visible on `entity` to the child-local name that
/// should inherit the alternative names. For example, a composite can map a
/// public `rx_a` port to an internal limiter's `rx` port so traces and monitor
/// configuration can refer to either level of the model.
pub fn populate_aka(
    aka: Option<&Aka>,
    new_aka: Option<&mut Aka>,
    entity: &Rc<Entity>,
    renames: &[(&str, &str)],
) {
    if let Some(new_aka) = new_aka {
        for (name_in_entity, name_in_child) in renames {
            let renames = if let Some(aka) = aka.as_ref() {
                match aka.names.get(*name_in_entity) {
                    Some(existing_renames) => {
                        let mut new_renames = existing_renames.clone();
                        new_renames.push(format!("{}::{}", entity.full_name(), name_in_entity));
                        new_renames
                    }
                    None => {
                        vec![format!("{}::{}", entity.full_name(), name_in_entity)]
                    }
                }
            } else {
                vec![format!("{}::{}", entity.full_name(), name_in_entity)]
            };
            new_aka.names.insert((*name_in_child).to_string(), renames);
        }
    }
}
