//! Strongly typed identifiers and registries.
//!
//! This module provides:
//! - `Identifier`: a trait for strongly typed IDs backed by `usize`
//! - `Identified<Id, T>`: a value paired with its typed ID
//! - `Registry<Id, T>`: a typed vector indexed by `Id`
//!
//! # Example
//! ```
//! use jstd::Identifier;
//! use jstd::registry::Registry;
//!
//! #[derive(Identifier)]
//! struct NodeId(usize);
//!
//! let mut registry = Registry::<NodeId, &str>::default();
//! let a = registry.push("a");
//! let b = registry.push("b");
//!
//! assert_eq!(usize::from(a), 0);
//! assert_eq!(usize::from(b), 1);
//! assert_eq!(registry[a], "a");
//! assert_eq!(registry[b], "b");
//! ```
use std::{
    fmt::{Debug, Display},
    hash::Hash,
    marker::PhantomData,
    ops::{Deref, DerefMut, Index, IndexMut},
    slice, vec,
};

use crate::intern::Intern;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeSeq};

/// Typed identifier trait used by [`Registry`].
///
/// Any `Identifier` is expected to be a light wrapper over `usize`.
pub trait Identifier: Copy + Hash + From<usize> + Into<usize> + Eq + Ord + std::fmt::Debug {}

impl Identifier for usize {}

/// A value paired with its typed identifier.
pub struct Identified<Id: Identifier, T> {
    pub id: Id,
    pub inner: T,
}

impl<Id: Identifier, T> Identified<Id, T> {
    /// Creates a new identified wrapper.
    pub fn new(id: Id, data: T) -> Self {
        Self { id, inner: data }
    }
}

impl<Id: Identifier, T> Deref for Identified<Id, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<Id: Identifier, T> DerefMut for Identified<Id, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<'a, Id: Identifier, T> Identified<Id, &'a mut T> {
    /// Converts `Identified<Id, &mut T>` into `Identified<Id, &T>`.
    pub fn immutable(self) -> Identified<Id, &'a T> {
        Identified::new(self.id, &*self.inner)
    }
}

impl<Id: Identifier, T: Display> Display for Identified<Id, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl<Id: Identifier, T: Debug> Debug for Identified<Id, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

/// A strongly typed vector
///
/// # Example
/// ```
/// use jstd::Identifier;
/// use jstd::registry::Registry;
///
/// #[derive(Identifier)]
/// struct ItemId(usize);
///
/// let mut reg = Registry::<ItemId, i32>::default();
/// let id = reg.push(10);
/// reg[id] += 5;
///
/// assert_eq!(reg[id], 15);
/// assert_eq!(reg.len(), 1);
/// assert!(!reg.is_empty());
/// ```
/// Segmented, **append-only-stable** backing store: element `n` lives in chunk
/// `k = floor(log2(n + 1))` at offset `n + 1 - 2^k`, so chunk `k` holds exactly
/// `2^k` elements. Each chunk is allocated once at its full capacity and never
/// reallocated, so **an element's address is stable for the life of the
/// registry** even as later `push`es grow the store (growing appends new chunks;
/// it never moves existing elements). Growing the outer `Vec<Vec<T>>` moves the
/// chunk *headers*, not their heap buffers. This stability is what lets the
/// literal/type interners hand out `&T` references that outlive a mint (see
/// `qcode`'s `RwLock`-wrapped interners). Chunk sizes double, so a small registry
/// stays cheap (chunks 1, 2, 4, …) and a large one needs few chunks.
///
/// The public API is identical to a flat `Vec`-backed registry: ids are dense
/// `0..len` and index in insertion order.
pub struct Registry<Id: Identifier, T> {
    chunks: Vec<Vec<T>>,
    len: usize,
    _marker: PhantomData<Id>,
}

/// `(chunk, offset)` for global index `n`.
#[inline]
fn locate(n: usize) -> (usize, usize) {
    let m = n + 1;
    let k = (usize::BITS - 1 - m.leading_zeros()) as usize;
    (k, m - (1 << k))
}

impl<Id: Identifier, T: Serialize> Serialize for Registry<Id, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Same wire format as the old flat registry: a single length-prefixed
        // sequence of elements in id order. The length must be given up front —
        // a `collect_seq` over the chunk-`Flatten` iterator has no exact length,
        // which length-prefixed formats (bincode) reject.
        let mut seq = serializer.serialize_seq(Some(self.len))?;
        for e in self.chunks.iter().flatten() {
            seq.serialize_element(e)?;
        }
        seq.end()
    }
}

impl<'de, Id: Identifier, T: Deserialize<'de>> Deserialize<'de> for Registry<Id, T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Vec::<T>::deserialize(deserializer)?.into_iter().collect())
    }
}

impl<Id: Identifier, T: Clone> Clone for Registry<Id, T> {
    fn clone(&self) -> Self {
        // Rebuild through `push` so each chunk is reallocated at its full
        // capacity (a derived `Vec` clone would shrink the last chunk to its
        // length and break the never-realloc stability invariant on next push).
        self.chunks.iter().flatten().cloned().collect()
    }
}

impl<Id: Identifier, T: Debug> Debug for Registry<Id, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.chunks.iter().flatten())
            .finish()
    }
}

impl<Id: Identifier, T: PartialEq> PartialEq for Registry<Id, T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self
                .chunks
                .iter()
                .flatten()
                .eq(other.chunks.iter().flatten())
    }
}

impl<Id: Identifier, T: Eq> Eq for Registry<Id, T> {}

impl<Id: Identifier, T> Registry<Id, T> {
    /// Pushes a value and returns its typed identifier.
    pub fn push(&mut self, e: T) -> Id {
        let n = self.len;
        let (k, offset) = locate(n);
        if offset == 0 {
            // First element of a fresh chunk `k`; earlier chunks are already full.
            self.chunks.push(Vec::with_capacity(1 << k));
        }
        self.chunks[k].push(e);
        self.len += 1;
        n.into()
    }

    /// Returns the number of elements.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if the registry contains no elements.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    #[track_caller]
    fn at(&self, n: usize) -> &T {
        debug_assert!(
            n < self.len,
            "registry index {n} out of bounds for length {}",
            self.len
        );
        let (k, offset) = locate(n);
        &self.chunks[k][offset]
    }

    #[inline]
    #[track_caller]
    fn at_mut(&mut self, n: usize) -> &mut T {
        debug_assert!(
            n < self.len,
            "registry index {n} out of bounds for length {}",
            self.len
        );
        let (k, offset) = locate(n);
        &mut self.chunks[k][offset]
    }

    /// Replaces the element at `id`, returning the previous value. The id (and
    /// every element's stable address) is unchanged. Used to *check out* an
    /// element — swap in a sentinel, own the original, swap it back later —
    /// without disturbing any other id.
    ///
    /// # Panics
    /// Panics if `id` is out of bounds.
    #[track_caller]
    pub fn replace(&mut self, id: Id, value: T) -> T {
        std::mem::replace(self.at_mut(id.into()), value)
    }

    /// Returns an immutable identified view of an element.
    ///
    /// # Panics
    /// Panics if `id` is out of bounds.
    #[track_caller]
    pub fn get(&self, id: Id) -> Identified<Id, &T> {
        Identified::new(id, self.at(id.into()))
    }

    /// Returns a mutable identified view of an element.
    ///
    /// # Panics
    /// Panics if `id` is out of bounds.
    #[track_caller]
    pub fn get_mut(&mut self, id: Id) -> Identified<Id, &mut T> {
        Identified::new(id, self.at_mut(id.into()))
    }

    /// Iterates immutably over `(id, value)` as [`Identified`] items.
    ///
    /// # Example
    /// ```
    /// use jstd::Identifier;
    /// use jstd::registry::Registry;
    ///
    /// #[derive(Identifier)]
    /// struct Id(usize);
    ///
    /// let mut reg = Registry::<Id, &str>::default();
    /// reg.push("x");
    /// reg.push("y");
    ///
    /// let ids: Vec<usize> = reg.iter().map(|item| usize::from(item.id)).collect();
    /// let vals: Vec<&str> = reg.iter().map(|item| **item).collect();
    ///
    /// assert_eq!(ids, vec![0, 1]);
    /// assert_eq!(vals, vec!["x", "y"]);
    /// ```
    pub fn iter(&self) -> Iter<'_, Id, T> {
        Iter {
            iter: self.chunks.iter().flatten(),
            index: 0,
            _marker: PhantomData,
        }
    }

    /// Iterates mutably over `(id, value)` as [`Identified`] items.
    ///
    /// # Example
    /// ```
    /// use jstd::Identifier;
    /// use jstd::registry::Registry;
    ///
    /// #[derive(Identifier)]
    /// struct Id(usize);
    ///
    /// let mut reg = Registry::<Id, i32>::default();
    /// reg.push(1);
    /// reg.push(2);
    ///
    /// for mut item in reg.iter_mut() {
    ///     **item += 10;
    /// }
    ///
    /// assert_eq!(reg[Id::from(0)], 11);
    /// assert_eq!(reg[Id::from(1)], 12);
    /// ```
    pub fn iter_mut(&mut self) -> IterMut<'_, Id, T> {
        IterMut {
            iter: self.chunks.iter_mut().flatten(),
            index: 0,
            _marker: PhantomData,
        }
    }

    /// Borrows the elements at `ids` mutably and disjointly, returned in the same
    /// order as `ids`.
    ///
    /// This is the disjoint-`&mut`-slice primitive the parallel function-pass
    /// driver uses to hand each worker its own body straight out of the registry,
    /// without the checkout/checkin swap (context-split stage 5b-ii, see
    /// `docs/plans/context-split/05b-plan.md` §2.2). Because every returned
    /// reference comes from a *distinct* [`iter_mut`](Self::iter_mut) slot, the
    /// borrows are provably non-overlapping and no `unsafe` is required.
    ///
    /// `ids` must be **distinct** and in bounds; the returned vector has one
    /// reference per requested id, positionally aligned with `ids`.
    ///
    /// # Panics
    /// Panics if `ids` contains a duplicate id or an out-of-bounds id.
    pub fn select_mut(&mut self, ids: &[Id]) -> Vec<&mut T> {
        // Map each requested global index to its position in `ids`, asserting
        // distinctness and bounds up front so a caller bug is a loud panic, never
        // a silently-shortened result.
        let mut want: std::collections::HashMap<usize, usize> =
            std::collections::HashMap::with_capacity(ids.len());
        for (pos, id) in ids.iter().enumerate() {
            let n: usize = (*id).into();
            assert!(
                n < self.len,
                "select_mut: id index {n} out of bounds (len {})",
                self.len
            );
            let prev = want.insert(n, pos);
            assert!(prev.is_none(), "select_mut: duplicate id index {n}");
        }

        // One `iter_mut` pass: each disjoint `&mut T` is routed to its requested
        // output slot. `iter_mut` yields ids in ascending global order; `want`
        // restores the caller's order.
        let mut slots: Vec<Option<&mut T>> = (0..ids.len()).map(|_| None).collect();
        for item in self.iter_mut() {
            let n: usize = item.id.into();
            if let Some(&pos) = want.get(&n) {
                slots[pos] = Some(item.inner);
            }
        }
        slots
            .into_iter()
            .map(|slot| slot.expect("select_mut: requested id had no backing slot"))
            .collect()
    }
}

pub struct Iter<'a, Id: Identifier, T> {
    iter: std::iter::Flatten<slice::Iter<'a, Vec<T>>>,
    index: usize,
    _marker: PhantomData<Id>,
}

impl<'a, Id: Identifier, T> Iterator for Iter<'a, Id, T> {
    type Item = Identified<Id, &'a T>;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.iter.next()?;
        let id = Id::from(self.index);
        self.index += 1;

        Some(Identified::new(id, value))
    }
}

pub struct IterMut<'a, Id: Identifier, T> {
    iter: std::iter::Flatten<slice::IterMut<'a, Vec<T>>>,
    index: usize,
    _marker: PhantomData<Id>,
}

impl<'a, Id: Identifier, T> Iterator for IterMut<'a, Id, T> {
    type Item = Identified<Id, &'a mut T>;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.iter.next()?;
        let id = Id::from(self.index);
        self.index += 1;

        Some(Identified::new(id, value))
    }
}

pub struct IntoIter<Id: Identifier, T> {
    iter: std::iter::Flatten<vec::IntoIter<Vec<T>>>,
    index: usize,
    _marker: PhantomData<Id>,
}

impl<Id: Identifier, T> Iterator for IntoIter<Id, T> {
    type Item = Identified<Id, T>;

    fn next(&mut self) -> Option<Self::Item> {
        let value = self.iter.next()?;
        let id = Id::from(self.index);
        self.index += 1;

        Some(Identified::new(id, value))
    }
}

impl<Id: Identifier, T> Default for Registry<Id, T> {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
            _marker: PhantomData,
        }
    }
}

impl<'a, Id: Identifier, T> IntoIterator for &'a Registry<Id, T> {
    type Item = Identified<Id, &'a T>;

    type IntoIter = Iter<'a, Id, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, Id: Identifier, T> IntoIterator for &'a mut Registry<Id, T> {
    type Item = Identified<Id, &'a mut T>;

    type IntoIter = IterMut<'a, Id, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<Id: Identifier, T> IntoIterator for Registry<Id, T> {
    type Item = Identified<Id, T>;

    type IntoIter = IntoIter<Id, T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.chunks.into_iter().flatten(),
            index: 0,
            _marker: PhantomData,
        }
    }
}

impl<Id: Identifier, T> FromIterator<T> for Registry<Id, T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut reg = Self::default();
        for e in iter {
            reg.push(e);
        }
        reg
    }
}

impl<Id: Identifier, T> Index<Id> for Registry<Id, T> {
    type Output = T;

    #[track_caller]
    fn index(&self, index: Id) -> &Self::Output {
        self.at(index.into())
    }
}

impl<Id: Identifier, T> IndexMut<Id> for Registry<Id, T> {
    #[track_caller]
    fn index_mut(&mut self, index: Id) -> &mut Self::Output {
        self.at_mut(index.into())
    }
}

impl<Id: Identifier, T: Intern> Intern for Registry<Id, T> {
    type Static = Registry<Id, T::Static>;

    fn intern(self, pool: &mut super::intern::StringPool) -> Self::Static {
        self.into_iter()
            .map(|item| item.inner.intern(pool))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jstd_derive::Identifier;

    #[derive(Identifier)]
    struct Id(usize);

    /// Segment math places element `n` in chunk `floor(log2(n+1))`.
    #[test]
    fn locate_matches_doubling_layout() {
        assert_eq!(locate(0), (0, 0)); // chunk 0 (size 1)
        assert_eq!(locate(1), (1, 0)); // chunk 1 (size 2)
        assert_eq!(locate(2), (1, 1));
        assert_eq!(locate(3), (2, 0)); // chunk 2 (size 4)
        assert_eq!(locate(6), (2, 3));
        assert_eq!(locate(7), (3, 0)); // chunk 3 (size 8)
    }

    /// Dense ids, `len`, and index order survive spanning many chunks.
    #[test]
    fn push_index_iter_across_chunks() {
        let mut reg = Registry::<Id, usize>::default();
        let ids: Vec<Id> = (0..1000).map(|v| reg.push(v)).collect();
        assert_eq!(reg.len(), 1000);
        for (i, &id) in ids.iter().enumerate() {
            assert_eq!(usize::from(id), i);
            assert_eq!(reg[id], i);
        }
        let seen: Vec<usize> = reg.iter().map(|item| *item.inner).collect();
        assert_eq!(seen, (0..1000).collect::<Vec<_>>());
    }

    /// The core invariant: an element's address is stable across later pushes
    /// (later pushes only append new chunks; existing chunks never reallocate).
    #[test]
    fn element_address_is_stable_across_pushes() {
        let mut reg = Registry::<Id, usize>::default();
        let first = reg.push(42);
        let addr = &reg[first] as *const usize;
        for v in 0..10_000 {
            reg.push(v);
        }
        assert_eq!(
            &reg[first] as *const usize, addr,
            "address moved after growth"
        );
        assert_eq!(reg[first], 42);
    }

    /// `replace` swaps contents in place, returning the old value and leaving the
    /// slot's address (and all other ids) untouched.
    #[test]
    fn replace_swaps_in_place() {
        let mut reg = Registry::<Id, i32>::default();
        let a = reg.push(1);
        let b = reg.push(2);
        let addr_b = &reg[b] as *const i32;
        let old = reg.replace(b, 99);
        assert_eq!(old, 2);
        assert_eq!(reg[b], 99);
        assert_eq!(reg[a], 1, "other ids untouched");
        assert_eq!(&reg[b] as *const i32, addr_b, "slot address stable");
        assert_eq!(reg.len(), 2, "len unchanged");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "registry index 0 out of bounds for length 0")]
    fn indexing_reports_requested_index_and_length() {
        let reg = Registry::<Id, usize>::default();
        let _ = reg[Id::from(0)];
    }

    /// `select_mut` returns disjoint mutable borrows in the caller's id order
    /// (not ascending id order), spanning multiple chunks, and lets each be
    /// written independently.
    #[test]
    fn select_mut_disjoint_in_input_order() {
        let mut reg = Registry::<Id, usize>::default();
        let ids: Vec<Id> = (0..100).map(|v| reg.push(v)).collect();

        // Deliberately out of order and spanning chunk boundaries.
        let picked = [ids[7], ids[0], ids[63], ids[64], ids[2]];
        let refs = reg.select_mut(&picked);
        assert_eq!(refs.len(), picked.len());
        // Order matches `picked`, not ascending id order.
        assert_eq!(
            refs.iter().map(|r| **r).collect::<Vec<_>>(),
            vec![7, 0, 63, 64, 2]
        );
        // Disjoint: mutate every borrow, then observe all writes landed.
        for r in refs {
            *r += 1000;
        }
        for &id in &picked {
            assert_eq!(reg[id], usize::from(id) + 1000);
        }
        // Untouched ids are unchanged.
        assert_eq!(reg[ids[1]], 1);
    }

    /// A duplicate id in the request is a loud panic (would otherwise alias).
    #[test]
    #[should_panic(expected = "duplicate id")]
    fn select_mut_rejects_duplicates() {
        let mut reg = Registry::<Id, usize>::default();
        let a = reg.push(1);
        reg.push(2);
        let _ = reg.select_mut(&[a, a]);
    }

    /// Rebuilding from a flat element sequence (the path `Deserialize` and
    /// `Clone` take through `FromIterator`) reproduces a chunked registry equal to
    /// the original, spanning several chunks.
    #[test]
    fn rebuild_from_flat_sequence() {
        let reg: Registry<Id, i32> = (0..300).collect();
        let flat: Vec<i32> = reg.iter().map(|item| *item.inner).collect();
        let back: Registry<Id, i32> = flat.into_iter().collect();
        assert_eq!(reg, back);
        assert_eq!(back.len(), 300);
    }

    /// Clone preserves contents and the never-realloc stability invariant: a
    /// clone's last (partial) chunk must still absorb a push without moving its
    /// existing elements.
    #[test]
    fn clone_preserves_stability() {
        let mut reg = Registry::<Id, usize>::default();
        for v in 0..5 {
            reg.push(v); // ends mid-chunk (chunk 2 holds indices 3,4 of capacity 4)
        }
        let mut cloned = reg.clone();
        assert_eq!(reg, cloned);
        let last = Id::from(4);
        let addr = &cloned[last] as *const usize;
        cloned.push(99); // fills the partial chunk without reallocating it
        assert_eq!(
            &cloned[last] as *const usize, addr,
            "clone's chunk reallocated"
        );
    }
}
