// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

use rand::Rng;
use rand::rngs::StdRng;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MenuItem {
    Burger,
    Cheeseburger,
    Fries,
    Nuggets,
    Salad,
    Soda,
    Milkshake,
}

impl MenuItem {
    pub const fn prep_ticks(self) -> u64 {
        match self {
            Self::Burger => 105,
            Self::Cheeseburger => 120,
            Self::Fries => 75,
            Self::Nuggets => 95,
            Self::Salad => 70,
            Self::Soda => 18,
            Self::Milkshake => 40,
        }
    }

    pub const fn order_ticks(self) -> u64 {
        match self {
            Self::Burger => 6,
            Self::Cheeseburger => 7,
            Self::Fries => 4,
            Self::Nuggets => 5,
            Self::Salad => 4,
            Self::Soda => 2,
            Self::Milkshake => 3,
        }
    }

    pub const fn price(self) -> f64 {
        match self {
            Self::Burger => 6.80,
            Self::Cheeseburger => 7.40,
            Self::Fries => 3.20,
            Self::Nuggets => 4.90,
            Self::Salad => 4.60,
            Self::Soda => 2.40,
            Self::Milkshake => 3.90,
        }
    }

    pub const fn ingredient_cost(self) -> f64 {
        match self {
            Self::Burger => 2.35,
            Self::Cheeseburger => 2.65,
            Self::Fries => 0.95,
            Self::Nuggets => 1.70,
            Self::Salad => 1.55,
            Self::Soda => 0.38,
            Self::Milkshake => 1.10,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OrderLine {
    pub item: MenuItem,
    pub count: u32,
}

impl OrderLine {
    pub const fn new(item: MenuItem, count: u32) -> Self {
        Self { item, count }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OrderTemplate {
    pub name: &'static str,
    pub items: &'static [OrderLine],
    pub weight: u32,
}

impl OrderTemplate {
    pub const fn new(name: &'static str, items: &'static [OrderLine], weight: u32) -> Self {
        Self {
            name,
            items,
            weight,
        }
    }
}

pub const ORDERS: [OrderTemplate; 8] = [
    OrderTemplate::new(
        "Burger Combo",
        &[
            OrderLine::new(MenuItem::Burger, 1),
            OrderLine::new(MenuItem::Fries, 1),
            OrderLine::new(MenuItem::Soda, 1),
        ],
        24,
    ),
    OrderTemplate::new(
        "Cheeseburger Combo",
        &[
            OrderLine::new(MenuItem::Cheeseburger, 1),
            OrderLine::new(MenuItem::Fries, 1),
            OrderLine::new(MenuItem::Soda, 1),
        ],
        21,
    ),
    OrderTemplate::new(
        "Burger + Shake",
        &[
            OrderLine::new(MenuItem::Burger, 1),
            OrderLine::new(MenuItem::Milkshake, 1),
        ],
        10,
    ),
    OrderTemplate::new(
        "Nuggets Combo",
        &[
            OrderLine::new(MenuItem::Nuggets, 1),
            OrderLine::new(MenuItem::Fries, 1),
            OrderLine::new(MenuItem::Soda, 1),
        ],
        17,
    ),
    OrderTemplate::new(
        "Light Lunch",
        &[
            OrderLine::new(MenuItem::Salad, 1),
            OrderLine::new(MenuItem::Soda, 1),
        ],
        9,
    ),
    OrderTemplate::new(
        "Fries + Soda",
        &[
            OrderLine::new(MenuItem::Fries, 1),
            OrderLine::new(MenuItem::Soda, 1),
        ],
        7,
    ),
    OrderTemplate::new(
        "Family Snack",
        &[
            OrderLine::new(MenuItem::Burger, 2),
            OrderLine::new(MenuItem::Nuggets, 2),
            OrderLine::new(MenuItem::Fries, 2),
            OrderLine::new(MenuItem::Soda, 3),
        ],
        8,
    ),
    OrderTemplate::new(
        "Cheeseburger + Shake",
        &[
            OrderLine::new(MenuItem::Cheeseburger, 1),
            OrderLine::new(MenuItem::Milkshake, 1),
        ],
        4,
    ),
];

pub fn choose_order_index(rng: &mut StdRng) -> usize {
    let total_weight: u32 = ORDERS.iter().map(|order| order.weight).sum();
    let mut choice = rng.random_range(0..total_weight);
    for (index, order) in ORDERS.iter().enumerate() {
        if choice < order.weight {
            return index;
        }
        choice -= order.weight;
    }
    ORDERS.len() - 1
}

#[must_use]
pub const fn order_name(order_index: usize) -> &'static str {
    ORDERS[order_index].name
}

#[must_use]
pub fn order_value(order_index: usize) -> (f64, f64) {
    ORDERS[order_index]
        .items
        .iter()
        .fold((0.0, 0.0), |(price, cost), line| {
            let count = f64::from(line.count);
            (
                price + line.item.price() * count,
                cost + line.item.ingredient_cost() * count,
            )
        })
}
