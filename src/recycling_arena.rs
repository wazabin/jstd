//! Typed slab storage whose removed slots are reused.
//!
//! [`RecyclingArena`] is intended for derived, body-local bookkeeping where an
//! identifier is only retained while its payload is live. Removal links the
//! vacant slot into an internal free list, and the next insertion reuses it.
//! Unlike [`StableArena`](crate::stable_arena::StableArena), identifiers are
//! therefore not permanently unique.

use std::{
    marker::PhantomData,
    ops::{Index, IndexMut},
};

use crate::registry::{Identified, Identifier};

/// One physical slot in a [`RecyclingArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot<Id, T> {
    Live(T),
    Free(Option<Id>),
}

/// A typed slab that reuses removed slots through an intrusive free list.
///
/// IDs remain stable while their payload is live, but may identify a different
/// payload after removal and reinsertion. They must not escape the owner whose
/// mutation controls the arena.
#[derive(Debug, Clone)]
pub struct RecyclingArena<Id: Identifier, T> {
    slots: Vec<Slot<Id, T>>,
    free: Option<Id>,
    live: usize,
    marker: PhantomData<Id>,
}

impl<Id: Identifier, T> RecyclingArena<Id, T> {
    /// Inserts `value`, reusing the most recently removed slot when possible.
    pub fn push(&mut self, value: T) -> Id {
        let id = match self.free {
            Some(id) => {
                let slot = &mut self.slots[id.into()];
                let Slot::Free(next) = *slot else {
                    unreachable!("arena slot {id:?} is on the free list but live");
                };
                self.free = next;
                *slot = Slot::Live(value);
                id
            }
            None => {
                let raw = self.slots.len();
                let id = Id::from(raw);
                assert_eq!(
                    Into::<usize>::into(id),
                    raw,
                    "RecyclingArena identifier space exhausted"
                );
                self.slots.push(Slot::Live(value));
                id
            }
        };
        self.live += 1;
        id
    }

    /// Removes and returns `id`'s payload.
    ///
    /// # Panics
    /// Panics if `id` is out of bounds or already free.
    pub fn remove(&mut self, id: Id) -> T {
        let slot = &mut self.slots[id.into()];
        if matches!(slot, Slot::Free(_)) {
            panic!("arena slot {id:?} is already free");
        }
        let Slot::Live(value) = std::mem::replace(slot, Slot::Free(self.free)) else {
            unreachable!();
        };
        self.free = Some(id);
        self.live -= 1;
        value
    }

    /// Returns the number of live payloads.
    pub fn len(&self) -> usize {
        self.live
    }

    /// Returns whether the arena contains no live payloads.
    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    /// Returns whether `id` currently names a live payload.
    pub fn contains(&self, id: Id) -> bool {
        matches!(self.slots.get(id.into()), Some(Slot::Live(_)))
    }

    /// Returns the live payload identified by `id`, if any.
    pub fn get(&self, id: Id) -> Option<Identified<Id, &T>> {
        match self.slots.get(id.into())? {
            Slot::Live(value) => Some(Identified::new(id, value)),
            Slot::Free(_) => None,
        }
    }

    /// Returns the live payload identified by `id` mutably, if any.
    pub fn get_mut(&mut self, id: Id) -> Option<Identified<Id, &mut T>> {
        match self.slots.get_mut(id.into())? {
            Slot::Live(value) => Some(Identified::new(id, value)),
            Slot::Free(_) => None,
        }
    }

    /// Iterates over live payloads with their IDs in slot order.
    pub fn iter(&self) -> impl Iterator<Item = Identified<Id, &T>> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(raw, slot)| match slot {
                Slot::Live(value) => Some(Identified::new(Id::from(raw), value)),
                Slot::Free(_) => None,
            })
    }

    /// Removes every payload and resets identifier allocation to zero.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.free = None;
        self.live = 0;
    }

    /// Releases capacity beyond the number of issued slots.
    pub fn shrink_to_fit(&mut self) {
        self.slots.shrink_to_fit();
    }
}

impl<Id: Identifier, T> Default for RecyclingArena<Id, T> {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            free: None,
            live: 0,
            marker: PhantomData,
        }
    }
}

impl<Id: Identifier, T> Index<Id> for RecyclingArena<Id, T> {
    type Output = T;

    fn index(&self, id: Id) -> &Self::Output {
        self.get(id)
            .unwrap_or_else(|| panic!("arena slot {id:?} is free"))
            .inner
    }
}

impl<Id: Identifier, T> IndexMut<Id> for RecyclingArena<Id, T> {
    fn index_mut(&mut self, id: Id) -> &mut Self::Output {
        self.get_mut(id)
            .unwrap_or_else(|| panic!("arena slot {id:?} is free"))
            .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Identifier;

    #[derive(Identifier)]
    struct TestId(u32);

    #[test]
    fn removed_slots_are_reused_last_first() {
        let mut arena = RecyclingArena::<TestId, _>::default();
        let a = arena.push("a");
        let b = arena.push("b");
        let c = arena.push("c");
        assert_eq!(arena.remove(b), "b");
        assert_eq!(arena.remove(a), "a");

        assert_eq!(arena.push("d"), a);
        assert_eq!(arena.push("e"), b);
        assert_eq!(arena.push("f"), TestId::from(3));
        assert_eq!(
            arena.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            vec![a, b, c, TestId::from(3)]
        );
    }

    #[test]
    fn access_mutation_iteration_and_clear_cover_live_and_free_slots() {
        let mut arena = RecyclingArena::<TestId, String>::default();
        assert!(arena.is_empty());
        assert_eq!(arena.len(), 0);

        let a = arena.push("a".into());
        let b = arena.push("b".into());
        let c = arena.push("c".into());
        assert_eq!(arena.len(), 3);
        assert!(arena.contains(a));
        assert_eq!(arena.get(a).map(|entry| entry.as_str()), Some("a"));

        arena.get_mut(a).unwrap().push('!');
        arena[c].push('?');
        assert_eq!(&arena[a], "a!");
        assert_eq!(&arena[c], "c?");

        assert_eq!(arena.remove(b), "b");
        assert_eq!(arena.len(), 2);
        assert!(!arena.is_empty());
        assert!(!arena.contains(b));
        assert!(arena.get(b).is_none());
        assert!(arena.get_mut(b).is_none());
        assert!(!arena.contains(TestId::from(99)));
        assert!(arena.get(TestId::from(99)).is_none());

        let entries: Vec<_> = arena
            .iter()
            .map(|entry| (entry.id, entry.as_str()))
            .collect();
        assert_eq!(entries, vec![(a, "a!"), (c, "c?")]);

        arena.shrink_to_fit();
        assert_eq!(&arena[a], "a!");
        arena.clear();
        assert!(arena.is_empty());
        assert_eq!(arena.len(), 0);
        assert!(!arena.contains(a));
        assert_eq!(arena.push("fresh".into()), TestId::from(0));
    }

    #[test]
    fn mixed_operations_match_an_option_vec_model() {
        let mut arena = RecyclingArena::<TestId, usize>::default();
        let mut model: Vec<Option<usize>> = Vec::new();
        let mut live: Vec<usize> = Vec::new();
        let mut free: Vec<usize> = Vec::new();

        for value in 0..200 {
            if value % 3 == 2 && !live.is_empty() {
                let raw = live.remove(0);
                assert_eq!(arena.remove(TestId::from(raw)), model[raw].take().unwrap());
                free.push(raw);
            } else {
                let expected = free.pop().unwrap_or(model.len());
                if expected == model.len() {
                    model.push(Some(value));
                } else {
                    model[expected] = Some(value);
                }
                assert_eq!(arena.push(value), TestId::from(expected));
                live.push(expected);
            }

            assert_eq!(arena.len(), model.iter().flatten().count());
            assert_eq!(
                arena
                    .iter()
                    .map(|entry| (usize::from(entry.id), *entry.inner))
                    .collect::<Vec<_>>(),
                model
                    .iter()
                    .enumerate()
                    .filter_map(|(id, value)| value.map(|value| (id, value)))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    #[should_panic(expected = "is already free")]
    fn double_remove_panics() {
        let mut arena = RecyclingArena::<TestId, _>::default();
        let id = arena.push(1);
        arena.remove(id);
        arena.remove(id);
    }

    #[test]
    #[should_panic(expected = "is free")]
    fn shared_indexing_a_free_slot_panics() {
        let mut arena = RecyclingArena::<TestId, _>::default();
        let id = arena.push(1);
        arena.remove(id);
        let _ = arena[id];
    }

    #[test]
    #[should_panic(expected = "is free")]
    fn mutable_indexing_a_free_slot_panics() {
        let mut arena = RecyclingArena::<TestId, _>::default();
        let id = arena.push(1);
        arena.remove(id);
        arena[id] = 2;
    }
}
