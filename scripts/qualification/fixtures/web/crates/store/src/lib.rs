//! Shared in-memory repositories; locks cover read-modify-write transactions.
use chrono::{DateTime, Utc};
use domain::{Entity, RecordStatus};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::Arc,
};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("record {0} was not found")]
    NotFound(Uuid),
    #[error("record already exists")]
    Duplicate,
    #[error("revision mismatch: expected {expected}, current {actual}")]
    Conflict { expected: u64, actual: u64 },
}

#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    pub query: Option<String>,
    pub status: Option<RecordStatus>,
    pub offset: usize,
    pub limit: usize,
}

pub struct Page<T> {
    pub items: Vec<T>,
    pub total: usize,
}

#[derive(Clone)]
pub struct Repository<T> {
    records: Arc<RwLock<HashMap<Uuid, T>>>,
}

impl<T> Default for Repository<T> {
    fn default() -> Self {
        Self {
            records: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl<T: Entity> Repository<T> {
    pub async fn list(&self, options: &ListOptions) -> Page<T> {
        let records = self.records.read().await;
        let query = options.query.as_ref().map(|q| q.to_lowercase());
        let mut matches: Vec<_> = records
            .values()
            .filter(|record| {
                options
                    .status
                    .is_none_or(|status| record.status() == status)
                    && query
                        .as_ref()
                        .is_none_or(|q| record.title().to_lowercase().contains(q))
            })
            .cloned()
            .collect();
        matches.sort_by_key(|record| (record.created_at(), record.id()));
        let total = matches.len();
        let items = matches
            .into_iter()
            .skip(options.offset)
            .take(options.limit)
            .collect();
        Page { items, total }
    }

    pub async fn get(&self, id: Uuid) -> Result<T, StoreError> {
        self.records
            .read()
            .await
            .get(&id)
            .cloned()
            .ok_or(StoreError::NotFound(id))
    }

    pub async fn insert(&self, entity: T) -> Result<T, StoreError> {
        let mut records = self.records.write().await;
        if records.contains_key(&entity.id()) {
            return Err(StoreError::Duplicate);
        }
        records.insert(entity.id(), entity.clone());
        Ok(entity)
    }

    pub async fn update<F>(&self, id: Uuid, expected: u64, change: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut T),
    {
        let mut records = self.records.write().await;
        let record = records.get_mut(&id).ok_or(StoreError::NotFound(id))?;
        let actual = record.version().revision as u64;
        if actual != expected {
            return Err(StoreError::Conflict { expected, actual });
        }
        change(record);
        record.mark_updated();
        Ok(record.clone())
    }

    pub async fn delete(&self, id: Uuid, expected: u64) -> Result<T, StoreError> {
        let mut records = self.records.write().await;
        let record = records.get(&id).ok_or(StoreError::NotFound(id))?;
        let actual = record.version().revision as u64;
        if actual != expected {
            return Err(StoreError::Conflict { expected, actual });
        }
        Ok(records
            .remove(&id)
            .expect("record checked under write lock"))
    }

    pub async fn summary(&self) -> BTreeMap<String, usize> {
        let records = self.records.read().await;
        let mut counts = BTreeMap::new();
        for record in records.values() {
            *counts.entry(record.status().to_string()).or_default() += 1;
        }
        counts
    }
}

#[derive(Debug, Clone, Serialize)]
#[derive(statearena::Persist)]
pub struct AuditEvent {
    pub resource: String,
    pub record_id: Uuid,
    pub action: String,
    pub actor: String,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Default)]
pub struct AuditLog(Arc<RwLock<VecDeque<AuditEvent>>>);

impl AuditLog {
    pub async fn record(
        &self,
        resource: &'static str,
        record_id: Uuid,
        action: &'static str,
        actor: String,
    ) {
        let mut events = self.0.write().await;
        if events.len() == 1024 {
            events.pop_front();
        }
        events.push_back(AuditEvent {
            resource: resource.into(),
            record_id,
            action: action.into(),
            actor,
            at: Utc::now(),
        });
    }

    pub async fn recent(&self, resource: &str, limit: usize) -> Vec<AuditEvent> {
        self.0
            .read()
            .await
            .iter()
            .rev()
            .filter(|event| event.resource == resource)
            .take(limit)
            .cloned()
            .collect()
    }
}

#[derive(Clone, Default)]
pub struct Database {
    pub projects: Repository<domain::Project>,
    pub tickets: Repository<domain::Ticket>,
    pub customers: Repository<domain::Customer>,
    pub invoices: Repository<domain::Invoice>,
    pub notes: Repository<domain::Note>,
    pub labels: Repository<domain::Label>,
    pub audit: AuditLog,
}

#[derive(Default, statearena::Persist)]
pub struct Snapshot {
 pub projects: Vec<domain::Project>, pub tickets: Vec<domain::Ticket>,
 pub customers: Vec<domain::Customer>, pub invoices: Vec<domain::Invoice>,
 pub notes: Vec<domain::Note>, pub labels: Vec<domain::Label>,
 pub audit: Vec<AuditEvent>, pub ticks: u64,
}
impl<T: Entity> Repository<T> {
 pub async fn snapshot(&self)->Vec<T> { let mut v:Vec<T>=self.records.read().await.values().cloned().collect();v.sort_by_key(|v|v.id());v }
 pub async fn restore(&self, v:Vec<T>) { *self.records.write().await=v.into_iter().map(|v|(v.id(),v)).collect(); }
}
impl Database {
 pub async fn snapshot(&self,ticks:u64)->Snapshot { Snapshot {
 projects:self.projects.snapshot().await,tickets:self.tickets.snapshot().await,
 customers:self.customers.snapshot().await,invoices:self.invoices.snapshot().await,
 notes:self.notes.snapshot().await,labels:self.labels.snapshot().await,
 audit:self.audit.0.read().await.iter().cloned().collect(),ticks,
 } }
 pub async fn restore(&self,s:Snapshot) {
 self.projects.restore(s.projects).await;self.tickets.restore(s.tickets).await;
 self.customers.restore(s.customers).await;self.invoices.restore(s.invoices).await;
 self.notes.restore(s.notes).await;self.labels.restore(s.labels).await;
 *self.audit.0.write().await=s.audit.into_iter().collect();
 }
}
