# electric_shuttle

Add or replace game files by putting them at the same paths here:

- `vehicles/<name>.vehicle.ron`, `scenes/`, `roads/`, `missions/` (+ `.lua`), `tests/`
- `assets/` models, each with its `.asset.ron` licence file

Check with `wp check` (errors point at files in this folder), see what the mod
changes with `wp mod list`, and switch it off in game.toml:

```toml
[mods]
disabled = ["electric_shuttle"]
```
