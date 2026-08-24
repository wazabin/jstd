//! Dense payload storage with stable, never-reused logical identifiers.
//!
//! [`StableArena`] separates identity from physical position. Removing an entry
//! eagerly drops its payload with `swap_remove`, while a location table keeps
//! every surviving identifier valid. Unlike [`Registry`](crate::registry::Registry),
//! payload addresses are not stable across mutation.

use std::{
    fmt::{Debug, Formatter},
    iter::Zip,
    ops::{Index, IndexMut},
    slice, vec,
};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::registry::{Identified, Identifier};

const DEAD: usize = usize::MAX;

/// Stable logical IDs over compact, dense physical payload storage.
#[derive(Clone, PartialEq, Eq)]
pub struct StableArena<Id: Identifier, T> {
    values: Vec<T>,
    slot_ids: Vec<Id>,
    locations: Vec<usize>,
}

impl<Id: Identifier, T> StableArena<Id, T> {
    /// Inserts `value` under a fresh, monotonic identifier.
    pub fn push(&mut self, value: T) -> Id {
        let raw = self.locations.len();
        assert!(raw < DEAD, "StableArena identifier space exhausted");
        let id = Id::from(raw);
        assert_eq!(
            Into::<usize>::into(id),
            raw,
            "StableArena identifier does not round-trip through its backing type"
        );

        let slot = self.values.len();
        assert!(slot < DEAD, "StableArena physical slot space exhausted");
        self.values.push(value);
        self.slot_ids.push(id);
        self.locations.push(slot);
        id
    }

    /// Returns the number of live payloads.
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns the number of logical IDs issued, including removed IDs.
    pub fn issued_len(&self) -> usize {
        self.locations.len()
    }

    /// Returns the payload capacity reserved by the dense value vector.
    pub fn capacity(&self) -> usize {
        self.values.capacity()
    }

    /// Returns bytes reserved by the arena's three structural vectors.
    ///
    /// Allocations owned by `T` itself are deliberately excluded.
    pub fn structural_bytes(&self) -> usize {
        self.values
            .capacity()
            .saturating_mul(std::mem::size_of::<T>())
            .saturating_add(
                self.slot_ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Id>()),
            )
            .saturating_add(
                self.locations
                    .capacity()
                    .saturating_mul(std::mem::size_of::<usize>()),
            )
    }

    /// Returns `true` when no live payload remains.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Releases structural capacity retained beyond the current live/issued
    /// lengths.
    ///
    /// Identifiers, liveness, and dense physical order are unchanged; only
    /// allocator capacity is returned. Intended for explicit end-of-mutation
    /// boundaries — shrinking after every removal would reallocate on the next
    /// growth.
    pub fn shrink_to_fit(&mut self) {
        self.values.shrink_to_fit();
        self.slot_ids.shrink_to_fit();
        self.locations.shrink_to_fit();
    }

    /// Returns whether `id` currently names a live payload.
    pub fn contains(&self, id: Id) -> bool {
        self.live_slot(id).is_some()
    }

    /// Returns the live payload identified by `id`, if any.
    pub fn get(&self, id: Id) -> Option<Identified<Id, &T>> {
        let slot = self.live_slot(id)?;
        Some(Identified::new(id, &self.values[slot]))
    }

    /// Returns the live payload identified by `id` mutably, if any.
    pub fn get_mut(&mut self, id: Id) -> Option<Identified<Id, &mut T>> {
        let slot = self.live_slot(id)?;
        Some(Identified::new(id, &mut self.values[slot]))
    }

    /// Removes and returns `id`'s payload while preserving every other ID.
    ///
    /// # Panics
    ///
    /// Panics when `id` is unknown or was already removed.
    pub fn remove(&mut self, id: Id) -> T {
        let raw = id.into();
        let slot = self.live_slot(id).unwrap_or_else(|| {
            panic!(
                "StableArena::remove: dead or unknown id {id:?} (issued {}, live {})",
                self.issued_len(),
                self.len()
            )
        });

        self.locations[raw] = DEAD;
        let removed = self.values.swap_remove(slot);
        let removed_id = self.slot_ids.swap_remove(slot);
        debug_assert_eq!(removed_id, id);

        if slot < self.values.len() {
            let moved_id = self.slot_ids[slot];
            self.locations[Into::<usize>::into(moved_id)] = slot;
        }
        removed
    }

    /// Iterates over live entries in dense physical order.
    pub fn iter(&self) -> Iter<'_, Id, T> {
        Iter {
            inner: self.slot_ids.iter().zip(self.values.iter()),
        }
    }

    /// Iterates mutably over live entries in dense physical order.
    pub fn iter_mut(&mut self) -> IterMut<'_, Id, T> {
        IterMut {
            inner: self.slot_ids.iter().zip(self.values.iter_mut()),
        }
    }

    #[inline]
    fn live_slot(&self, id: Id) -> Option<usize> {
        let slot = *self.locations.get(Into::<usize>::into(id))?;
        (slot != DEAD).then_some(slot)
    }
}

impl<Id: Identifier, T> Default for StableArena<Id, T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            slot_ids: Vec::new(),
            locations: Vec::new(),
        }
    }
}

impl<Id: Identifier, T: Debug> Debug for StableArena<Id, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StableArena")
            .field("issued", &self.issued_len())
            .field(
                "entries",
                &self
                    .iter()
                    .map(|entry| (entry.id, entry.inner))
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl<Id: Identifier, T> Index<Id> for StableArena<Id, T> {
    type Output = T;

    fn index(&self, id: Id) -> &Self::Output {
        self.get(id).map(|entry| entry.inner).unwrap_or_else(|| {
            panic!(
                "StableArena index: dead or unknown id {id:?} (issued {}, live {})",
                self.issued_len(),
                self.len()
            )
        })
    }
}

impl<Id: Identifier, T> IndexMut<Id> for StableArena<Id, T> {
    fn index_mut(&mut self, id: Id) -> &mut Self::Output {
        let issued = self.issued_len();
        let live = self.len();
        self.get_mut(id)
            .map(|entry| entry.inner)
            .unwrap_or_else(|| {
                panic!(
                    "StableArena mutable index: dead or unknown id {id:?} (issued {issued}, live {live})"
                )
            })
    }
}

/// Immutable dense-order iterator.
pub struct Iter<'a, Id: Identifier, T> {
    inner: Zip<slice::Iter<'a, Id>, slice::Iter<'a, T>>,
}

impl<'a, Id: Identifier, T> Iterator for Iter<'a, Id, T> {
    type Item = Identified<Id, &'a T>;

    fn next(&mut self) -> Option<Self::Item> {
        let (id, value) = self.inner.next()?;
        Some(Identified::new(*id, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<Id: Identifier, T> ExactSizeIterator for Iter<'_, Id, T> {}

/// Mutable dense-order iterator.
pub struct IterMut<'a, Id: Identifier, T> {
    inner: Zip<slice::Iter<'a, Id>, slice::IterMut<'a, T>>,
}

impl<'a, Id: Identifier, T> Iterator for IterMut<'a, Id, T> {
    type Item = Identified<Id, &'a mut T>;

    fn next(&mut self) -> Option<Self::Item> {
        let (id, value) = self.inner.next()?;
        Some(Identified::new(*id, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<Id: Identifier, T> ExactSizeIterator for IterMut<'_, Id, T> {}

/// Consuming dense-order iterator.
pub struct IntoIter<Id: Identifier, T> {
    inner: Zip<vec::IntoIter<Id>, vec::IntoIter<T>>,
}

impl<Id: Identifier, T> Iterator for IntoIter<Id, T> {
    type Item = Identified<Id, T>;

    fn next(&mut self) -> Option<Self::Item> {
        let (id, value) = self.inner.next()?;
        Some(Identified::new(id, value))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<Id: Identifier, T> ExactSizeIterator for IntoIter<Id, T> {}

impl<'a, Id: Identifier, T> IntoIterator for &'a StableArena<Id, T> {
    type Item = Identified<Id, &'a T>;
    type IntoIter = Iter<'a, Id, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, Id: Identifier, T> IntoIterator for &'a mut StableArena<Id, T> {
    type Item = Identified<Id, &'a mut T>;
    type IntoIter = IterMut<'a, Id, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<Id: Identifier, T> IntoIterator for StableArena<Id, T> {
    type Item = Identified<Id, T>;
    type IntoIter = IntoIter<Id, T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            inner: self.slot_ids.into_iter().zip(self.values),
        }
    }
}

impl<Id: Identifier, T> FromIterator<T> for StableArena<Id, T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut arena = Self::default();
        for value in iter {
            arena.push(value);
        }
        arena
    }
}

#[derive(Serialize)]
struct StableArenaWireRef<'a, T> {
    next_id: usize,
    entries: Vec<(usize, &'a T)>,
}

#[derive(Serialize, Deserialize)]
struct StableArenaWire<T> {
    next_id: usize,
    entries: Vec<(usize, T)>,
}

impl<Id: Identifier, T: Serialize> Serialize for StableArena<Id, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        StableArenaWireRef {
            next_id: self.issued_len(),
            entries: self
                .slot_ids
                .iter()
                .copied()
                .map(Into::<usize>::into)
                .zip(self.values.iter())
                .collect(),
        }
        .serialize(serializer)
    }
}

impl<'de, Id: Identifier, T: Deserialize<'de>> Deserialize<'de> for StableArena<Id, T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let StableArenaWire { next_id, entries } = StableArenaWire::deserialize(deserializer)?;
        if next_id == DEAD {
            return Err(serde::de::Error::custom(
                "StableArena next ID exhausts the location sentinel",
            ));
        }
        if let Some(last_issued) = next_id.checked_sub(1)
            && Into::<usize>::into(Id::from(last_issued)) != last_issued
        {
            return Err(serde::de::Error::custom(format!(
                "StableArena next ID {next_id} exceeds its backing type"
            )));
        }

        let mut arena = Self {
            values: Vec::with_capacity(entries.len()),
            slot_ids: Vec::with_capacity(entries.len()),
            locations: vec![DEAD; next_id],
        };
        for (raw, value) in entries {
            if raw >= next_id {
                return Err(serde::de::Error::custom(format!(
                    "StableArena live ID {raw} is outside next ID {next_id}"
                )));
            }
            let id = Id::from(raw);
            if Into::<usize>::into(id) != raw {
                return Err(serde::de::Error::custom(format!(
                    "StableArena ID {raw} does not fit its backing type"
                )));
            }
            if arena.locations[raw] != DEAD {
                return Err(serde::de::Error::custom(format!(
                    "StableArena contains duplicate live ID {raw}"
                )));
            }
            let slot = arena.values.len();
            arena.locations[raw] = slot;
            arena.slot_ids.push(id);
            arena.values.push(value);
        }
        Ok(arena)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jstd_derive::Identifier;

    #[derive(Identifier)]
    struct Id(u32);

    #[test]
    fn removal_repairs_moved_slot_and_never_reuses_ids() {
        let mut arena = StableArena::<Id, &str>::default();
        let a = arena.push("a");
        let b = arena.push("b");
        let c = arena.push("c");

        assert_eq!(arena.remove(b), "b");
        assert!(!arena.contains(b));
        assert_eq!(arena.get(b).map(|entry| **entry), None);
        assert_eq!(arena[c], "c");
        assert_eq!(arena[a], "a");
        assert_eq!(
            arena.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            [a, c]
        );

        let d = arena.push("d");
        assert_eq!(usize::from(d), 3);
        assert_eq!(arena.issued_len(), 4);
        assert_eq!(arena.len(), 3);
    }

    #[test]
    fn removes_first_last_and_only_entries() {
        let mut arena = StableArena::<Id, i32>::default();
        let a = arena.push(1);
        let b = arena.push(2);
        let c = arena.push(3);
        assert_eq!(arena.remove(a), 1);
        assert_eq!(arena.remove(c), 3);
        assert_eq!(arena.remove(b), 2);
        assert!(arena.is_empty());
        assert_eq!(arena.issued_len(), 3);
    }

    #[test]
    #[should_panic(expected = "dead or unknown id")]
    fn indexing_removed_id_panics() {
        let mut arena = StableArena::<Id, i32>::default();
        let id = arena.push(1);
        arena.remove(id);
        let _ = arena[id];
    }

    #[test]
    #[should_panic(expected = "dead or unknown id")]
    fn double_remove_panics() {
        let mut arena = StableArena::<Id, i32>::default();
        let id = arena.push(1);
        arena.remove(id);
        arena.remove(id);
    }

    #[test]
    fn mutable_and_consuming_iteration_keep_stable_ids() {
        let mut arena = StableArena::<Id, i32>::default();
        let a = arena.push(1);
        let b = arena.push(2);
        let c = arena.push(3);
        arena.remove(b);
        for mut entry in &mut arena {
            **entry += 10;
        }
        let entries = arena
            .into_iter()
            .map(|entry| (entry.id, entry.inner))
            .collect::<Vec<_>>();
        assert_eq!(entries, [(a, 11), (c, 13)]);
    }

    #[test]
    fn shrink_to_fit_preserves_ids_order_and_liveness() {
        let mut arena = StableArena::<Id, u64>::default();
        let ids = (0..1_000).map(|step| arena.push(step)).collect::<Vec<_>>();
        for id in &ids[..900] {
            arena.remove(*id);
        }
        let before = arena.clone();
        arena.shrink_to_fit();
        assert_eq!(arena, before);
        assert!(arena.capacity() < 1_000);
        assert_eq!(arena.len(), 100);
        assert_eq!(arena.issued_len(), 1_000);
        assert_eq!(arena[ids[950]], 950);
        let next = arena.push(1_000);
        assert_eq!(usize::from(next), 1_000);
    }

    #[test]
    fn clone_and_equality_preserve_holes_and_physical_order() {
        let mut arena = StableArena::<Id, i32>::default();
        arena.push(1);
        let removed = arena.push(2);
        arena.push(3);
        arena.remove(removed);
        let clone = arena.clone();
        assert_eq!(clone, arena);
        assert_eq!(
            clone.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            arena.iter().map(|entry| entry.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn serde_round_trip_preserves_holes_order_and_cursor() {
        let mut arena = StableArena::<Id, String>::default();
        let a = arena.push("a".into());
        let b = arena.push("b".into());
        let c = arena.push("c".into());
        arena.remove(a);
        arena.remove(b);
        let bytes = bincode::serde::encode_to_vec(&arena, bincode::config::standard()).unwrap();
        let (mut decoded, used): (StableArena<Id, String>, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert_eq!(used, bytes.len());
        assert_eq!(decoded, arena);
        assert_eq!(decoded[c], "c");
        let d = decoded.push("d".into());
        assert_eq!(usize::from(d), 3);

        decoded.remove(c);
        decoded.remove(d);
        let bytes = bincode::serde::encode_to_vec(&decoded, bincode::config::standard()).unwrap();
        let (empty, _): (StableArena<Id, String>, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();
        assert!(empty.is_empty());
        assert_eq!(empty.issued_len(), 4);
    }

    #[test]
    fn malformed_wire_is_rejected() {
        let duplicate = StableArenaWire {
            next_id: 2,
            entries: vec![(0, 1_i32), (0, 2)],
        };
        let bytes = bincode::serde::encode_to_vec(duplicate, bincode::config::standard()).unwrap();
        assert!(
            bincode::serde::decode_from_slice::<StableArena<Id, i32>, _>(
                &bytes,
                bincode::config::standard()
            )
            .is_err()
        );

        let outside = StableArenaWire {
            next_id: 1,
            entries: vec![(1, 1_i32)],
        };
        let bytes = bincode::serde::encode_to_vec(outside, bincode::config::standard()).unwrap();
        assert!(
            bincode::serde::decode_from_slice::<StableArena<Id, i32>, _>(
                &bytes,
                bincode::config::standard()
            )
            .is_err()
        );

        let too_wide = StableArenaWire {
            next_id: u32::MAX as usize + 2,
            entries: Vec::<(usize, i32)>::new(),
        };
        let bytes = bincode::serde::encode_to_vec(too_wide, bincode::config::standard()).unwrap();
        assert!(
            bincode::serde::decode_from_slice::<StableArena<Id, i32>, _>(
                &bytes,
                bincode::config::standard()
            )
            .is_err()
        );
    }

    #[test]
    fn deterministic_mixed_operations_match_option_vec_model() {
        let mut arena = StableArena::<Id, u64>::default();
        let mut model: Vec<Option<u64>> = Vec::new();
        let mut state = 0x1234_5678_9abc_def0_u64;

        for step in 0..2_000_u64 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let live = model
                .iter()
                .enumerate()
                .filter_map(|(id, value)| value.as_ref().map(|_| id))
                .collect::<Vec<_>>();
            if live.is_empty() || state & 3 != 0 {
                let id = arena.push(step);
                assert_eq!(usize::from(id), model.len());
                model.push(Some(step));
            } else {
                let raw = live[state as usize % live.len()];
                let expected = model[raw].take().unwrap();
                assert_eq!(arena.remove(Id::from(raw)), expected);
            }

            for (raw, expected) in model.iter().enumerate() {
                assert_eq!(
                    arena.get(Id::from(raw)).map(|entry| **entry),
                    *expected,
                    "model mismatch at step {step}, id {raw}"
                );
            }
        }
    }

    #[test]
    fn public_views_iteration_and_capacity_accounting_work() {
        let mut arena = StableArena::<Id, i32>::default();
        assert!(arena.is_empty());
        assert_eq!(arena.structural_bytes(), 0);

        let first = arena.push(1);
        let second = arena.push(2);
        let capacity_before = arena.capacity();
        assert!(arena.structural_bytes() >= capacity_before * std::mem::size_of::<i32>());
        assert_eq!(arena.get(first).unwrap().id, first);
        **arena.get_mut(second).unwrap() = 20;
        arena[first] = 10;
        assert_eq!(arena[first], 10);
        assert_eq!(arena[second], 20);

        let mut entries = arena.iter();
        assert_eq!(entries.size_hint(), (2, Some(2)));
        assert_eq!(entries.next().unwrap().id, first);
        assert_eq!(entries.next().unwrap().id, second);
        assert!(entries.next().is_none());
        assert!(format!("{arena:?}").contains("issued"));

        let rebuilt: StableArena<Id, i32> = [3, 4].into_iter().collect();
        assert_eq!(
            rebuilt.iter().map(|entry| *entry.inner).collect::<Vec<_>>(),
            [3, 4]
        );
    }
}
