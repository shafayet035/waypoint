use serde::{Deserialize, Serialize};
use std::fmt;

/// A handle to an entity. The generation makes stale handles detectable after an
/// index is reused. Allocation is deterministic: the same operations always
/// produce the same IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Entity {
    index: u32,
    generation: u32,
}

impl Entity {
    pub(crate) fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }
    pub fn index(self) -> u32 {
        self.index
    }
    pub fn generation(self) -> u32 {
        self.generation
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.generation)
    }
}

impl std::str::FromStr for Entity {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (i, g) = s
            .split_once('v')
            .ok_or_else(|| format!("expected an entity like `3v0`, found `{s}`"))?;
        Ok(Entity::new(
            i.parse()
                .map_err(|_| format!("bad entity index in `{s}`"))?,
            g.parse()
                .map_err(|_| format!("bad entity generation in `{s}`"))?,
        ))
    }
}

#[derive(Default)]
pub(crate) struct Entities {
    generations: Vec<u32>,
    alive: Vec<bool>,
    /// Freed indices, reused lowest-first so allocation order is easy to predict.
    free: std::collections::BTreeSet<u32>,
}

impl Entities {
    pub fn alloc(&mut self) -> Entity {
        if let Some(index) = self.free.pop_first() {
            self.alive[index as usize] = true;
            Entity::new(index, self.generations[index as usize])
        } else {
            let index = self.generations.len() as u32;
            self.generations.push(0);
            self.alive.push(true);
            Entity::new(index, 0)
        }
    }

    pub fn free(&mut self, e: Entity) -> bool {
        if !self.is_alive(e) {
            return false;
        }
        let i = e.index() as usize;
        self.alive[i] = false;
        self.generations[i] += 1;
        self.free.insert(e.index());
        true
    }

    pub fn is_alive(&self, e: Entity) -> bool {
        let i = e.index() as usize;
        i < self.alive.len() && self.alive[i] && self.generations[i] == e.generation()
    }

    /// Live entities in ascending index order.
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.alive
            .iter()
            .enumerate()
            .filter(|(_, a)| **a)
            .map(|(i, _)| Entity::new(i as u32, self.generations[i]))
    }

    pub fn len(&self) -> usize {
        self.alive.iter().filter(|a| **a).count()
    }
}
