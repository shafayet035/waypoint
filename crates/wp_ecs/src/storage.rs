use crate::{Component, Entity};
use std::any::Any;

const NONE: u32 = u32::MAX;

/// Sparse-set storage: dense arrays for fast iteration, a sparse index for O(1)
/// lookup. Iteration order depends only on the sequence of inserts and removes,
/// so it is deterministic.
/// Component storage for one type. Opaque outside this crate.
pub struct Storage<T> {
    pub(crate) dense: Vec<T>,
    pub(crate) owners: Vec<Entity>,
    sparse: Vec<u32>,
}

impl<T> Default for Storage<T> {
    fn default() -> Self {
        Self {
            dense: Vec::new(),
            owners: Vec::new(),
            sparse: Vec::new(),
        }
    }
}

impl<T> Storage<T> {
    fn slot(&self, e: Entity) -> Option<usize> {
        let s = *self.sparse.get(e.index() as usize)?;
        (s != NONE && self.owners[s as usize] == e).then_some(s as usize)
    }

    pub fn insert(&mut self, e: Entity, value: T) {
        let i = e.index() as usize;
        if i >= self.sparse.len() {
            self.sparse.resize(i + 1, NONE);
        }
        let s = self.sparse[i];
        if s != NONE {
            // Same index: overwrite (the previous owner, if stale, is replaced).
            self.dense[s as usize] = value;
            self.owners[s as usize] = e;
        } else {
            self.sparse[i] = self.dense.len() as u32;
            self.dense.push(value);
            self.owners.push(e);
        }
    }

    pub fn remove(&mut self, e: Entity) -> Option<T> {
        let s = self.slot(e)?;
        let value = self.dense.swap_remove(s);
        self.owners.swap_remove(s);
        if s < self.owners.len() {
            let moved = self.owners[s];
            self.sparse[moved.index() as usize] = s as u32;
        }
        self.sparse[e.index() as usize] = NONE;
        Some(value)
    }

    pub fn get(&self, e: Entity) -> Option<&T> {
        self.slot(e).map(|s| &self.dense[s])
    }

    pub fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        self.slot(e).map(|s| &mut self.dense[s])
    }
}

/// Type-erased access used by the world for despawning and reflection.
pub(crate) trait AnyStorage: Any {
    fn remove_entity(&mut self, e: Entity);
    fn clear(&mut self);
    /// (owner, value) pairs in dense (iteration) order.
    fn dump_json(&self) -> Vec<(Entity, serde_json::Value)>;
    fn contains(&self, e: Entity) -> bool;
    fn get_json(&self, e: Entity) -> Option<serde_json::Value>;
    fn set_json(&mut self, e: Entity, v: serde_json::Value) -> Result<(), String>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Component> AnyStorage for Storage<T> {
    fn clear(&mut self) {
        self.dense.clear();
        self.owners.clear();
        self.sparse.clear();
    }
    fn dump_json(&self) -> Vec<(Entity, serde_json::Value)> {
        self.owners
            .iter()
            .zip(&self.dense)
            .map(|(e, v)| {
                (
                    *e,
                    serde_json::to_value(v).expect("component serializes to JSON"),
                )
            })
            .collect()
    }
    fn remove_entity(&mut self, e: Entity) {
        self.remove(e);
    }
    fn contains(&self, e: Entity) -> bool {
        self.slot(e).is_some()
    }
    fn get_json(&self, e: Entity) -> Option<serde_json::Value> {
        self.get(e)
            .map(|v| serde_json::to_value(v).expect("component serializes to JSON"))
    }
    fn set_json(&mut self, e: Entity, v: serde_json::Value) -> Result<(), String> {
        let value: T = serde_json::from_value(v).map_err(|err| format!("{}: {err}", T::NAME))?;
        self.insert(e, value);
        Ok(())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_remove_keeps_index_consistent() {
        let mut s = Storage::<u32>::default();
        let (a, b, c) = (Entity::new(0, 0), Entity::new(1, 0), Entity::new(2, 0));
        s.insert(a, 10);
        s.insert(b, 11);
        s.insert(c, 12);
        assert_eq!(s.remove(a), Some(10));
        assert_eq!(s.get(c), Some(&12));
        assert_eq!(s.get(b), Some(&11));
        assert_eq!(s.get(a), None);
        // A stale handle with an old generation must not see the new value.
        s.insert(Entity::new(0, 1), 20);
        assert_eq!(s.get(a), None);
        assert_eq!(s.get(Entity::new(0, 1)), Some(&20));
    }
}
