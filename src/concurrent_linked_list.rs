use std::{
    borrow::Borrow,
    fmt::Debug,
    marker::PhantomData,
    ptr::null_mut,
    sync::atomic::{AtomicPtr, Ordering},
};

/// ConcurrentLinkedList is a linked list that can only be prepended to or
/// iterated over. prepend() accepts an &self instead of an &mut self, and can
/// be called from multiple threads at the same time.
pub struct ConcurrentLinkedList<T> {
    head: AtomicPtr<ConcurrentLinkedListNode<T>>,
}

struct ConcurrentLinkedListNode<T> {
    val: T,
    next: *mut ConcurrentLinkedListNode<T>,
}

impl<T> ConcurrentLinkedList<T> {
    pub fn new() -> Self {
        ConcurrentLinkedList {
            head: AtomicPtr::new(null_mut()),
        }
    }

    pub fn prepend(&self, val: T) {
        let new_head = Box::into_raw(Box::new(ConcurrentLinkedListNode {
            val,
            next: null_mut(),
        }));
        loop {
            let old_head = self.head.load(Ordering::SeqCst);
            unsafe {
                (*new_head).next = old_head;
                if self
                    .head
                    .compare_exchange_weak(old_head, new_head, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    break;
                }
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        ConcurrentLinkedListIterator {
            cur: self.head.load(Ordering::Relaxed),
            lifetime: PhantomData,
        }
    }
}

impl<T> Default for ConcurrentLinkedList<T> {
    fn default() -> Self {
        Self {
            head: Default::default(),
        }
    }
}

impl<T: Clone> Clone for ConcurrentLinkedList<T> {
    fn clone(&self) -> Self {
        let mut iter = self.iter();
        match iter.next() {
            None => Self {
                head: AtomicPtr::new(null_mut()),
            },
            Some(x) => {
                let new_head = Box::into_raw(Box::new(ConcurrentLinkedListNode {
                    val: x.clone(),
                    next: null_mut(),
                }));
                let mut new_tail = new_head;
                for x in iter {
                    unsafe {
                        (*new_tail).next = Box::into_raw(Box::new(ConcurrentLinkedListNode {
                            val: x.clone(),
                            next: null_mut(),
                        }));
                        new_tail = (*new_tail).next;
                    }
                }
                Self {
                    head: AtomicPtr::new(new_head),
                }
            }
        }
    }
}

impl<T: Debug> Debug for ConcurrentLinkedList<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Slow, but hopefully Debug is only used for actual debugging
        f.write_fmt(format_args!("{:?}", self.iter().collect::<Vec<_>>()))
    }
}

impl<T> Drop for ConcurrentLinkedList<T> {
    fn drop(&mut self) {
        let mut cur = self.head.swap(null_mut(), Ordering::Relaxed);
        while !cur.is_null() {
            unsafe {
                // Re-box it so that box will call Drop and deallocate the memory
                let boxed = Box::from_raw(cur);
                cur = boxed.next;
            }
        }
    }
}

struct ConcurrentLinkedListIterator<'a, T> {
    cur: *const ConcurrentLinkedListNode<T>,
    lifetime: PhantomData<&'a ()>,
}

impl<'a, T: 'a> Iterator for ConcurrentLinkedListIterator<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cur.is_null() {
            None
        } else {
            unsafe {
                let result = Some((*self.cur).val.borrow());
                self.cur = (*self.cur).next;
                result
            }
        }
    }
}
