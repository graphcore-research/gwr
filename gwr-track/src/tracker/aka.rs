// Copyright (c) 2025 Graphcore Ltd. All rights reserved.

//! Also Known As (AKA) - an alternative name manager.

use std::collections::HashMap;
use std::fmt::Display;
use std::rc::Rc;

use crate::entity::Entity;

#[macro_export]
/// Helper function for creating local Aka derived from incoming Aka
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

#[must_use]
/// Lookup the renames for a given port name
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

/// Build up a new set of alternative names with a new set of renames
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
