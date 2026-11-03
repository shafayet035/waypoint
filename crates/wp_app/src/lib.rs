//! `App` and `Plugin`.
//!
//! A game is an [`App`] plus plugins. Every engine feature is a plugin built on
//! the same public API games and modders use. Plugins declare dependencies; the
//! app resolves a deterministic load order and reports clear errors.
//!
//! ```
//! use wp_app::{App, Plugin};
//! struct Hello;
//! impl Plugin for Hello {
//!     fn name(&self) -> &'static str { "hello" }
//!     fn build(&self, app: &mut App) {
//!         app.add_system(wp_ecs::Stage::Post, "say_hello", |_| {});
//!     }
//! }
//! let mut app = App::new();
//! app.add_plugin(Hello);
//! app.finish().unwrap();
//! app.run_ticks(3);
//! assert_eq!(app.time().tick, 3);
//! ```

use serde::Serialize;
use wp_core::{Diagnostic, SimTime};
use wp_ecs::{Events, Schedule, Stage, SystemBuilder, SystemInfo, World};

pub use wp_ecs;

pub trait Plugin: 'static {
    /// Stable, unique name, used in `game.toml` to switch the plugin off.
    fn name(&self) -> &'static str;
    fn version(&self) -> &'static str {
        "0.1.0"
    }
    /// Names of plugins that must be built first.
    fn dependencies(&self) -> Vec<&'static str> {
        Vec::new()
    }
    fn build(&self, app: &mut App);
}

/// A bundle of plugins, such as `waypoint::DefaultPlugins`.
pub trait PluginGroup {
    fn plugins(self) -> Vec<Box<dyn Plugin>>;
}

#[derive(Clone, Debug, Serialize)]
pub struct PluginInfo {
    pub order: usize,
    pub name: String,
    pub version: String,
    pub dependencies: Vec<String>,
}

pub struct App {
    pub world: World,
    schedule: Schedule,
    pending: Vec<Box<dyn Plugin>>,
    disabled: Vec<String>,
    loaded: Vec<PluginInfo>,
    building: Option<&'static str>,
    finished: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut world = World::new();
        world.insert_reflected_resource("SimTime", SimTime::default());
        Self {
            world,
            schedule: Schedule::new(),
            pending: Vec::new(),
            disabled: Vec::new(),
            loaded: Vec::new(),
            building: None,
            finished: false,
        }
    }

    pub fn add_plugin(&mut self, plugin: impl Plugin) -> &mut Self {
        assert!(!self.finished, "add plugins before App::finish");
        self.pending.push(Box::new(plugin));
        self
    }

    pub fn add_plugins(&mut self, group: impl PluginGroup) -> &mut Self {
        assert!(!self.finished, "add plugins before App::finish");
        self.pending.extend(group.plugins());
        self
    }

    /// Switch a plugin off by name (from `game.toml`), before `finish`.
    pub fn disable_plugin(&mut self, name: &str) -> &mut Self {
        self.disabled.push(name.to_string());
        self
    }

    /// Register a system. Inside `Plugin::build`, the plugin's name is recorded.
    pub fn add_system(
        &mut self,
        stage: Stage,
        name: &str,
        run: impl FnMut(&mut World) + 'static,
    ) -> SystemBuilder<'_> {
        let plugin = self.building.unwrap_or("app");
        self.schedule.add(plugin, stage, name, run)
    }

    /// Register an event type; its queue is cleared at the start of every tick.
    pub fn add_event<T: 'static>(&mut self, name: &str) -> &mut Self {
        if !self.world.has_resource::<Events<T>>() {
            self.world.insert_resource(Events::<T>::default());
            self.add_system(Stage::Input, &format!("clear_events:{name}"), |w| {
                w.resource_mut::<Events<T>>().clear()
            });
        }
        self
    }

    /// Resolve plugin order and build every plugin. Errors use stable codes.
    pub fn finish(&mut self) -> Result<(), Diagnostic> {
        if self.finished {
            return Ok(());
        }
        let mut plugins: Vec<Box<dyn Plugin>> = std::mem::take(&mut self.pending);
        for d in &self.disabled {
            if !plugins.iter().any(|p| p.name() == d) {
                return Err(Diagnostic::error(
                    "WP-A004",
                    format!("cannot disable unknown plugin `{d}`"),
                )
                .help(format!("loaded plugins: {}", names(&plugins).join(", "))));
            }
        }
        plugins.retain(|p| !self.disabled.iter().any(|d| d == p.name()));

        let mut seen = std::collections::BTreeSet::new();
        for p in &plugins {
            if !seen.insert(p.name()) {
                return Err(Diagnostic::error(
                    "WP-A002",
                    format!("plugin `{}` was added twice", p.name()),
                )
                .help("remove the duplicate add_plugin call"));
            }
        }
        for p in &plugins {
            for dep in p.dependencies() {
                if !plugins.iter().any(|q| q.name() == dep) {
                    let why = if self.disabled.iter().any(|d| d == dep) {
                        " (it is disabled)"
                    } else {
                        ""
                    };
                    return Err(Diagnostic::error(
                        "WP-A001",
                        format!(
                            "plugin `{}` needs `{dep}`, which is not loaded{why}",
                            p.name()
                        ),
                    )
                    .help(format!(
                        "add the `{dep}` plugin, or also disable `{}`",
                        p.name()
                    )));
                }
            }
        }

        // Topological order; among ready plugins, keep insertion order.
        let mut done: Vec<&'static str> = Vec::new();
        let mut order: Vec<usize> = Vec::new();
        while order.len() < plugins.len() {
            let next = (0..plugins.len()).find(|&i| {
                !order.contains(&i) && plugins[i].dependencies().iter().all(|d| done.contains(d))
            });
            match next {
                Some(i) => {
                    order.push(i);
                    done.push(plugins[i].name());
                }
                None => {
                    let stuck: Vec<_> = (0..plugins.len())
                        .filter(|i| !order.contains(i))
                        .map(|i| plugins[i].name())
                        .collect();
                    return Err(Diagnostic::error(
                        "WP-A003",
                        format!("plugin dependency cycle among: {}", stuck.join(", ")),
                    ));
                }
            }
        }

        for i in order {
            let p = &plugins[i];
            self.building = Some(p.name());
            p.build(self);
            self.building = None;
            self.loaded.push(PluginInfo {
                order: self.loaded.len(),
                name: p.name().to_string(),
                version: p.version().to_string(),
                dependencies: p.dependencies().iter().map(|s| s.to_string()).collect(),
            });
        }
        self.schedule.build().map_err(|e| {
            Diagnostic::error("WP-A010", format!("system schedule is invalid: {e}"))
        })?;
        self.finished = true;
        Ok(())
    }

    /// Advance one gameplay tick (Physics runs `substeps` times inside it).
    pub fn tick(&mut self) {
        assert!(self.finished, "call App::finish before ticking");
        let substeps = self.world.resource::<SimTime>().substeps;
        self.schedule.run_tick(&mut self.world, substeps);
        self.world.resource_mut::<SimTime>().tick += 1;
    }

    pub fn run_ticks(&mut self, n: u64) {
        for _ in 0..n {
            self.tick();
        }
    }

    pub fn time(&self) -> SimTime {
        self.world.resource::<SimTime>().clone()
    }

    pub fn plugins(&self) -> &[PluginInfo] {
        &self.loaded
    }

    pub fn systems(&mut self) -> Vec<SystemInfo> {
        self.schedule.describe().unwrap_or_default()
    }
}

fn names(plugins: &[Box<dyn Plugin>]) -> Vec<&'static str> {
    plugins.iter().map(|p| p.name()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct P(&'static str, Vec<&'static str>);
    impl Plugin for P {
        fn name(&self) -> &'static str {
            self.0
        }
        fn dependencies(&self) -> Vec<&'static str> {
            self.1.clone()
        }
        fn build(&self, _: &mut App) {}
    }

    #[test]
    fn orders_by_dependencies() {
        let mut app = App::new();
        app.add_plugin(P("vehicle", vec!["physics"]))
            .add_plugin(P("physics", vec![]));
        app.finish().unwrap();
        let order: Vec<_> = app.plugins().iter().map(|p| p.name.as_str()).collect();
        assert_eq!(order, vec!["physics", "vehicle"]);
    }

    #[test]
    fn missing_dependency_has_code() {
        let mut app = App::new();
        app.add_plugin(P("vehicle", vec!["physics"]))
            .add_plugin(P("physics", vec![]));
        app.disable_plugin("physics");
        let err = app.finish().unwrap_err();
        assert_eq!(err.code, "WP-A001");
        assert!(err.message.contains("disabled"));
    }

    #[test]
    fn cycle_detected() {
        let mut app = App::new();
        app.add_plugin(P("a", vec!["b"]))
            .add_plugin(P("b", vec!["a"]));
        assert_eq!(app.finish().unwrap_err().code, "WP-A003");
    }
}
