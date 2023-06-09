//! A map of dense integer key to value.

use std::marker::PhantomData;

pub trait Index: From<usize> {
    fn index(&self) -> usize;
}

/// A map of a dense integer key to value, implemented as a vector.
/// Effectively wraps Vec<V> to provided typed keys.
#[derive(Default)]
pub struct DenseMap<K, V> {
    vec: Vec<V>,
    key_type: std::marker::PhantomData<K>,
}

impl<K: Index, V> DenseMap<K, V> {
    pub fn new() -> Self {
        DenseMap {
            vec: Vec::new(),
            key_type: PhantomData::default(),
        }
    }

    pub fn get(&self, k: K) -> &V {
        &self.vec[k.index()]
    }

    pub fn get_mut(&mut self, k: K) -> &mut V {
        &mut self.vec[k.index()]
    }

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
}

impl<K: Index, V: Clone> DenseMap<K, V> {
    pub fn new_sized(n: K, default: V) -> Self {
        let mut m = Self::new();
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
