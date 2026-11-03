use crate::storage::Storage;
use crate::{Component, Entity, World};
use std::cell::{Ref, RefMut};
use std::marker::PhantomData;

/// Something a query can fetch per entity: `&T`, `&mut T`, or a tuple of them.
pub trait QueryParam {
    type State<'w>;
    type Item<'s>;
    fn borrow(world: &World) -> Option<Self::State<'_>>;
    /// The entity list of the smallest storage, used to drive iteration.
    fn owners<'s>(state: &'s Self::State<'_>) -> &'s [Entity];
    fn fetch<'s>(state: &'s mut Self::State<'_>, e: Entity) -> Option<Self::Item<'s>>;
}

impl<T: Component> QueryParam for &T {
    type State<'w> = Ref<'w, Storage<T>>;
    type Item<'s> = &'s T;
    fn borrow(world: &World) -> Option<Self::State<'_>> {
        world.storage::<T>()
    }
    fn owners<'s>(state: &'s Self::State<'_>) -> &'s [Entity] {
        &state.owners
    }
    fn fetch<'s>(state: &'s mut Self::State<'_>, e: Entity) -> Option<Self::Item<'s>> {
        state.get(e)
    }
}

impl<T: Component> QueryParam for &mut T {
    type State<'w> = RefMut<'w, Storage<T>>;
    type Item<'s> = &'s mut T;
    fn borrow(world: &World) -> Option<Self::State<'_>> {
        world.storage_mut::<T>()
    }
    fn owners<'s>(state: &'s Self::State<'_>) -> &'s [Entity] {
        &state.owners
    }
    fn fetch<'s>(state: &'s mut Self::State<'_>, e: Entity) -> Option<Self::Item<'s>> {
        state.get_mut(e)
    }
}

macro_rules! tuple_query {
    ($($name:ident $idx:tt),+) => {
        impl<$($name: QueryParam),+> QueryParam for ($($name,)+) {
            type State<'w> = ($($name::State<'w>,)+);
            type Item<'s> = ($($name::Item<'s>,)+);
            fn borrow(world: &World) -> Option<Self::State<'_>> {
                Some(($($name::borrow(world)?,)+))
            }
            fn owners<'s>(state: &'s Self::State<'_>) -> &'s [Entity] {
                let mut best: &'s [Entity] = &[];
                let mut first = true;
                $(
                    let o = $name::owners(&state.$idx);
                    if first || o.len() < best.len() { best = o; first = false; }
                )+
                let _ = first;
                best
            }
            fn fetch<'s>(state: &'s mut Self::State<'_>, e: Entity) -> Option<Self::Item<'s>> {
                Some(($($name::fetch(&mut state.$idx, e)?,)+))
            }
        }
    };
}

tuple_query!(A 0);
tuple_query!(A 0, B 1);
tuple_query!(A 0, B 1, C 2);
tuple_query!(A 0, B 1, C 2, D 3);
tuple_query!(A 0, B 1, C 2, D 3, E 4);
tuple_query!(A 0, B 1, C 2, D 3, E 4, F 5);

/// A query over all entities that have every component in `Q`.
/// Entities are visited in the deterministic order of the smallest storage.
pub struct Query<'w, Q: QueryParam> {
    world: &'w World,
    _q: PhantomData<Q>,
}

impl<'w, Q: QueryParam> Query<'w, Q> {
    pub(crate) fn new(world: &'w World) -> Self {
        Self {
            world,
            _q: PhantomData,
        }
    }

    pub fn for_each<F>(self, mut f: F)
    where
        F: for<'s> FnMut(Entity, Q::Item<'s>),
    {
        let Some(mut state) = Q::borrow(self.world) else {
            return;
        };
        let candidates = Q::owners(&state).to_vec();
        for e in candidates {
            if let Some(item) = Q::fetch(&mut state, e) {
                f(e, item);
            }
        }
    }

    /// Entities matching the query, in iteration order.
    pub fn entities(self) -> Vec<Entity> {
        let mut out = Vec::new();
        let Some(mut state) = Q::borrow(self.world) else {
            return out;
        };
        let candidates = Q::owners(&state).to_vec();
        for e in candidates {
            if Q::fetch(&mut state, e).is_some() {
                out.push(e);
            }
        }
        out
    }

    /// Run `f` for one entity, if it matches.
    pub fn get<R>(self, e: Entity, f: impl for<'s> FnOnce(Q::Item<'s>) -> R) -> Option<R> {
        let mut state = Q::borrow(self.world)?;
        Q::fetch(&mut state, e).map(f)
    }
}
