# Nylon Ring Examples

This directory contains simple examples demonstrating how to use nylon-ring.

## Examples

### ex-nyring-plugin

A simple plugin library (cdylib) with two handlers:
- **echo**: Returns the input data unchanged
- **uppercase**: Converts input text to uppercase

### ex-nyring-host

A host application that loads the plugin and demonstrates three usage patterns:
1. Simple echo call
2. Text transformation (uppercase)
3. Multiple sequential calls

## Running the Demo

```bash
# From the root of nylon-ring project
cargo run --manifest-path examples/ex-nyring-host/Cargo.toml
```

The demo will:
1. Build the plugin automatically
2. Load the plugin library
3. Execute three demos showing different usage of the `call` method
4. Clean up resources

## Expected Output

```
=== Nylon Ring Demo ===

Building plugin...
Loading plugin from: target/debug/libex_nyring_plugin.dylib

[Plugin] Initialized!
--- Demo 1: Echo ---
Sending: Hello, Nylon Ring!
[Plugin] Echo received: Hello, Nylon Ring!
Status: Ok
Response: Hello, Nylon Ring!

--- Demo 2: Uppercase ---
Sending: make me loud
[Plugin] Uppercase received, sending back: MAKE ME LOUD
Status: Ok
Response: MAKE ME LOUD

--- Demo 3: Multiple Calls ---
[Plugin] Echo received: Message #1
Call 1: Ok
[Plugin] Echo received: Message #2
Call 2: Ok
...

=== Demo Complete ===
[Plugin] Shutting down!
```

## Key Features Demonstrated

- **Plugin Loading**: Dynamic library loading with libloading
- **Async Call Pattern**: Using the `call` method for request/response
- **Plugin State**: Storing and using host context and vtable
- **Error Handling**: Panic catching in both plugin and host
- **Resource Management**: Proper initialization and shutdown

