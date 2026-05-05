# Copyright (c) 2023 Graphcore Ltd. All rights reserved.

@0x9b8b9fe63fea8cba;

# Tracked state ################################################################

struct Log @0x835154066f85a612 {
  level     @1 :LogLevel;
  message   @0 :Text;

  # Logging levels to match those of the Rust log::Level
  enum LogLevel {
    error @0;
    warn  @1;
    info  @2;
    debug @3;
    trace @4;
  }
}

struct Capacity @0xac486acbf54c747d {
  units    @1  :Text;
  value    @0  :UInt64;
}

struct Object @0xa7fb0080384f0095 {
  details  @3 :Text;
  type     @2 :UInt8;
  units    @1 :Text;
  size     @0 :UInt64;
}

struct Monitor @0xa021bdb26d110114 {
  name     @0 :Text;
}

struct Entity @0xbc946b85a6484339 {
  name     @0 :Text;
}

struct Create @0xc95443fd58b475bb {
  union {
    object  @3 :Object;
    monitor @2 :Monitor;
    entity  @1 :Entity;
  }
  id        @0 :UInt64;
}

struct Duration @0xbb9dea1ae271789d {
  nanosecs @1  :UInt32;
  seconds  @0  :UInt32; # Approximately 136 years of range
}

struct Event @0xc13b4d9cc5ead95b {
  union {
    capacity @9 :Capacity;
    value   @8  :Float64;
    connect @7  :UInt64;
    time    @6  :Duration;
    enter   @5  :UInt64;
    exit    @4  :UInt64;
    destroy @3  :UInt64;
    create  @2  :Create;
    log     @1  :Log;
  }
  id        @0 :UInt64;
}
