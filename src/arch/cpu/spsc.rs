use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Fixed-capacity single-producer/single-consumer ring queue.
///
/// One slot is deliberately left unused so equality of the head and tail
/// indices unambiguously means "empty". Consequently, a queue declared with
/// `N` slots can hold at most `N - 1` values.
///
/// The caller must preserve the SPSC contract: exactly one CPU may call
/// [`Self::try_push`] and exactly one CPU may call [`Self::try_pop`] / inspect
/// the front element.
pub struct SpscQueue<T, const N: usize> {
    slots: [UnsafeCell<MaybeUninit<T>>; N],
    head: AtomicUsize,
    tail: AtomicUsize,
}

unsafe impl<T: Send, const N: usize> Sync for SpscQueue<T, N> {}

impl<T, const N: usize> SpscQueue<T, N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    #[inline(always)]
    const fn next(index: usize) -> usize {
        (index + 1) % N
    }

    pub fn try_push(&self, value: T) -> Result<(), T> {
        debug_assert!(N >= 2, "SPSC queue requires at least two slots");

        let tail = self.tail.load(Ordering::Relaxed);
        let next = Self::next(tail);

        if next == self.head.load(Ordering::Acquire) {
            return Err(value);
        }

        unsafe {
            (*self.slots[tail].get()).write(value);
        }
        self.tail.store(next, Ordering::Release);
        Ok(())
    }

    pub fn try_pop(&self) -> Option<T> {
        debug_assert!(N >= 2, "SPSC queue requires at least two slots");

        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }

        let value = unsafe { (*self.slots[head].get()).assume_init_read() };
        self.head.store(Self::next(head), Ordering::Release);
        Some(value)
    }

    pub fn is_full(&self) -> bool {
        debug_assert!(N >= 2, "SPSC queue requires at least two slots");

        let tail = self.tail.load(Ordering::Relaxed);
        Self::next(tail) == self.head.load(Ordering::Acquire)
    }

    /// Inspect the current front value without consuming it.
    ///
    /// This is safe under the queue's SPSC contract: the producer cannot
    /// overwrite the occupied head slot until the sole consumer advances it.
    pub fn with_front<R>(&self, inspect: impl FnOnce(&T) -> R) -> Option<R> {
        let head = self.head.load(Ordering::Relaxed);
        if head == self.tail.load(Ordering::Acquire) {
            return None;
        }

        let value = unsafe { &*(*self.slots[head].get()).as_ptr() };
        Some(inspect(value))
    }
}

impl<T, const N: usize> Default for SpscQueue<T, N> {
    fn default() -> Self {
        Self::new()
    }
}
