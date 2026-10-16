/// Progress and cancellation hooks for long parses. `unlost-core`'s `Job` implements this; tests
/// use [`NoProgress`].
pub trait Progress: Send + Sync {
    fn stage(&self, _stage: &str) {}
    fn set_total(&self, _total: u64) {}
    fn advance(&self, _n: u64) {}
    fn found(&self, _n: u64) {}
    fn cancelled(&self) -> bool {
        false
    }
}

pub struct NoProgress;

impl Progress for NoProgress {}
