# wp_data

Loading text data. `load_ron`/`load_toml` return `Loaded<T>` (value + path + source) or a
coded diagnostic with line and column. Use `Loaded::error_at(code, "field", msg)` for semantic
errors so they point at the field. `schema_json::<T>()` backs `wp schema`.
Add `#[serde(deny_unknown_fields)]` to every data struct so typos are errors.
