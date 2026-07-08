# Copyright (c) 2023 Graphcore Ltd. All rights reserved.

@0x9b8b9fe63fea8cba;

# Tracked state ################################################################

struct Log @0x835154066f85a612 {
  level     @1 :LogLevel;
  message   @0 :Text;

  # Logging levels to match those of the Rust log::Level
  enum LogLevel {
    trace   @4;
    debug   @3;
    info    @2;
    warn    @1;
    error   @0;
  }
}

struct Capacity @0xac486acbf54c747d {
  units     @1  :Text;
  value     @0  :UInt64;
}

struct Object @0xa7fb0080384f0095 {
  details   @3 :Text;
  type      @2 :UInt8;
  units     @1 :Text;
  size      @0 :UInt64;
}

struct Monitor @0xa021bdb26d110114 {
  name      @0 :Text;
}

struct Entity @0xbc946b85a6484339 {
  name      @0 :Text;
}

struct Lane @0xadd90ea73cb2a0d0 {
  name      @0 :Text;
}

struct Group @0xbff44d59fb8e94de {
  name      @0 :Text;
}

struct BeginActivity @0xaed8c4666e3db85e {
  name      @1 :Text;
  lane      @0 :UInt64;
}

struct Create @0xc95443fd58b475bb {
  union {
    group   @5 :Group;
    lane    @4 :Lane;
    object  @3 :Object;
    monitor @2 :Monitor;
    entity  @1 :Entity;
  }
  id        @0 :UInt64;
}

struct Event @0xc13b4d9cc5ead95b {
  union {
    removeFromGroup @13 :UInt64;
    addToGroup      @12 :UInt64;
    endActivity     @11 :Void;
    beginActivity   @10 :BeginActivity;
    capacity        @9  :Capacity;
    value           @8  :Float64;
    connect         @7  :UInt64;
    time            @6  :Float64;
    enter           @5  :UInt64;
    exit            @4  :UInt64;
    destroy         @3  :UInt64;
    create          @2  :Create;
    log             @1  :Log;
  }
  id        @0 :UInt64;
}
