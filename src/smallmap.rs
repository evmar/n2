//! A map-like object for maps with few entries.
//! TODO: this may not be needed at all, but the code used this pattern in a
//! few places so I figured I may as well name it.

use std::borrow::Borrow;

/// A map-like object implemented as a list of pairs, for cases where the
/// number of entries in the map is small.
pub struct SmallMap<K, V>(Vec<(K, V)>);
impl<K: PartialEq, V> SmallMap<K, V> {
    pub fn new() -> Self {
        SmallMap(Vec::new())
    }

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
        return None;
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
}
