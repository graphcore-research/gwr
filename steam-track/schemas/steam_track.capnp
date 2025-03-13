@0x9b8b9fe63fea8cba;

using Cxx = import "/capnp/c++.capnp";
$Cxx.namespace("steam_trace");

# Tracked state ################################################################

struct Log @0x835154066f85a612 {
  level     @1  :LogLevel;
  message   @0  :Text;

  # Logging levels to match those of the Rust log::Level
  enum LogLevel {
    error @0;
    warn  @1;
    info  @2;
    debug @3;
    trace @4;
  }
}

struct Entity @0xbc946b85a6484339 {
  name     @3  :Text;
  reqType  @2  :Int8;
  numBytes @1  :UInt64;
  tag      @0  :UInt64;
}

struct Event @0xc13b4d9cc5ead95b {
  union {
    time    @6  :Float64;
    enter   @5  :UInt64;
    exit    @4  :UInt64;
    destroy @3  :UInt64;
    create  @2  :Entity;
    log     @1  :Log;
  }
  tag       @0 :UInt64;
}
