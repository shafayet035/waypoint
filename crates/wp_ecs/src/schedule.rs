use crate::{Component, World};
use serde::Serialize;
use std::fmt;

/// Explicit stages, run in this order every tick. `Physics` runs once per
/// physics substep (8 times per tick by default).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Stage {
    Input,
    Agents,
    Physics,
    Post,
    RenderExtract,
}

impl Stage {
    pub const ALL: [Stage; 5] = [
        Stage::Input,
        Stage::Agents,
        Stage::Physics,
        Stage::Post,
        Stage::RenderExtract,
    ];
}

type SystemFn = Box<dyn FnMut(&mut World)>;

struct System {
    name: String,
    plugin: String,
    stage: Stage,
    reads: Vec<String>,
    writes: Vec<String>,
    after: Vec<String>,
    before: Vec<String>,
    run: SystemFn,
}

/// Resolved, printable description of one system.
#[derive(Clone, Debug, Serialize)]
pub struct SystemInfo {
    pub order: usize,
    pub name: String,
    pub plugin: String,
    pub stage: Stage,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleError {
    DuplicateSystem(String),
    UnknownOrderingTarget { system: String, target: String },
    CrossStageOrdering { system: String, target: String },
    Cycle(Stage, Vec<String>),
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSystem(n) => write!(f, "two systems are named `{n}`"),
            Self::UnknownOrderingTarget { system, target } => {
                write!(
                    f,
                    "system `{system}` is ordered against `{target}`, which does not exist"
                )
            }
            Self::CrossStageOrdering { system, target } => {
                write!(
                    f,
                    "system `{system}` is ordered against `{target}` in a different stage; stages already order them"
                )
            }
            Self::Cycle(stage, names) => write!(
                f,
                "ordering cycle in stage {stage:?}: {}",
                names.join(" → ")
            ),
        }
    }
}

/// Systems grouped by stage, in a deterministic order: explicit `before`/`after`
/// constraints first, then registration order.
#[derive(Default)]
pub struct Schedule {
    systems: Vec<System>,
    /// Indices into `systems`, per stage, after `build`.
    order: Vec<Vec<usize>>,
    built: bool,
}

/// Returned by [`Schedule::add`] to declare access and ordering.
pub struct SystemBuilder<'a> {
    system: &'a mut System,
}

impl SystemBuilder<'_> {
    pub fn reads<T: Component>(self) -> Self {
        self.system.reads.push(T::NAME.to_string());
        self
    }
    pub fn writes<T: Component>(self) -> Self {
        self.system.writes.push(T::NAME.to_string());
        self
    }
    /// Declare access to a resource (for `wp systems` output and future parallelism).
    pub fn reads_resource(self, name: &str) -> Self {
        self.system.reads.push(format!("res:{name}"));
        self
    }
    pub fn writes_resource(self, name: &str) -> Self {
        self.system.writes.push(format!("res:{name}"));
        self
    }
    pub fn after(self, other: &str) -> Self {
        self.system.after.push(other.to_string());
        self
    }
    pub fn before(self, other: &str) -> Self {
        self.system.before.push(other.to_string());
        self
    }
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(
        &mut self,
        plugin: &str,
        stage: Stage,
        name: &str,
        run: impl FnMut(&mut World) + 'static,
    ) -> SystemBuilder<'_> {
        self.built = false;
        self.systems.push(System {
            name: name.to_string(),
            plugin: plugin.to_string(),
            stage,
            reads: vec![],
            writes: vec![],
            after: vec![],
            before: vec![],
            run: Box::new(run),
        });
        SystemBuilder {
            system: self.systems.last_mut().unwrap(),
        }
    }

    /// Resolve the order. Called automatically by `run_tick` if needed.
    pub fn build(&mut self) -> Result<(), ScheduleError> {
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.systems {
            if !seen.insert(s.name.as_str()) {
                return Err(ScheduleError::DuplicateSystem(s.name.clone()));
            }
        }
        let index_of = |n: &str| self.systems.iter().position(|s| s.name == n);
        let mut order = Vec::new();
        for stage in Stage::ALL {
            let members: Vec<usize> = (0..self.systems.len())
                .filter(|&i| self.systems[i].stage == stage)
                .collect();
            // edges[a] contains b when a must run before b.
            let mut edges: Vec<Vec<usize>> = vec![vec![]; self.systems.len()];
            let mut indeg = vec![0usize; self.systems.len()];
            for &i in &members {
                let s = &self.systems[i];
                for (targets, i_first) in [(&s.after, false), (&s.before, true)] {
                    for t in targets {
                        let j =
                            index_of(t).ok_or_else(|| ScheduleError::UnknownOrderingTarget {
                                system: s.name.clone(),
                                target: t.clone(),
                            })?;
                        if self.systems[j].stage != stage {
                            return Err(ScheduleError::CrossStageOrdering {
                                system: s.name.clone(),
                                target: t.clone(),
                            });
                        }
                        let (a, b) = if i_first { (i, j) } else { (j, i) };
                        edges[a].push(b);
                        indeg[b] += 1;
                    }
                }
            }
            // Kahn's algorithm, always taking the earliest-registered ready system.
            let mut ready: std::collections::BTreeSet<usize> =
                members.iter().copied().filter(|&i| indeg[i] == 0).collect();
            let mut out = Vec::new();
            while let Some(i) = ready.pop_first() {
                out.push(i);
                for &b in &edges[i] {
                    indeg[b] -= 1;
                    if indeg[b] == 0 {
                        ready.insert(b);
                    }
                }
            }
            if out.len() != members.len() {
                let stuck = members
                    .iter()
                    .filter(|i| !out.contains(i))
                    .map(|&i| self.systems[i].name.clone())
                    .collect();
                return Err(ScheduleError::Cycle(stage, stuck));
            }
            order.push(out);
        }
        self.order = order;
        self.built = true;
        Ok(())
    }

    /// Run one gameplay tick: every stage in order, `Physics` `substeps` times.
    pub fn run_tick(&mut self, world: &mut World, substeps: u32) {
        if !self.built {
            self.build()
                .unwrap_or_else(|e| panic!("schedule error: {e}"));
        }
        for (si, stage) in Stage::ALL.iter().enumerate() {
            let reps = if *stage == Stage::Physics {
                substeps.max(1)
            } else {
                1
            };
            for _ in 0..reps {
                for &i in &self.order[si] {
                    (self.systems[i].run)(world);
                }
            }
        }
    }

    /// The resolved order, for `wp systems`.
    pub fn describe(&mut self) -> Result<Vec<SystemInfo>, ScheduleError> {
        if !self.built {
            self.build()?;
        }
        let mut out = Vec::new();
        for stage_order in &self.order {
            for &i in stage_order {
                let s = &self.systems[i];
                out.push(SystemInfo {
                    order: out.len(),
                    name: s.name.clone(),
                    plugin: s.plugin.clone(),
                    stage: s.stage,
                    reads: s.reads.clone(),
                    writes: s.writes.clone(),
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Log(Vec<&'static str>);

    fn sched() -> Schedule {
        let mut s = Schedule::new();
        s.add("t", Stage::Physics, "integrate", |w| {
            w.resource_mut::<Log>().0.push("integrate")
        });
        s.add("t", Stage::Input, "read_input", |w| {
            w.resource_mut::<Log>().0.push("read_input")
        });
        s.add("t", Stage::Physics, "forces", |w| {
            w.resource_mut::<Log>().0.push("forces")
        })
        .before("integrate");
        s
    }

    #[test]
    fn stages_then_constraints_then_registration() {
        let mut s = sched();
        let mut w = World::new();
        w.insert_resource(Log::default());
        s.run_tick(&mut w, 2);
        assert_eq!(
            w.resource::<Log>().0,
            vec!["read_input", "forces", "integrate", "forces", "integrate"]
        );
    }

    #[test]
    fn detects_cycles_and_unknown_targets() {
        let mut s = Schedule::new();
        s.add("t", Stage::Post, "a", |_| {}).after("b");
        s.add("t", Stage::Post, "b", |_| {}).after("a");
        assert!(matches!(s.build(), Err(ScheduleError::Cycle(..))));
        let mut s = Schedule::new();
        s.add("t", Stage::Post, "a", |_| {}).after("ghost");
        assert!(matches!(
            s.build(),
            Err(ScheduleError::UnknownOrderingTarget { .. })
        ));
    }
}
