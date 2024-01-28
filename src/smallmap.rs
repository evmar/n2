//! A map-like object for maps with few entries.
//! TODO: this may not be needed at all, but the code used this pattern in a
//! few places so I figured I may as well name it.

use std::{borrow::Borrow, fmt::Debug};

/// A map-like object implemented as a list of pairs, for cases where the
/// number of entries in the map is small.
#[derive(Debug)]
pub struct SmallMap<K, V>(Vec<(K, V)>);

impl<K, V> SmallMap<K, V> {
    pub fn with_capacity(cap: usize) -> Self {
        Self(Vec::with_capacity(cap))
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl<K, V> Default for SmallMap<K, V> {
    fn default() -> Self {
        SmallMap(Vec::default())
    }
}

impl<K: PartialEq, V> SmallMap<K, V> {
    pub fn insert(&mut self, k: K, v: V) {
        for (ik, iv) in self.0.iter_mut() {
            if *ik == k {
                *iv = v;
                return;
            }
        }
        self.0.push((k, v));
    }

    pub fn get<Q>(&self, q: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: PartialEq + ?Sized,
    {
        for (k, v) in self.0.iter() {
            if k.borrow() == q {
                return Some(v);
            }
        }
        None
    }

    pub fn iter(&self) -> std::slice::Iter<(K, V)> {
        self.0.iter()
    }

    pub fn iter_mut(&mut self) -> std::slice::IterMut<(K, V)> {
        self.0.iter_mut()
    }

    pub fn into_iter(self) -> std::vec::IntoIter<(K, V)> {
        self.0.into_iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &V> + '_ {
        self.0.iter().map(|x| &x.1)
    }
}

impl<K: PartialEq, V, const N: usize> std::convert::From<[(K, V); N]> for SmallMap<K, V> {
    fn from(value: [(K, V); N]) -> Self {
        let mut result = SmallMap::default();
        for (k, v) in value {
            result.insert(k, v);
        }
        result
    }
}

impl<K: Debug, V: Debug> Debug for SmallMap<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

// Only for tests because it is order-sensitive
#[cfg(test)]
impl<K: PartialEq, V: PartialEq> PartialEq for SmallMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        return self.0 == other.0;
    }
}

// TODO: Make this not order-sensitive
impl<K: PartialEq, V: PartialEq> PartialEq for SmallMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
