// Copyright (c) 2023 Graphcore Ltd. All rights reserved.

//! Common code used by all packet types
//!
//! Currently just the [build_packet_type](crate::build_packet_type) macro.

pub use paste::paste;

#[macro_export]
/// Macro helper for building packet types.
///
/// This macro builds the common components in a packet as well as adding all
/// the packet-specific fields.
macro_rules! build_packet_type {
    ($protocol:ident, $packet_type:ident, $types:ident, $pkt_type:ident; $($field:ident : $type:ty, $bits:expr, $scale:expr),+ $(,)*) => {
        $crate::packet::paste! {

        #[derive(Default)]
        pub struct [< $packet_type Cfg >] {
            /// Initial value for the number of payload bytes.
            pub payload_bytes: u32,
            $(
            #[doc=concat!("Initial value for the ", stringify!($field), " field.")]
            pub $field: $type,
            )+
        }

        #[derive(Clone)]
        pub struct [< $packet_type  >] {
            /// The entity responsible for the logging control of this packet.
            pub entity: std::rc::Rc<Entity>,

            /// The unique id used for logging this packet.
            pub id: gwr_track::Id,

            pkt_type: $types,
            $($field: $type,)+
            payload_bytes: u32,
            payload: Option<Box<Vec<u8>>>,
        }
        impl [< $packet_type  >] {
            #[doc=concat!("Create a new ", stringify!($packet_type), " packet.")]
            pub fn new(created_by: &std::rc::Rc<gwr_track::entity::Entity>, cfg: &[< $packet_type Cfg >]) -> Self {
                let pkt = Self {
                    entity: created_by.clone(),
                    pkt_type: $types::$pkt_type,
                    id: gwr_track::create_id!(created_by),
                    payload_bytes: cfg.payload_bytes,
                    payload: None,
                    $(
                        $field: (cfg.$field >> $scale) & ((1 << $bits) - 1),
                    )+
                };
                gwr_track::create!(created_by ; pkt, pkt.total_bytes(), pkt.req_type() as i8);
                pkt
            }

            pub fn pkt_type(&self) -> $types {
                self.pkt_type
            }

        $(
            #[doc=concat!("Get the current value of the ", stringify!($field), " field.")]
            pub fn $field(&self) -> $type {
                self.$field << $scale
            }

            #[doc=concat!("Update the ", stringify!($field), " field.")]
            pub fn [< set_$field >](&mut self, value: $type) {
                self.$field = (value >> $scale) & ((1 << $bits) - 1);
            }
        )+

            /// Get the number of payload bytes.
            pub fn payload_bytes(&self) -> u32 {
                self.payload_bytes
            }

            /// Update the number of payload bytes.
            pub fn set_payload_bytes(&mut self, payload_bytes: u32) {
                self.payload_bytes = payload_bytes;
            }

            /// Update the payload contents. Will also set `payload_bytes` to match.
            pub fn set_payload(&mut self, payload: Vec<u8>) {
                self.payload_bytes = payload.len() as u32;
                self.payload = Some(Box::new(payload));
            }

            pub fn get_payload(&self) -> &Option<Box<Vec<u8>>> {
                &self.payload
            }
        }

        impl gwr_engine::traits::TotalBytes for [< $packet_type  >] {
            fn total_bytes(&self) -> usize {
                self.payload_bytes as usize + $protocol::HEADER_BYTES
            }
        }

        impl std::fmt::Display for [< $packet_type  >] {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "{}", self.id)
            }
        }

        impl std::fmt::Debug for [< $packet_type  >] {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "{}:", stringify!($pkt_type))?;
                $(
                write!(f, " {}: {},", stringify!($field), self.$field)?;
                )+
                Ok(())
            }
        }

        impl gwr_track::id::Unique for [< $packet_type  >] {
            fn id(&self) -> gwr_track::id::Id {
                self.id
            }
        }

        impl gwr_engine::traits::SimObject for [< $packet_type  >] {}

        } // paste::item!
    };
}
