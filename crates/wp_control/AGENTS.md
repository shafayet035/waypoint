# wp_control

The control protocol. `Session` owns an `App` and answers JSON-RPC 2.0 (`handle_line`, `handle`,
or `call` for in-process use). `METHODS` is the single source for `rpc.methods`; add new methods
there and in `Session::call`, then list them in `wp docs cli`.

- Entities are strings like `"3v0"`; methods taking `entity` default to `Session::player`.
- Errors: -32700 parse, -32600 invalid request, -32601 unknown method, -32602 bad params,
  -32000 engine error with a diagnostic (WP-K0xx) in `data`.
- `sim.snapshot`/`sim.restore` use `World::snapshot`; only reflected resources are saved, so register
  new simulation resources with `insert_reflected_resource`. Closure state inside systems (for
  example a scenario driver's controller) is not captured.
- `tcp::listen` binds 127.0.0.1 only; never widen it without authentication.
