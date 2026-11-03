# wp_app

`App` + `Plugin`. Plugins declare `dependencies()` by name; `App::finish()` resolves the order
(errors WP-A001..A004, A010), builds each plugin, then validates the schedule.
Inside `Plugin::build`, `app.add_system(stage, name, fn)` records the plugin name for `wp systems`.
Use `add_event::<T>(name)` for per-tick event queues.
