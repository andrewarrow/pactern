//! Tenant-scoped transactional host services with bounded, replayable authority.
//! This adapter is embedded by a trusted Rust application; source effects alone
//! never create grants. The initial store is in-memory and uses optimistic commits.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostError {
    Denied,
    Revoked,
    BudgetExceeded,
    Timeout,
    InvalidAmount,
    InsufficientQuota,
    Conflict,
    MissingTenant,
    ReplayMismatch,
    MissingRecording,
    IncompleteReplay,
    StorageUnavailable,
}
impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "host operation failed: {self:?}")
    }
}
impl std::error::Error for HostError {}
type Result<T> = std::result::Result<T, HostError>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Read,
    Write,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub tenant: String,
    pub operations: BTreeSet<Operation>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub operations: u64,
    pub bytes: u64,
    pub memory_bytes: u64,
    pub timeout_ms: u64,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            operations: 1000,
            bytes: 1_048_576,
            memory_bytes: 1_048_576,
            timeout_ms: 10_000,
        }
    }
}
#[derive(Clone)]
struct Meter {
    remaining: Budget,
    deadline: Instant,
}
impl Meter {
    fn new(b: Budget) -> Self {
        Self {
            remaining: b,
            deadline: Instant::now()
                .checked_add(Duration::from_millis(b.timeout_ms))
                .unwrap_or_else(Instant::now),
        }
    }
}
#[derive(Clone, Debug)]
struct Account {
    balance: i64,
    revision: u64,
}
#[derive(Default)]
pub struct QuotaStore {
    accounts: Mutex<BTreeMap<String, Account>>,
    writes: AtomicU64,
}
impl QuotaStore {
    pub fn new(accounts: impl IntoIterator<Item = (String, i64)>) -> Result<Self> {
        let mut result = BTreeMap::new();
        for (tenant, balance) in accounts {
            if balance < 0 || result.contains_key(&tenant) {
                return Err(HostError::InvalidAmount);
            }
            result.insert(
                tenant,
                Account {
                    balance,
                    revision: 0,
                },
            );
        }
        Ok(Self {
            accounts: Mutex::new(result),
            writes: AtomicU64::new(0),
        })
    }
    pub fn balance(&self, tenant: &str) -> Result<i64> {
        self.accounts
            .lock()
            .map_err(|_| HostError::StorageUnavailable)?
            .get(tenant)
            .map(|a| a.balance)
            .ok_or(HostError::MissingTenant)
    }
    pub fn live_writes(&self) -> u64 {
        self.writes.load(Ordering::SeqCst)
    }
    fn begin(&self, tenant: &str) -> Result<Transaction> {
        let accounts = self
            .accounts
            .lock()
            .map_err(|_| HostError::StorageUnavailable)?;
        let account = accounts.get(tenant).ok_or(HostError::MissingTenant)?;
        Ok(Transaction {
            tenant: tenant.into(),
            balance: account.balance,
            revision: account.revision,
            dirty: false,
        })
    }
    fn commit(&self, txn: &Transaction) -> Result<()> {
        if txn.balance < 0 {
            return Err(HostError::InvalidAmount);
        }
        let mut accounts = self
            .accounts
            .lock()
            .map_err(|_| HostError::StorageUnavailable)?;
        let account = accounts
            .get_mut(&txn.tenant)
            .ok_or(HostError::MissingTenant)?;
        if account.revision != txn.revision {
            return Err(HostError::Conflict);
        }
        if txn.dirty {
            let revision = account
                .revision
                .checked_add(1)
                .ok_or(HostError::StorageUnavailable)?;
            account.balance = txn.balance;
            account.revision = revision;
            self.writes.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
}
struct Transaction {
    tenant: String,
    balance: i64,
    revision: u64,
    dirty: bool,
}
/// An unforgeable owning token. It is intentionally neither Clone nor Copy.
#[derive(Debug)]
pub struct Handle {
    invocation: u64,
    id: u64,
}
#[derive(Clone)]
pub struct Cancellation {
    cancelled: Arc<AtomicBool>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Begin { tenant: String },
    Read { handle: u64 },
    Reserve { handle: u64, amount: String },
    Commit { handle: u64 },
    Rollback { handle: u64 },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Response {
    Handle(u64),
    Balance(String),
    Reserved(bool),
    Unit,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceEntry {
    pub request: Request,
    pub response: Option<std::result::Result<Response, HostError>>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trace {
    pub version: String,
    pub source_revision: String,
    pub policy_hash: String,
    pub entries: Vec<TraceEntry>,
}
enum Mode {
    Record,
    Replay { cursor: usize },
}
/// Authority and transactions end together. A trap drops this invocation, so
/// uncommitted writes cannot escape; explicit earlier commits remain committed.
pub struct Invocation {
    id: u64,
    store: Arc<QuotaStore>,
    grants: Vec<Grant>,
    meters: Vec<Meter>,
    cancelled: Arc<AtomicBool>,
    handles: BTreeMap<u64, Transaction>,
    next_handle: u64,
    mode: Mode,
    trace: Trace,
}
impl Invocation {
    pub fn record(
        store: Arc<QuotaStore>,
        grants: Vec<Grant>,
        budget: Budget,
        revision: impl Into<String>,
        policy_hash: impl Into<String>,
    ) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            store,
            grants,
            meters: vec![Meter::new(budget)],
            cancelled: Arc::new(AtomicBool::new(false)),
            handles: BTreeMap::new(),
            next_handle: 0,
            mode: Mode::Record,
            trace: Trace {
                version: "0.1.0".into(),
                source_revision: revision.into(),
                policy_hash: policy_hash.into(),
                entries: vec![],
            },
        }
    }
    pub fn replay(
        store: Arc<QuotaStore>,
        grants: Vec<Grant>,
        budget: Budget,
        revision: &str,
        policy_hash: &str,
        trace: Trace,
    ) -> Result<Self> {
        if trace.version != "0.1.0"
            || trace.source_revision != revision
            || trace.policy_hash != policy_hash
        {
            return Err(HostError::ReplayMismatch);
        }
        let mut invocation = Self::record(store, grants, budget, revision, policy_hash);
        invocation.mode = Mode::Replay { cursor: 0 };
        invocation.trace = trace;
        Ok(invocation)
    }
    pub fn cancellation(&self) -> Cancellation {
        Cancellation {
            cancelled: self.cancelled.clone(),
        }
    }
    pub fn active_handles(&self) -> usize {
        if self.cancelled.load(Ordering::SeqCst) {
            0
        } else {
            self.handles.len()
        }
    }
    pub fn revoke(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.handles.clear();
    }
    /// Cooperative execution checkpoint for guest ticks. It checks all active
    /// scope deadlines and cancellation without charging host operations/bytes.
    pub fn checkpoint(&mut self) -> Result<()> {
        if self.cancelled.load(Ordering::SeqCst) {
            self.handles.clear();
            return Err(HostError::Revoked);
        }
        let now = Instant::now();
        if self.meters.iter().any(|meter| now >= meter.deadline) {
            self.revoke();
            return Err(HostError::Timeout);
        }
        Ok(())
    }
    fn charge(&mut self, bytes: u64, memory: u64) -> Result<()> {
        self.checkpoint()?;
        let error = if self.meters.iter().any(|m| {
            m.remaining.operations == 0
                || m.remaining.bytes < bytes
                || m.remaining.memory_bytes < memory
        }) {
            Some(HostError::BudgetExceeded)
        } else {
            None
        };
        if let Some(error) = error {
            self.revoke();
            return Err(error);
        }
        for meter in &mut self.meters {
            meter.remaining.operations -= 1;
            meter.remaining.bytes -= bytes;
            meter.remaining.memory_bytes -= memory;
        }
        Ok(())
    }
    /// Child work charges every active ancestor. Child-created handles expire
    /// on exit, including unwinding; errors revoke the whole invocation.
    pub fn scoped<T>(
        &mut self,
        budget: Budget,
        work: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<T> {
        self.charge(0, 0)?;
        let existing: BTreeSet<_> = self.handles.keys().copied().collect();
        self.meters.push(Meter::new(budget));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(self)));
        let expired = self
            .meters
            .iter()
            .any(|meter| Instant::now() >= meter.deadline);
        self.handles.retain(|id, _| existing.contains(id));
        self.meters.pop();
        match result {
            Err(panic) => {
                self.revoke();
                std::panic::resume_unwind(panic)
            }
            Ok(Err(error)) => {
                self.revoke();
                Err(error)
            }
            Ok(Ok(_)) if expired => {
                self.revoke();
                Err(HostError::Timeout)
            }
            Ok(Ok(_)) if self.cancelled.load(Ordering::SeqCst) => {
                self.revoke();
                Err(HostError::Revoked)
            }
            Ok(Ok(value)) => Ok(value),
        }
    }
    fn permitted(&self, tenant: &str, op: Operation) -> bool {
        self.grants
            .iter()
            .any(|g| g.tenant == tenant && g.operations.contains(&op))
    }
    fn token(&self, h: &Handle) -> Result<u64> {
        if h.invocation != self.id
            || !self.handles.contains_key(&h.id)
            || self.cancelled.load(Ordering::SeqCst)
        {
            Err(HostError::Revoked)
        } else {
            Ok(h.id)
        }
    }
    pub fn begin(&mut self, tenant: &str) -> Result<Handle> {
        if !self.permitted(tenant, Operation::Read) {
            return Err(HostError::Denied);
        }
        let response = self.exchange(Request::Begin {
            tenant: tenant.into(),
        })?;
        if let Response::Handle(id) = response {
            Ok(Handle {
                invocation: self.id,
                id,
            })
        } else {
            self.revoke();
            Err(HostError::ReplayMismatch)
        }
    }
    pub fn read(&mut self, handle: &Handle) -> Result<i64> {
        let id = self.token(handle)?;
        match self.exchange(Request::Read { handle: id })? {
            Response::Balance(s) => s.parse().map_err(|_| HostError::ReplayMismatch),
            _ => Err(HostError::ReplayMismatch),
        }
    }
    pub fn reserve(&mut self, handle: &Handle, amount: i64) -> Result<bool> {
        let id = self.token(handle)?;
        let txn = self.handles.get(&id).ok_or(HostError::Revoked)?;
        if !self.permitted(&txn.tenant, Operation::Write) {
            return Err(HostError::Denied);
        }
        match self.exchange(Request::Reserve {
            handle: id,
            amount: amount.to_string(),
        })? {
            Response::Reserved(b) => Ok(b),
            _ => Err(HostError::ReplayMismatch),
        }
    }
    pub fn commit(&mut self, handle: Handle) -> Result<()> {
        let id = self.token(&handle)?;
        let txn = self.handles.get(&id).ok_or(HostError::Revoked)?;
        if !self.permitted(&txn.tenant, Operation::Write) {
            self.handles.remove(&id);
            return Err(HostError::Denied);
        }
        match self.exchange(Request::Commit { handle: id })? {
            Response::Unit => Ok(()),
            _ => Err(HostError::ReplayMismatch),
        }
    }
    pub fn rollback(&mut self, handle: Handle) -> Result<()> {
        let id = self.token(&handle)?;
        match self.exchange(Request::Rollback { handle: id })? {
            Response::Unit => Ok(()),
            _ => Err(HostError::ReplayMismatch),
        }
    }
    /// Compiler-generated lexical cleanup. Discarding an in-memory pending
    /// transaction is not an external interaction and is deliberately unmetered
    /// and unrecorded. Replay performs the identical token-liveness cleanup.
    /// An already-revoked invocation accepts cleanup of its own owning token.
    pub fn release(&mut self, handle: Handle) -> Result<()> {
        if handle.invocation != self.id {
            return Err(HostError::Revoked);
        }
        if self.cancelled.load(Ordering::SeqCst) {
            self.handles.clear();
            return Ok(());
        }
        self.handles
            .remove(&handle.id)
            .map(|_| ())
            .ok_or(HostError::Revoked)
    }
    fn exchange(&mut self, request: Request) -> Result<Response> {
        let bytes = serde_json::to_vec(&request)
            .map_err(|_| HostError::StorageUnavailable)?
            .len() as u64;
        self.charge(bytes, bytes + 128)?;
        match self.mode {
            Mode::Record => {
                let response = self.live(&request);
                self.trace.entries.push(TraceEntry {
                    request,
                    response: Some(response.clone()),
                });
                response
            }
            Mode::Replay { cursor } => {
                let Some(entry) = self.trace.entries.get(cursor) else {
                    self.revoke();
                    return Err(HostError::MissingRecording);
                };
                if entry.request != request {
                    self.revoke();
                    return Err(HostError::ReplayMismatch);
                }
                let Some(response) = entry.response.clone() else {
                    self.revoke();
                    return Err(HostError::MissingRecording);
                };
                self.mode = Mode::Replay { cursor: cursor + 1 };
                // Owning calls consume their token even when storage reports a
                // conflict. Mirror live liveness without touching the store.
                if let Request::Commit { handle } | Request::Rollback { handle } = &request {
                    self.handles.remove(handle);
                }
                if let Ok(value) = &response {
                    // Rebuild only token liveness, never touch the live store.
                    match (&request, value) {
                        (Request::Begin { tenant }, Response::Handle(id)) => {
                            if *id != self.next_handle {
                                self.revoke();
                                return Err(HostError::ReplayMismatch);
                            }
                            self.next_handle += 1;
                            self.handles.insert(
                                *id,
                                Transaction {
                                    tenant: tenant.clone(),
                                    balance: 0,
                                    revision: 0,
                                    dirty: false,
                                },
                            );
                        }
                        (
                            Request::Commit { handle } | Request::Rollback { handle },
                            Response::Unit,
                        ) => {
                            self.handles.remove(handle);
                        }
                        (Request::Read { .. }, Response::Balance(s))
                            if s.parse::<i64>()
                                .is_ok_and(|value| value >= 0 && value.to_string() == *s) => {}
                        (Request::Reserve { amount, .. }, Response::Reserved(_))
                            if amount
                                .parse::<i64>()
                                .is_ok_and(|value| value > 0 && value.to_string() == *amount) => {}
                        _ => {
                            self.revoke();
                            return Err(HostError::ReplayMismatch);
                        }
                    }
                }
                response
            }
        }
    }
    fn live(&mut self, request: &Request) -> Result<Response> {
        match request {
            Request::Begin { tenant } => {
                let txn = self.store.begin(tenant)?;
                let id = self.next_handle;
                self.next_handle += 1;
                self.handles.insert(id, txn);
                Ok(Response::Handle(id))
            }
            Request::Read { handle } => Ok(Response::Balance(
                self.handles
                    .get(handle)
                    .ok_or(HostError::Revoked)?
                    .balance
                    .to_string(),
            )),
            Request::Reserve { handle, amount } => {
                let amount = amount
                    .parse::<i64>()
                    .map_err(|_| HostError::InvalidAmount)?;
                if amount <= 0 {
                    return Err(HostError::InvalidAmount);
                }
                let txn = self.handles.get_mut(handle).ok_or(HostError::Revoked)?;
                if amount > txn.balance {
                    return Ok(Response::Reserved(false));
                }
                txn.balance -= amount;
                txn.dirty = true;
                Ok(Response::Reserved(true))
            }
            Request::Commit { handle } => {
                let txn = self.handles.remove(handle).ok_or(HostError::Revoked)?;
                self.store.commit(&txn)?;
                Ok(Response::Unit)
            }
            Request::Rollback { handle } => {
                self.handles.remove(handle).ok_or(HostError::Revoked)?;
                Ok(Response::Unit)
            }
        }
    }
    pub fn finish(mut self) -> Result<Trace> {
        if matches!(self.mode,Mode::Replay{cursor} if cursor!=self.trace.entries.len()) {
            self.revoke();
            return Err(HostError::IncompleteReplay);
        }
        self.revoke();
        Ok(self.trace.clone())
    }
}
impl Drop for Invocation {
    fn drop(&mut self) {
        self.revoke();
    }
}
