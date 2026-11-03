# wp_cli

The `wp` binary. Commands live in `commands.rs`; each returns `Ok(exit_code)` or diagnostics.
Every command must support `--json`. Error-code docs are in `catalog.rs`, and
`tests/cli.rs` fails if a code used anywhere in `crates/` is missing there.
`wp new` embeds `examples/sandbox` at compile time.
