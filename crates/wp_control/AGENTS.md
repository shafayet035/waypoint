# wp_control

The control protocol. `Session` (`session.rs`) owns an `App`, ticks it (`Session::tick` collects
events) and dispatches JSON-RPC 2.0 (`handle_line`, `handle`, or `call` in-process). Method groups:
`session.rs` (sim.*), `world.rs` (world.*, input.set, vehicle.telemetry), `render.rs`
(render.screenshot). `rpc.rs` holds `METHODS` (the single source for `rpc.methods`) and errors;
`json.rs` small helpers; `tcp.rs` the localhost transport.

- Add a method: a `pub(crate) fn` in the right module, an arm in `Session::call`, a row in
  `METHODS`, and a line in `wp docs cli`.
- Entities are strings like `"3v0"`; methods taking `entity` default to the player.
- Errors: -32700 parse, -32600 invalid request, -32601 unknown method, -32602 bad params,
  -32000 engine error with a diagnostic (WP-K0xx) in `data`.
- Snapshots save only reflected resources; register simulation resources with
  `insert_reflected_resource`. Closure state inside systems is not captured.
- `tcp::listen` binds 127.0.0.1 only; never widen it without authentication.
