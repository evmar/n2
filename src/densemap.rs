//! A map of dense integer key to value.

use std::marker::PhantomData;

pub trait Index: From<usize> {
    fn index(&self) -> usize;
}

/// A map of a dense integer key to value, implemented as a vector.
/// Effectively wraps Vec<V> to provided typed keys.
pub struct DenseMap<K, V> {
    vec: Vec<V>,
    key_type: std::marker::PhantomData<K>,
}

impl<K, V> Default for DenseMap<K, V> {
    fn default() -> Self {
        DenseMap {
            vec: Vec::default(),
            key_type: PhantomData,
        }
    }
}

impl<K: Index, V> std::ops::Index<K> for DenseMap<K, V> {
    type Output = V;

    fn index(&self, k: K) -> &Self::Output {
        &self.vec[k.index()]
    }
}

impl<K: Index, V> std::ops::IndexMut<K> for DenseMap<K, V> {
    fn index_mut(&mut self, k: K) -> &mut Self::Output {
        &mut self.vec[k.index()]
    }
}

impl<K: Index, V> DenseMap<K, V> {
    pub fn lookup(&self, k: K) -> Option<&V> {
        self.vec.get(k.index())
    }

    pub fn next_id(&self) -> K {
        K::from(self.vec.len())
    }

    pub fn push(&mut self, val: V) -> K {
        let id = self.next_id();
        self.vec.push(val);
        id
    }

    pub fn all_ids(&self) -> impl Iterator<Item = K> {
        (0..self.vec.len()).map(|id| K::from(id))
    }
}

impl<K: Index, V: Clone> DenseMap<K, V> {
    pub fn new_sized(n: K, default: V) -> Self {
        let mut m = Self::default();
        m.vec.resize(n.index(), default);
        m
    }

    pub fn set_grow(&mut self, k: K, v: V, default: V) {
        if k.index() >= self.vec.len() {
            self.vec.resize(k.index() + 1, default);
        }
        self.vec[k.index()] = v
    }
}
