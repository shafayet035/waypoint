use crate::entity::Entities;
use crate::query::{Query, QueryParam};
use crate::storage::{AnyStorage, Storage};
use crate::{Component, Entity};
use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{BTreeMap, HashMap};

/// Holds all entities, components and resources.
///
/// Structural changes (spawn, insert, remove) need `&mut World`. Reads and
/// in-place writes go through `&World` with runtime borrow checks, so a system
/// can hold a resource and run a query at the same time.
#[derive(Default)]
pub struct World {
    entities: Entities,
    // Looked up by TypeId only; never iterated, so HashMap order cannot leak.
    storages: HashMap<TypeId, RefCell<Box<dyn AnyStorage>>>,
    // Name → TypeId, iterated in name order for reflection.
    names: BTreeMap<&'static str, TypeId>,
    resources: HashMap<TypeId, RefCell<Box<dyn Any>>>,
}

impl World {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- entities -------------------------------------------------------

    pub fn spawn(&mut self) -> Entity {
        self.entities.alloc()
    }

    pub fn despawn(&mut self, e: Entity) -> bool {
        if !self.entities.is_alive(e) {
            return false;
        }
        for s in self.storages.values() {
            s.borrow_mut().remove_entity(e);
        }
        self.entities.free(e)
    }

    pub fn is_alive(&self, e: Entity) -> bool {
        self.entities.is_alive(e)
    }

    /// Live entities in ascending index order.
    pub fn entities(&self) -> Vec<Entity> {
        self.entities.iter().collect()
    }

    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    // ---- components -----------------------------------------------------

    /// Make a component type known to reflection before any entity has it.
    pub fn register<T: Component>(&mut self) {
        let id = TypeId::of::<T>();
        if let Some(prev) = self.names.get(T::NAME) {
            assert!(
                *prev == id,
                "two component types share the name `{}`",
                T::NAME
            );
        }
        self.storages
            .entry(id)
            .or_insert_with(|| RefCell::new(Box::new(Storage::<T>::default())));
        self.names.insert(T::NAME, id);
    }

    pub fn insert<T: Component>(&mut self, e: Entity, component: T) {
        assert!(
            self.entities.is_alive(e),
            "insert {} on dead entity {e}",
            T::NAME
        );
        self.register::<T>();
        self.storage_mut::<T>()
            .expect("registered")
            .insert(e, component);
    }

    pub fn remove<T: Component>(&mut self, e: Entity) -> Option<T> {
        self.storage_mut::<T>()?.remove(e)
    }

    pub fn has<T: Component>(&self, e: Entity) -> bool {
        self.get::<T>(e).is_some()
    }

    pub fn get<T: Component>(&self, e: Entity) -> Option<Ref<'_, T>> {
        let s = self.storage::<T>()?;
        Ref::filter_map(s, |s| s.get(e)).ok()
    }

    pub fn get_mut<T: Component>(&self, e: Entity) -> Option<RefMut<'_, T>> {
        let s = self.storage_mut::<T>()?;
        RefMut::filter_map(s, |s| s.get_mut(e)).ok()
    }

    pub(crate) fn storage<T: Component>(&self) -> Option<Ref<'_, Storage<T>>> {
        let cell = self.storages.get(&TypeId::of::<T>())?;
        let b = cell.try_borrow().unwrap_or_else(|_| {
            panic!(
                "component {} is already mutably borrowed (same type twice in one query?)",
                T::NAME
            )
        });
        Some(Ref::map(b, |b| {
            b.as_any()
                .downcast_ref::<Storage<T>>()
                .expect("storage type")
        }))
    }

    pub(crate) fn storage_mut<T: Component>(&self) -> Option<RefMut<'_, Storage<T>>> {
        let cell = self.storages.get(&TypeId::of::<T>())?;
        let b = cell.try_borrow_mut().unwrap_or_else(|_| {
            panic!(
                "component {} is already borrowed (same type twice in one query?)",
                T::NAME
            )
        });
        Some(RefMut::map(b, |b| {
            b.as_any_mut()
                .downcast_mut::<Storage<T>>()
                .expect("storage type")
        }))
    }

    pub fn query<Q: QueryParam>(&self) -> Query<'_, Q> {
        Query::new(self)
    }

    // ---- resources ------------------------------------------------------

    pub fn insert_resource<R: 'static>(&mut self, r: R) {
        self.resources
            .insert(TypeId::of::<R>(), RefCell::new(Box::new(r)));
    }

    pub fn remove_resource<R: 'static>(&mut self) -> Option<R> {
        let cell = self.resources.remove(&TypeId::of::<R>())?;
        cell.into_inner().downcast::<R>().ok().map(|b| *b)
    }

    pub fn has_resource<R: 'static>(&self) -> bool {
        self.resources.contains_key(&TypeId::of::<R>())
    }

    pub fn get_resource<R: 'static>(&self) -> Option<Ref<'_, R>> {
        let cell = self.resources.get(&TypeId::of::<R>())?;
        Some(Ref::map(cell.borrow(), |b| {
            b.downcast_ref::<R>().expect("resource type")
        }))
    }

    pub fn get_resource_mut<R: 'static>(&self) -> Option<RefMut<'_, R>> {
        let cell = self.resources.get(&TypeId::of::<R>())?;
        Some(RefMut::map(cell.borrow_mut(), |b| {
            b.downcast_mut::<R>().expect("resource type")
        }))
    }

    /// Panics with the resource's type name if it is missing.
    pub fn resource<R: 'static>(&self) -> Ref<'_, R> {
        self.get_resource::<R>().unwrap_or_else(|| {
            panic!(
                "missing resource {}; is its plugin added?",
                std::any::type_name::<R>()
            )
        })
    }

    pub fn resource_mut<R: 'static>(&self) -> RefMut<'_, R> {
        self.get_resource_mut::<R>().unwrap_or_else(|| {
            panic!(
                "missing resource {}; is its plugin added?",
                std::any::type_name::<R>()
            )
        })
    }

    // ---- reflection -----------------------------------------------------

    /// Registered component names in sorted order.
    pub fn component_names(&self) -> Vec<&'static str> {
        self.names.keys().copied().collect()
    }

    /// All components of one entity as `{ "Name": value, ... }`, sorted by name.
    pub fn entity_json(&self, e: Entity) -> Option<serde_json::Value> {
        if !self.is_alive(e) {
            return None;
        }
        let mut map = serde_json::Map::new();
        for (name, id) in &self.names {
            if let Some(v) = self.storages[id].borrow().get_json(e) {
                map.insert((*name).to_string(), v);
            }
        }
        Some(serde_json::Value::Object(map))
    }

    pub fn get_component_json(&self, e: Entity, name: &str) -> Option<serde_json::Value> {
        let id = self.names.get(name)?;
        self.storages[id].borrow().get_json(e)
    }

    /// Set a component from JSON by its reflected name.
    pub fn set_component_json(
        &mut self,
        e: Entity,
        name: &str,
        value: serde_json::Value,
    ) -> Result<(), String> {
        if !self.is_alive(e) {
            return Err(format!("entity {e} does not exist"));
        }
        let id = self.names.get(name).ok_or_else(|| {
            format!(
                "unknown component `{name}`; known: {}",
                self.component_names().join(", ")
            )
        })?;
        self.storages[id].borrow_mut().set_json(e, value)
    }

    /// Every entity and its components, in entity order. Stable across runs, so
    /// it is what state hashes and snapshots are built from.
    pub fn to_json(&self) -> serde_json::Value {
        let list: Vec<serde_json::Value> = self
            .entities
            .iter()
            .map(|e| {
                serde_json::json!({ "entity": e.to_string(), "components": self.entity_json(e).unwrap() })
            })
            .collect();
        serde_json::Value::Array(list)
    }

    /// A 64-bit fingerprint of all component data. Same build + same inputs →
    /// same hash; the first differing tick shows where two runs diverged.
    pub fn state_hash(&self) -> String {
        let bytes = serde_json::to_vec(&self.to_json()).expect("world serializes");
        wp_core::hash::hex_hash(&bytes)
    }

    #[cfg(test)]
    pub(crate) fn component_count<T: Component>(&self) -> usize {
        self.storage::<T>().map(|s| s.dense.len()).unwrap_or(0)
    }

    /// True if `e` has the component registered under `name`.
    pub fn has_component_named(&self, name: &str, e: Entity) -> bool {
        self.names
            .get(name)
            .is_some_and(|id| self.storages[id].borrow().contains(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Pos(f32, f32);
    impl Component for Pos {
        const NAME: &'static str = "Pos";
    }
    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Vel(f32, f32);
    impl Component for Vel {
        const NAME: &'static str = "Vel";
    }

    #[test]
    fn spawn_insert_query() {
        let mut w = World::new();
        let a = w.spawn();
        let b = w.spawn();
        w.insert(a, Pos(0.0, 0.0));
        w.insert(a, Vel(1.0, 2.0));
        w.insert(b, Pos(5.0, 5.0));
        w.query::<(&mut Pos, &Vel)>().for_each(|_, (p, v)| {
            p.0 += v.0;
            p.1 += v.1;
        });
        assert_eq!(*w.get::<Pos>(a).unwrap(), Pos(1.0, 2.0));
        assert_eq!(*w.get::<Pos>(b).unwrap(), Pos(5.0, 5.0));
        assert_eq!(w.query::<&Pos>().entities(), vec![a, b]);
        assert_eq!(w.component_count::<Vel>(), 1);
    }

    #[test]
    fn despawn_and_stale_handles() {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Pos(1.0, 1.0));
        assert!(w.despawn(a));
        let a2 = w.spawn();
        assert_eq!(a2.index(), a.index());
        assert_ne!(a2.generation(), a.generation());
        assert!(w.get::<Pos>(a).is_none());
        assert!(!w.has_component_named("Pos", a2));
    }

    #[test]
    fn reflection_round_trip() {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Pos(1.0, 2.0));
        let json = w.entity_json(a).unwrap();
        assert_eq!(json, serde_json::json!({ "Pos": [1.0, 2.0] }));
        w.set_component_json(a, "Pos", serde_json::json!([3.0, 4.0]))
            .unwrap();
        assert_eq!(*w.get::<Pos>(a).unwrap(), Pos(3.0, 4.0));
        assert!(
            w.set_component_json(a, "Nope", serde_json::json!(1))
                .is_err()
        );
    }

    #[test]
    fn state_hash_is_stable() {
        let build = || {
            let mut w = World::new();
            for i in 0..10 {
                let e = w.spawn();
                w.insert(e, Pos(i as f32, 0.5));
            }
            w.state_hash()
        };
        assert_eq!(build(), build());
    }

    #[test]
    #[should_panic(expected = "already")]
    fn same_type_twice_panics() {
        let mut w = World::new();
        let a = w.spawn();
        w.insert(a, Pos(0.0, 0.0));
        w.query::<(&mut Pos, &Pos)>().for_each(|_, _| {});
    }

    #[test]
    fn resources() {
        let mut w = World::new();
        w.insert_resource(5u32);
        *w.resource_mut::<u32>() += 1;
        assert_eq!(*w.resource::<u32>(), 6);
        assert_eq!(w.remove_resource::<u32>(), Some(6));
        assert!(!w.has_resource::<u32>());
    }
}
