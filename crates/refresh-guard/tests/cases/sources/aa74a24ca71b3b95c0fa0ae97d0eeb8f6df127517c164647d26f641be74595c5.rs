//! Shared persisted models and validation policy for a small operations backend.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[derive(statearena::Persist)]
pub struct VersionStamp {
    pub revision: u32, // S4 prepared variant: u64 is accepted by every consumer.
}

impl Default for VersionStamp {
    fn default() -> Self {
        Self { revision: 1 }
    }
}

/// Default retention for archived business records, also exercised by the probe.
pub fn retention_days() -> u16 {
    30 + 6 // S3 deep policy body
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProbeSchema {
    #[serde(rename = "S5:v1")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Active,
    Open,
    Draft,
    Archived,
}

impl std::fmt::Display for RecordStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Active => "active",
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Archived => "archived",
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{field}: {message}")]
pub struct ValidationError {
    pub field: &'static str,
    pub message: String,
}

pub fn validate_text(field: &'static str, value: &str, max: usize) -> Result<(), ValidationError> {
    let count = value.trim().chars().count();
    if count == 0 || count > max {
        return Err(ValidationError {
            field,
            message: format!("must contain 1..={max} characters"),
        });
    }
    if value
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(ValidationError {
            field,
            message: "contains control characters".into(),
        });
    }
    Ok(())
}

pub fn validate_nonnegative(field: &'static str, value: i64) -> Result<(), ValidationError> {
    if !(0..=1_000_000_000).contains(&value) {
        return Err(ValidationError {
            field,
            message: "must be between 0 and 1000000000".into(),
        });
    }
    Ok(())
}

pub trait Entity: Clone + Send + Sync + 'static {
    fn id(&self) -> Uuid;
    fn title(&self) -> &str;
    fn status(&self) -> RecordStatus;
    fn version(&self) -> VersionStamp;
    fn created_at(&self) -> DateTime<Utc>;
    fn mark_updated(&mut self);
}

/// A project owned by the operations service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(statearena::Persist)]
pub struct Project {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub owner_email: String,
    pub budget_cents: i64,
    pub status: RecordStatus,
    pub tags: Vec<String>,
    pub version: VersionStamp,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Entity for Project {
    fn id(&self) -> Uuid {
        self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn status(&self) -> RecordStatus {
        self.status
    }
    fn version(&self) -> VersionStamp {
        self.version
    }
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn mark_updated(&mut self) {
        self.version.revision += 1;
        self.updated_at = Utc::now();
    }
}

/// A ticket owned by the operations service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(statearena::Persist)]
pub struct Ticket {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub assignee: String,
    pub priority: i64,
    pub status: RecordStatus,
    pub tags: Vec<String>,
    pub version: VersionStamp,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Entity for Ticket {
    fn id(&self) -> Uuid {
        self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn status(&self) -> RecordStatus {
        self.status
    }
    fn version(&self) -> VersionStamp {
        self.version
    }
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn mark_updated(&mut self) {
        self.version.revision += 1;
        self.updated_at = Utc::now();
    }
}

/// A customer owned by the operations service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(statearena::Persist)]
pub struct Customer {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub contact_email: String,
    pub credit_cents: i64,
    pub status: RecordStatus,
    pub tags: Vec<String>,
    pub version: VersionStamp,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Entity for Customer {
    fn id(&self) -> Uuid {
        self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn status(&self) -> RecordStatus {
        self.status
    }
    fn version(&self) -> VersionStamp {
        self.version
    }
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn mark_updated(&mut self) {
        self.version.revision += 1;
        self.updated_at = Utc::now();
    }
}

/// A invoice owned by the operations service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(statearena::Persist)]
pub struct Invoice {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub customer_reference: String,
    pub amount_cents: i64,
    pub status: RecordStatus,
    pub tags: Vec<String>,
    pub version: VersionStamp,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Entity for Invoice {
    fn id(&self) -> Uuid {
        self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn status(&self) -> RecordStatus {
        self.status
    }
    fn version(&self) -> VersionStamp {
        self.version
    }
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn mark_updated(&mut self) {
        self.version.revision += 1;
        self.updated_at = Utc::now();
    }
}

/// A note owned by the operations service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(statearena::Persist)]
pub struct Note {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub author: String,
    pub word_limit: i64,
    pub status: RecordStatus,
    pub tags: Vec<String>,
    pub version: VersionStamp,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Entity for Note {
    fn id(&self) -> Uuid {
        self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn status(&self) -> RecordStatus {
        self.status
    }
    fn version(&self) -> VersionStamp {
        self.version
    }
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn mark_updated(&mut self) {
        self.version.revision += 1;
        self.updated_at = Utc::now();
    }
}

/// A label owned by the operations service.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(statearena::Persist)]
pub struct Label {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub color: String,
    pub sort_order: i64,
    pub status: RecordStatus,
    pub tags: Vec<String>,
    pub version: VersionStamp,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Entity for Label {
    fn id(&self) -> Uuid {
        self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn status(&self) -> RecordStatus {
        self.status
    }
    fn version(&self) -> VersionStamp {
        self.version
    }
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn mark_updated(&mut self) {
        self.version.revision += 1;
        self.updated_at = Utc::now();
    }
}

impl Default for RecordStatus { fn default()->Self { Self::Active } }
impl statearena::Persist for RecordStatus {
 fn schema()->statearena::Schema { statearena::Schema::record("RecordStatus/v1",size_of::<Self>(),vec![]) }
 fn encode(&self)->statearena::Value { statearena::Value::Text(serde_json::to_string(self).unwrap()) }
 fn decode(v:&statearena::Value,s:&statearena::Schema,_:&mut Vec<String>)->statearena::Result<Self> {
  if s.name!="RecordStatus/v1" {return Err("status schema mismatch".into())}
  match v { statearena::Value::Text(x)=>serde_json::from_str(x).map_err(|e|e.to_string()), _=>Err("expected status".into()) }
 }
}
