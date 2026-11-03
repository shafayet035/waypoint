# wp_cli

The `wp` binary. `src/commands/` has one module per command group (`new`, `project` = check and
schema, `docs`, `drive` = run/shot/serve, `test` = test and vehicle test, `info` = systems and
plugins); shared helpers live in `commands/mod.rs`. Each command returns `Ok(exit_code)` or
diagnostics and must support `--json`. Session setup goes through `waypoint::drive::DriveSpec`.
Error-code docs are in `catalog.rs`; `tests/cli.rs` fails if a code used anywhere in `crates/`
is missing there. `wp new` embeds `examples/sandbox` at compile time (`commands/new.rs`).
`src/mcp.rs` is the MCP server (`wp mcp`).
