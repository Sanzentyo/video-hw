use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueSendError {
    Full,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueRecvError {
    Empty,
    Disconnected,
    Timeout,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct QueueStats {
    pub depth: usize,
    pub peak_depth: usize,
}

#[derive(Debug)]
struct QueueCounters {
    depth: AtomicUsize,
    peak_depth: AtomicUsize,
}

impl QueueCounters {
    fn new() -> Self {
        Self {
            depth: AtomicUsize::new(0),
            peak_depth: AtomicUsize::new(0),
        }
    }

    fn on_send(&self) {
        let depth = self.depth.fetch_add(1, Ordering::Relaxed) + 1;
        let mut peak = self.peak_depth.load(Ordering::Relaxed);
        while depth > peak {
            match self.peak_depth.compare_exchange_weak(
                peak,
                depth,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => peak = current,
            }
        }
    }

    fn on_recv(&self) {
        let current = self.depth.load(Ordering::Relaxed);
        if current > 0 {
            let _ = self.depth.fetch_sub(1, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> QueueStats {
        QueueStats {
            depth: self.depth.load(Ordering::Relaxed),
            peak_depth: self.peak_depth.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug)]
pub struct BoundedQueueTx<T> {
    inner: SyncSender<T>,
    counters: Arc<QueueCounters>,
}

impl<T> Clone for BoundedQueueTx<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            counters: Arc::clone(&self.counters),
        }
    }
}

#[derive(Debug)]
pub struct BoundedQueueRx<T> {
    inner: Receiver<T>,
    counters: Arc<QueueCounters>,
}

impl<T> BoundedQueueTx<T> {
    pub fn send(&self, value: T) -> Result<(), QueueSendError> {
        self.inner
            .send(value)
            .map_err(|_| QueueSendError::Disconnected)?;
        self.counters.on_send();
        Ok(())
    }

    pub fn try_send(&self, value: T) -> Result<(), QueueSendError> {
        match self.inner.try_send(value) {
            Ok(()) => {
                self.counters.on_send();
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(QueueSendError::Full),
            Err(TrySendError::Disconnected(_)) => Err(QueueSendError::Disconnected),
        }
    }

    pub fn stats(&self) -> QueueStats {
        self.counters.snapshot()
    }
}

impl<T> BoundedQueueRx<T> {
    pub fn recv(&self) -> Result<T, QueueRecvError> {
        match self.inner.recv() {
            Ok(item) => {
                self.counters.on_recv();
                Ok(item)
            }
            Err(_) => Err(QueueRecvError::Disconnected),
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, QueueRecvError> {
        match self.inner.recv_timeout(timeout) {
            Ok(item) => {
                self.counters.on_recv();
                Ok(item)
            }
            Err(RecvTimeoutError::Timeout) => Err(QueueRecvError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(QueueRecvError::Disconnected),
        }
    }

    pub fn try_recv(&self) -> Result<T, QueueRecvError> {
        match self.inner.try_recv() {
            Ok(item) => {
                self.counters.on_recv();
                Ok(item)
            }
            Err(TryRecvError::Empty) => Err(QueueRecvError::Empty),
            Err(TryRecvError::Disconnected) => Err(QueueRecvError::Disconnected),
        }
    }

    pub fn stats(&self) -> QueueStats {
        self.counters.snapshot()
    }
}

pub fn bounded_queue<T>(capacity: usize) -> (BoundedQueueTx<T>, BoundedQueueRx<T>) {
    let (tx, rx) = mpsc::sync_channel(capacity.max(1));
    let counters = Arc::new(QueueCounters::new());
    (
        BoundedQueueTx {
            inner: tx,
            counters: Arc::clone(&counters),
        },
        BoundedQueueRx {
            inner: rx,
            counters,
        },
    )
}

#[derive(Debug)]
pub struct InFlightCredits {
    capacity: usize,
    used: AtomicUsize,
}

impl InFlightCredits {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            used: AtomicUsize::new(0),
        }
    }

    pub fn try_acquire(&self) -> bool {
        loop {
            let used = self.used.load(Ordering::Relaxed);
            if used >= self.capacity {
                return false;
            }
            if self
                .used
                .compare_exchange_weak(used, used + 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub fn release(&self) {
        loop {
            let used = self.used.load(Ordering::Relaxed);
            if used == 0 {
                return;
            }
            if self
                .used
                .compare_exchange_weak(used, used - 1, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    pub fn snapshot(&self) -> (usize, usize) {
        (self.used.load(Ordering::Relaxed), self.capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_stats_track_depth_and_peak() {
        let (tx, rx) = bounded_queue::<usize>(2);
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        let stats = tx.stats();
        assert_eq!(stats.depth, 2);
        assert_eq!(stats.peak_depth, 2);

        assert_eq!(rx.recv().unwrap(), 1);
        let stats_after = rx.stats();
        assert_eq!(stats_after.depth, 1);
        assert_eq!(stats_after.peak_depth, 2);
    }

    #[test]
    fn inflight_credits_work() {
        let credits = InFlightCredits::new(2);
        assert!(credits.try_acquire());
        assert!(credits.try_acquire());
        assert!(!credits.try_acquire());
        let (used, cap) = credits.snapshot();
        assert_eq!(used, 2);
        assert_eq!(cap, 2);
        credits.release();
        assert!(credits.try_acquire());
    }
}
