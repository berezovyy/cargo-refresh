//! Resource handlers for a small operations dashboard. S7 inserts a comment before this line.
use crate::{
    AppState,
    error::ApiError,
    helpers::{self, Collection, DeleteQuery, Envelope, ListQuery},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use domain::{Entity, RecordStatus, VersionStamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A leaf response label reused by the diagnostic probe.
pub fn service_label() -> &'static str {
    let _ = std::panic::catch_unwind(refresh_panic128);
    "S1:handler1"
}

/// Convert an operational base priority into the display weight.
pub fn priority_weight(base: u16) -> u16 {
    base + 1 // S2 leaf arithmetic
}

/// HTTP DTOs and handlers for projects.
pub mod projects {
    use super::*;
    use domain::Project;

    /// Incoming creation payload, independent of the persisted record layout.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CreateProject {
        pub title: String,
        #[serde(default)]
        pub description: String,
        pub owner_email: String,
        pub budget_cents: i64,
        #[serde(default)]
        pub tags: Vec<String>,
    }

    impl CreateProject {
        pub fn validate(&self) -> Result<(), ApiError> {
            domain::validate_text("title", &self.title, 160)?;
            if self.description.chars().count() > 8000 {
                return Err(ApiError::BadRequest(
                    "description exceeds 8000 characters".into(),
                ));
            }
            domain::validate_text("owner_email", &self.owner_email, 128)?;
            domain::validate_nonnegative("budget_cents", self.budget_cents)?;
            helpers::validate_tags(&self.tags)?;

            if !self.owner_email.contains('@') {
                return Err(ApiError::BadRequest(
                    "owner_email must be an email address".into(),
                ));
            }

            Ok(())
        }

        pub fn into_entity(self) -> Project {
            let now = Utc::now();
            Project {
                id: Uuid::new_v4(),
                title: self.title.trim().to_owned(),
                description: self.description,
                owner_email: self.owner_email,
                budget_cents: self.budget_cents,
                status: RecordStatus::Active,
                tags: helpers::normalize_tags(self.tags),
                version: VersionStamp::default(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    /// PATCH fields are optional; revision is required to prevent lost updates.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UpdateProject {
        pub revision: u64,
        pub title: Option<String>,
        pub description: Option<String>,
        pub owner_email: Option<String>,
        pub budget_cents: Option<i64>,
        pub status: Option<RecordStatus>,
        pub tags: Option<Vec<String>>,
    }

    impl UpdateProject {
        pub fn validate(&self) -> Result<(), ApiError> {
            if let Some(title) = &self.title {
                domain::validate_text("title", title, 160)?;
            }
            if let Some(description) = &self.description {
                if description.chars().count() > 8000 {
                    return Err(ApiError::BadRequest(
                        "description exceeds 8000 characters".into(),
                    ));
                }
            }
            if let Some(value) = &self.owner_email {
                domain::validate_text("owner_email", value, 128)?;
            }
            if let Some(value) = self.budget_cents {
                domain::validate_nonnegative("budget_cents", value)?;
            }
            if let Some(tags) = &self.tags {
                helpers::validate_tags(tags)?;
            }
            if self.revision == 0 {
                return Err(ApiError::BadRequest("revision must be positive".into()));
            }
            Ok(())
        }

        pub fn apply(self, record: &mut Project) {
            if let Some(title) = self.title {
                record.title = title.trim().to_owned();
            }
            if let Some(description) = self.description {
                record.description = description;
            }
            if let Some(value) = self.owner_email {
                record.owner_email = value;
            }
            if let Some(value) = self.budget_cents {
                record.budget_cents = value;
            }
            if let Some(status) = self.status {
                record.status = status;
            }
            if let Some(tags) = self.tags {
                record.tags = helpers::normalize_tags(tags);
            }
        }
    }

    /// Navigation links are generated from the canonical API resource name.
    #[derive(Debug, Serialize)]
    pub struct ProjectLinks {
        pub detail: String,
        pub collection: &'static str,
        pub summary: &'static str,
    }

    impl ProjectLinks {
        pub fn for_id(id: Uuid) -> Self {
            Self {
                detail: format!("/v1/projects/{id}"),
                collection: "/v1/projects",
                summary: "/v1/projects/summary",
            }
        }
    }

    /// Explicit response conversion makes the public JSON schema stable.
    #[derive(Debug, Serialize)]
    pub struct ProjectView {
        pub id: Uuid,
        pub title: String,
        pub description: String,
        pub owner_email: String,
        pub budget_cents: i64,
        pub status: RecordStatus,
        pub tags: Vec<String>,
        pub revision: u64,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub archived: bool,
        pub retention_days: u16,
        pub links: ProjectLinks,
    }

    impl From<Project> for ProjectView {
        fn from(record: Project) -> Self {
            let revision = helpers::revision(&record);
            let archived = record.status() == RecordStatus::Archived;
            Self {
                id: record.id,
                title: record.title,
                description: record.description,
                owner_email: record.owner_email,
                budget_cents: record.budget_cents,
                status: record.status,
                tags: record.tags,
                revision,
                created_at: record.created_at,
                updated_at: record.updated_at,
                archived,
                retention_days: domain::retention_days(),
                links: ProjectLinks::for_id(record.id),
            }
        }
    }

    #[derive(Serialize)]
    pub struct ProjectSummary {
        pub resource: &'static str,
        pub by_status: std::collections::BTreeMap<String, usize>,
        pub total: usize,
        pub weighted_total: usize,
        pub retention_days: u16,
        pub service: &'static str,
        pub recent_events: Vec<store::AuditEvent>,
    }

    /// Paginated, filtered collection endpoint.
    #[tracing::instrument(skip(state), fields(resource = "projects"))]
    pub async fn list(
        State(state): State<AppState>,
        Query(query): Query<ListQuery>,
    ) -> Result<Json<Collection<ProjectView>>, ApiError> {
        let options = query.options()?;
        let page = state.db.projects.list(&options).await;
        Ok(helpers::collection(page, &options))
    }

    /// POST returns 201 and the canonical DTO.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "projects"))]
    pub async fn create(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(payload): Json<CreateProject>,
    ) -> Result<(StatusCode, Json<Envelope<ProjectView>>), ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let entity = state.db.projects.insert(payload.into_entity()).await?;
        state
            .db
            .audit
            .record("projects", entity.id, "created", actor)
            .await;
        tracing::info!(id = %entity.id, "created project");
        Ok((
            StatusCode::CREATED,
            helpers::envelope("projects", entity.into()),
        ))
    }

    /// Fetching a missing ID maps repository errors to HTTP 404.
    #[tracing::instrument(skip(state), fields(resource = "projects"))]
    pub async fn get(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
    ) -> Result<Json<Envelope<ProjectView>>, ApiError> {
        let entity = state.db.projects.get(id).await?;
        Ok(helpers::envelope("projects", entity.into()))
    }

    /// Update and revision comparison execute under one repository write lock.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "projects"))]
    pub async fn update(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        headers: HeaderMap,
        Json(payload): Json<UpdateProject>,
    ) -> Result<Json<Envelope<ProjectView>>, ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let expected = payload.revision;
        let entity = state
            .db
            .projects
            .update(id, expected, |record| payload.apply(record))
            .await?;
        state
            .db
            .audit
            .record("projects", id, "updated", actor)
            .await;
        Ok(helpers::envelope("projects", entity.into()))
    }

    /// Deletion requires the last known revision in the query string.
    #[tracing::instrument(skip(state, headers), fields(resource = "projects"))]
    pub async fn delete(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        Query(query): Query<DeleteQuery>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let actor = helpers::actor(&headers)?;
        state.db.projects.delete(id, query.revision).await?;
        state
            .db
            .audit
            .record("projects", id, "deleted", actor)
            .await;
        Ok(StatusCode::NO_CONTENT)
    }

    /// Dashboard aggregation also exercises the shared policy and leaf helpers.
    #[tracing::instrument(skip(state), fields(resource = "projects"))]
    pub async fn summary(State(state): State<AppState>) -> Json<ProjectSummary> {
        let by_status = state.db.projects.summary().await;
        let total = by_status.values().sum::<usize>();
        let recent_events = state.db.audit.recent("projects", 10).await;
        Json(ProjectSummary {
            resource: "projects",
            by_status,
            total,
            weighted_total: total.saturating_mul(priority_weight(0) as usize),
            retention_days: domain::retention_days(),
            service: service_label(),
            recent_events,
        })
    }
}

/// HTTP DTOs and handlers for tickets.
pub mod tickets {
    use super::*;
    use domain::Ticket;

    /// Incoming creation payload, independent of the persisted record layout.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CreateTicket {
        pub title: String,
        #[serde(default)]
        pub description: String,
        pub assignee: String,
        pub priority: i64,
        #[serde(default)]
        pub tags: Vec<String>,
    }

    impl CreateTicket {
        pub fn validate(&self) -> Result<(), ApiError> {
            domain::validate_text("title", &self.title, 160)?;
            if self.description.chars().count() > 8000 {
                return Err(ApiError::BadRequest(
                    "description exceeds 8000 characters".into(),
                ));
            }
            domain::validate_text("assignee", &self.assignee, 128)?;
            domain::validate_nonnegative("priority", self.priority)?;
            helpers::validate_tags(&self.tags)?;

            if !self.assignee.contains('@') {
                return Err(ApiError::BadRequest(
                    "assignee must be an email address".into(),
                ));
            }

            Ok(())
        }

        pub fn into_entity(self) -> Ticket {
            let now = Utc::now();
            Ticket {
                id: Uuid::new_v4(),
                title: self.title.trim().to_owned(),
                description: self.description,
                assignee: self.assignee,
                priority: self.priority,
                status: RecordStatus::Open,
                tags: helpers::normalize_tags(self.tags),
                version: VersionStamp::default(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    /// PATCH fields are optional; revision is required to prevent lost updates.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UpdateTicket {
        pub revision: u64,
        pub title: Option<String>,
        pub description: Option<String>,
        pub assignee: Option<String>,
        pub priority: Option<i64>,
        pub status: Option<RecordStatus>,
        pub tags: Option<Vec<String>>,
    }

    impl UpdateTicket {
        pub fn validate(&self) -> Result<(), ApiError> {
            if let Some(title) = &self.title {
                domain::validate_text("title", title, 160)?;
            }
            if let Some(description) = &self.description {
                if description.chars().count() > 8000 {
                    return Err(ApiError::BadRequest(
                        "description exceeds 8000 characters".into(),
                    ));
                }
            }
            if let Some(value) = &self.assignee {
                domain::validate_text("assignee", value, 128)?;
            }
            if let Some(value) = self.priority {
                domain::validate_nonnegative("priority", value)?;
            }
            if let Some(tags) = &self.tags {
                helpers::validate_tags(tags)?;
            }
            if self.revision == 0 {
                return Err(ApiError::BadRequest("revision must be positive".into()));
            }
            Ok(())
        }

        pub fn apply(self, record: &mut Ticket) {
            if let Some(title) = self.title {
                record.title = title.trim().to_owned();
            }
            if let Some(description) = self.description {
                record.description = description;
            }
            if let Some(value) = self.assignee {
                record.assignee = value;
            }
            if let Some(value) = self.priority {
                record.priority = value;
            }
            if let Some(status) = self.status {
                record.status = status;
            }
            if let Some(tags) = self.tags {
                record.tags = helpers::normalize_tags(tags);
            }
        }
    }

    /// Navigation links are generated from the canonical API resource name.
    #[derive(Debug, Serialize)]
    pub struct TicketLinks {
        pub detail: String,
        pub collection: &'static str,
        pub summary: &'static str,
    }

    impl TicketLinks {
        pub fn for_id(id: Uuid) -> Self {
            Self {
                detail: format!("/v1/tickets/{id}"),
                collection: "/v1/tickets",
                summary: "/v1/tickets/summary",
            }
        }
    }

    /// Explicit response conversion makes the public JSON schema stable.
    #[derive(Debug, Serialize)]
    pub struct TicketView {
        pub id: Uuid,
        pub title: String,
        pub description: String,
        pub assignee: String,
        pub priority: i64,
        pub status: RecordStatus,
        pub tags: Vec<String>,
        pub revision: u64,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub archived: bool,
        pub retention_days: u16,
        pub links: TicketLinks,
    }

    impl From<Ticket> for TicketView {
        fn from(record: Ticket) -> Self {
            let revision = helpers::revision(&record);
            let archived = record.status() == RecordStatus::Archived;
            Self {
                id: record.id,
                title: record.title,
                description: record.description,
                assignee: record.assignee,
                priority: record.priority,
                status: record.status,
                tags: record.tags,
                revision,
                created_at: record.created_at,
                updated_at: record.updated_at,
                archived,
                retention_days: domain::retention_days(),
                links: TicketLinks::for_id(record.id),
            }
        }
    }

    #[derive(Serialize)]
    pub struct TicketSummary {
        pub resource: &'static str,
        pub by_status: std::collections::BTreeMap<String, usize>,
        pub total: usize,
        pub weighted_total: usize,
        pub retention_days: u16,
        pub service: &'static str,
        pub recent_events: Vec<store::AuditEvent>,
    }

    /// Paginated, filtered collection endpoint.
    #[tracing::instrument(skip(state), fields(resource = "tickets"))]
    pub async fn list(
        State(state): State<AppState>,
        Query(query): Query<ListQuery>,
    ) -> Result<Json<Collection<TicketView>>, ApiError> {
        let options = query.options()?;
        let page = state.db.tickets.list(&options).await;
        Ok(helpers::collection(page, &options))
    }

    /// POST returns 201 and the canonical DTO.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "tickets"))]
    pub async fn create(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(payload): Json<CreateTicket>,
    ) -> Result<(StatusCode, Json<Envelope<TicketView>>), ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let entity = state.db.tickets.insert(payload.into_entity()).await?;
        state
            .db
            .audit
            .record("tickets", entity.id, "created", actor)
            .await;
        tracing::info!(id = %entity.id, "created ticket");
        Ok((
            StatusCode::CREATED,
            helpers::envelope("tickets", entity.into()),
        ))
    }

    /// Fetching a missing ID maps repository errors to HTTP 404.
    #[tracing::instrument(skip(state), fields(resource = "tickets"))]
    pub async fn get(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
    ) -> Result<Json<Envelope<TicketView>>, ApiError> {
        let entity = state.db.tickets.get(id).await?;
        Ok(helpers::envelope("tickets", entity.into()))
    }

    /// Update and revision comparison execute under one repository write lock.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "tickets"))]
    pub async fn update(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        headers: HeaderMap,
        Json(payload): Json<UpdateTicket>,
    ) -> Result<Json<Envelope<TicketView>>, ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let expected = payload.revision;
        let entity = state
            .db
            .tickets
            .update(id, expected, |record| payload.apply(record))
            .await?;
        state.db.audit.record("tickets", id, "updated", actor).await;
        Ok(helpers::envelope("tickets", entity.into()))
    }

    /// Deletion requires the last known revision in the query string.
    #[tracing::instrument(skip(state, headers), fields(resource = "tickets"))]
    pub async fn delete(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        Query(query): Query<DeleteQuery>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let actor = helpers::actor(&headers)?;
        state.db.tickets.delete(id, query.revision).await?;
        state.db.audit.record("tickets", id, "deleted", actor).await;
        Ok(StatusCode::NO_CONTENT)
    }

    /// Dashboard aggregation also exercises the shared policy and leaf helpers.
    #[tracing::instrument(skip(state), fields(resource = "tickets"))]
    pub async fn summary(State(state): State<AppState>) -> Json<TicketSummary> {
        let by_status = state.db.tickets.summary().await;
        let total = by_status.values().sum::<usize>();
        let recent_events = state.db.audit.recent("tickets", 10).await;
        Json(TicketSummary {
            resource: "tickets",
            by_status,
            total,
            weighted_total: total.saturating_mul(priority_weight(0) as usize),
            retention_days: domain::retention_days(),
            service: service_label(),
            recent_events,
        })
    }
}

/// HTTP DTOs and handlers for customers.
pub mod customers {
    use super::*;
    use domain::Customer;

    /// Incoming creation payload, independent of the persisted record layout.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CreateCustomer {
        pub title: String,
        #[serde(default)]
        pub description: String,
        pub contact_email: String,
        pub credit_cents: i64,
        #[serde(default)]
        pub tags: Vec<String>,
    }

    impl CreateCustomer {
        pub fn validate(&self) -> Result<(), ApiError> {
            domain::validate_text("title", &self.title, 160)?;
            if self.description.chars().count() > 8000 {
                return Err(ApiError::BadRequest(
                    "description exceeds 8000 characters".into(),
                ));
            }
            domain::validate_text("contact_email", &self.contact_email, 128)?;
            domain::validate_nonnegative("credit_cents", self.credit_cents)?;
            helpers::validate_tags(&self.tags)?;

            if !self.contact_email.contains('@') {
                return Err(ApiError::BadRequest(
                    "contact_email must be an email address".into(),
                ));
            }

            Ok(())
        }

        pub fn into_entity(self) -> Customer {
            let now = Utc::now();
            Customer {
                id: Uuid::new_v4(),
                title: self.title.trim().to_owned(),
                description: self.description,
                contact_email: self.contact_email,
                credit_cents: self.credit_cents,
                status: RecordStatus::Active,
                tags: helpers::normalize_tags(self.tags),
                version: VersionStamp::default(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    /// PATCH fields are optional; revision is required to prevent lost updates.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UpdateCustomer {
        pub revision: u64,
        pub title: Option<String>,
        pub description: Option<String>,
        pub contact_email: Option<String>,
        pub credit_cents: Option<i64>,
        pub status: Option<RecordStatus>,
        pub tags: Option<Vec<String>>,
    }

    impl UpdateCustomer {
        pub fn validate(&self) -> Result<(), ApiError> {
            if let Some(title) = &self.title {
                domain::validate_text("title", title, 160)?;
            }
            if let Some(description) = &self.description {
                if description.chars().count() > 8000 {
                    return Err(ApiError::BadRequest(
                        "description exceeds 8000 characters".into(),
                    ));
                }
            }
            if let Some(value) = &self.contact_email {
                domain::validate_text("contact_email", value, 128)?;
            }
            if let Some(value) = self.credit_cents {
                domain::validate_nonnegative("credit_cents", value)?;
            }
            if let Some(tags) = &self.tags {
                helpers::validate_tags(tags)?;
            }
            if self.revision == 0 {
                return Err(ApiError::BadRequest("revision must be positive".into()));
            }
            Ok(())
        }

        pub fn apply(self, record: &mut Customer) {
            if let Some(title) = self.title {
                record.title = title.trim().to_owned();
            }
            if let Some(description) = self.description {
                record.description = description;
            }
            if let Some(value) = self.contact_email {
                record.contact_email = value;
            }
            if let Some(value) = self.credit_cents {
                record.credit_cents = value;
            }
            if let Some(status) = self.status {
                record.status = status;
            }
            if let Some(tags) = self.tags {
                record.tags = helpers::normalize_tags(tags);
            }
        }
    }

    /// Navigation links are generated from the canonical API resource name.
    #[derive(Debug, Serialize)]
    pub struct CustomerLinks {
        pub detail: String,
        pub collection: &'static str,
        pub summary: &'static str,
    }

    impl CustomerLinks {
        pub fn for_id(id: Uuid) -> Self {
            Self {
                detail: format!("/v1/customers/{id}"),
                collection: "/v1/customers",
                summary: "/v1/customers/summary",
            }
        }
    }

    /// Explicit response conversion makes the public JSON schema stable.
    #[derive(Debug, Serialize)]
    pub struct CustomerView {
        pub id: Uuid,
        pub title: String,
        pub description: String,
        pub contact_email: String,
        pub credit_cents: i64,
        pub status: RecordStatus,
        pub tags: Vec<String>,
        pub revision: u64,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub archived: bool,
        pub retention_days: u16,
        pub links: CustomerLinks,
    }

    impl From<Customer> for CustomerView {
        fn from(record: Customer) -> Self {
            let revision = helpers::revision(&record);
            let archived = record.status() == RecordStatus::Archived;
            Self {
                id: record.id,
                title: record.title,
                description: record.description,
                contact_email: record.contact_email,
                credit_cents: record.credit_cents,
                status: record.status,
                tags: record.tags,
                revision,
                created_at: record.created_at,
                updated_at: record.updated_at,
                archived,
                retention_days: domain::retention_days(),
                links: CustomerLinks::for_id(record.id),
            }
        }
    }

    #[derive(Serialize)]
    pub struct CustomerSummary {
        pub resource: &'static str,
        pub by_status: std::collections::BTreeMap<String, usize>,
        pub total: usize,
        pub weighted_total: usize,
        pub retention_days: u16,
        pub service: &'static str,
        pub recent_events: Vec<store::AuditEvent>,
    }

    /// Paginated, filtered collection endpoint.
    #[tracing::instrument(skip(state), fields(resource = "customers"))]
    pub async fn list(
        State(state): State<AppState>,
        Query(query): Query<ListQuery>,
    ) -> Result<Json<Collection<CustomerView>>, ApiError> {
        let options = query.options()?;
        let page = state.db.customers.list(&options).await;
        Ok(helpers::collection(page, &options))
    }

    /// POST returns 201 and the canonical DTO.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "customers"))]
    pub async fn create(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(payload): Json<CreateCustomer>,
    ) -> Result<(StatusCode, Json<Envelope<CustomerView>>), ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let entity = state.db.customers.insert(payload.into_entity()).await?;
        state
            .db
            .audit
            .record("customers", entity.id, "created", actor)
            .await;
        tracing::info!(id = %entity.id, "created customer");
        Ok((
            StatusCode::CREATED,
            helpers::envelope("customers", entity.into()),
        ))
    }

    /// Fetching a missing ID maps repository errors to HTTP 404.
    #[tracing::instrument(skip(state), fields(resource = "customers"))]
    pub async fn get(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
    ) -> Result<Json<Envelope<CustomerView>>, ApiError> {
        let entity = state.db.customers.get(id).await?;
        Ok(helpers::envelope("customers", entity.into()))
    }

    /// Update and revision comparison execute under one repository write lock.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "customers"))]
    pub async fn update(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        headers: HeaderMap,
        Json(payload): Json<UpdateCustomer>,
    ) -> Result<Json<Envelope<CustomerView>>, ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let expected = payload.revision;
        let entity = state
            .db
            .customers
            .update(id, expected, |record| payload.apply(record))
            .await?;
        state
            .db
            .audit
            .record("customers", id, "updated", actor)
            .await;
        Ok(helpers::envelope("customers", entity.into()))
    }

    /// Deletion requires the last known revision in the query string.
    #[tracing::instrument(skip(state, headers), fields(resource = "customers"))]
    pub async fn delete(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        Query(query): Query<DeleteQuery>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let actor = helpers::actor(&headers)?;
        state.db.customers.delete(id, query.revision).await?;
        state
            .db
            .audit
            .record("customers", id, "deleted", actor)
            .await;
        Ok(StatusCode::NO_CONTENT)
    }

    /// Dashboard aggregation also exercises the shared policy and leaf helpers.
    #[tracing::instrument(skip(state), fields(resource = "customers"))]
    pub async fn summary(State(state): State<AppState>) -> Json<CustomerSummary> {
        let by_status = state.db.customers.summary().await;
        let total = by_status.values().sum::<usize>();
        let recent_events = state.db.audit.recent("customers", 10).await;
        Json(CustomerSummary {
            resource: "customers",
            by_status,
            total,
            weighted_total: total.saturating_mul(priority_weight(0) as usize),
            retention_days: domain::retention_days(),
            service: service_label(),
            recent_events,
        })
    }
}

/// HTTP DTOs and handlers for invoices.
pub mod invoices {
    use super::*;
    use domain::Invoice;

    /// Incoming creation payload, independent of the persisted record layout.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CreateInvoice {
        pub title: String,
        #[serde(default)]
        pub description: String,
        pub customer_reference: String,
        pub amount_cents: i64,
        #[serde(default)]
        pub tags: Vec<String>,
    }

    impl CreateInvoice {
        pub fn validate(&self) -> Result<(), ApiError> {
            domain::validate_text("title", &self.title, 160)?;
            if self.description.chars().count() > 8000 {
                return Err(ApiError::BadRequest(
                    "description exceeds 8000 characters".into(),
                ));
            }
            domain::validate_text("customer_reference", &self.customer_reference, 128)?;
            domain::validate_nonnegative("amount_cents", self.amount_cents)?;
            helpers::validate_tags(&self.tags)?;

            domain::validate_text("customer_reference", &self.customer_reference, 128)?;

            Ok(())
        }

        pub fn into_entity(self) -> Invoice {
            let now = Utc::now();
            Invoice {
                id: Uuid::new_v4(),
                title: self.title.trim().to_owned(),
                description: self.description,
                customer_reference: self.customer_reference,
                amount_cents: self.amount_cents,
                status: RecordStatus::Draft,
                tags: helpers::normalize_tags(self.tags),
                version: VersionStamp::default(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    /// PATCH fields are optional; revision is required to prevent lost updates.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UpdateInvoice {
        pub revision: u64,
        pub title: Option<String>,
        pub description: Option<String>,
        pub customer_reference: Option<String>,
        pub amount_cents: Option<i64>,
        pub status: Option<RecordStatus>,
        pub tags: Option<Vec<String>>,
    }

    impl UpdateInvoice {
        pub fn validate(&self) -> Result<(), ApiError> {
            if let Some(title) = &self.title {
                domain::validate_text("title", title, 160)?;
            }
            if let Some(description) = &self.description {
                if description.chars().count() > 8000 {
                    return Err(ApiError::BadRequest(
                        "description exceeds 8000 characters".into(),
                    ));
                }
            }
            if let Some(value) = &self.customer_reference {
                domain::validate_text("customer_reference", value, 128)?;
            }
            if let Some(value) = self.amount_cents {
                domain::validate_nonnegative("amount_cents", value)?;
            }
            if let Some(tags) = &self.tags {
                helpers::validate_tags(tags)?;
            }
            if self.revision == 0 {
                return Err(ApiError::BadRequest("revision must be positive".into()));
            }
            Ok(())
        }

        pub fn apply(self, record: &mut Invoice) {
            if let Some(title) = self.title {
                record.title = title.trim().to_owned();
            }
            if let Some(description) = self.description {
                record.description = description;
            }
            if let Some(value) = self.customer_reference {
                record.customer_reference = value;
            }
            if let Some(value) = self.amount_cents {
                record.amount_cents = value;
            }
            if let Some(status) = self.status {
                record.status = status;
            }
            if let Some(tags) = self.tags {
                record.tags = helpers::normalize_tags(tags);
            }
        }
    }

    /// Navigation links are generated from the canonical API resource name.
    #[derive(Debug, Serialize)]
    pub struct InvoiceLinks {
        pub detail: String,
        pub collection: &'static str,
        pub summary: &'static str,
    }

    impl InvoiceLinks {
        pub fn for_id(id: Uuid) -> Self {
            Self {
                detail: format!("/v1/invoices/{id}"),
                collection: "/v1/invoices",
                summary: "/v1/invoices/summary",
            }
        }
    }

    /// Explicit response conversion makes the public JSON schema stable.
    #[derive(Debug, Serialize)]
    pub struct InvoiceView {
        pub id: Uuid,
        pub title: String,
        pub description: String,
        pub customer_reference: String,
        pub amount_cents: i64,
        pub status: RecordStatus,
        pub tags: Vec<String>,
        pub revision: u64,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub archived: bool,
        pub retention_days: u16,
        pub links: InvoiceLinks,
    }

    impl From<Invoice> for InvoiceView {
        fn from(record: Invoice) -> Self {
            let revision = helpers::revision(&record);
            let archived = record.status() == RecordStatus::Archived;
            Self {
                id: record.id,
                title: record.title,
                description: record.description,
                customer_reference: record.customer_reference,
                amount_cents: record.amount_cents,
                status: record.status,
                tags: record.tags,
                revision,
                created_at: record.created_at,
                updated_at: record.updated_at,
                archived,
                retention_days: domain::retention_days(),
                links: InvoiceLinks::for_id(record.id),
            }
        }
    }

    #[derive(Serialize)]
    pub struct InvoiceSummary {
        pub resource: &'static str,
        pub by_status: std::collections::BTreeMap<String, usize>,
        pub total: usize,
        pub weighted_total: usize,
        pub retention_days: u16,
        pub service: &'static str,
        pub recent_events: Vec<store::AuditEvent>,
    }

    /// Paginated, filtered collection endpoint.
    #[tracing::instrument(skip(state), fields(resource = "invoices"))]
    pub async fn list(
        State(state): State<AppState>,
        Query(query): Query<ListQuery>,
    ) -> Result<Json<Collection<InvoiceView>>, ApiError> {
        let options = query.options()?;
        let page = state.db.invoices.list(&options).await;
        Ok(helpers::collection(page, &options))
    }

    /// POST returns 201 and the canonical DTO.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "invoices"))]
    pub async fn create(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(payload): Json<CreateInvoice>,
    ) -> Result<(StatusCode, Json<Envelope<InvoiceView>>), ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let entity = state.db.invoices.insert(payload.into_entity()).await?;
        state
            .db
            .audit
            .record("invoices", entity.id, "created", actor)
            .await;
        tracing::info!(id = %entity.id, "created invoice");
        Ok((
            StatusCode::CREATED,
            helpers::envelope("invoices", entity.into()),
        ))
    }

    /// Fetching a missing ID maps repository errors to HTTP 404.
    #[tracing::instrument(skip(state), fields(resource = "invoices"))]
    pub async fn get(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
    ) -> Result<Json<Envelope<InvoiceView>>, ApiError> {
        let entity = state.db.invoices.get(id).await?;
        Ok(helpers::envelope("invoices", entity.into()))
    }

    /// Update and revision comparison execute under one repository write lock.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "invoices"))]
    pub async fn update(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        headers: HeaderMap,
        Json(payload): Json<UpdateInvoice>,
    ) -> Result<Json<Envelope<InvoiceView>>, ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let expected = payload.revision;
        let entity = state
            .db
            .invoices
            .update(id, expected, |record| payload.apply(record))
            .await?;
        state
            .db
            .audit
            .record("invoices", id, "updated", actor)
            .await;
        Ok(helpers::envelope("invoices", entity.into()))
    }

    /// Deletion requires the last known revision in the query string.
    #[tracing::instrument(skip(state, headers), fields(resource = "invoices"))]
    pub async fn delete(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        Query(query): Query<DeleteQuery>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let actor = helpers::actor(&headers)?;
        state.db.invoices.delete(id, query.revision).await?;
        state
            .db
            .audit
            .record("invoices", id, "deleted", actor)
            .await;
        Ok(StatusCode::NO_CONTENT)
    }

    /// Dashboard aggregation also exercises the shared policy and leaf helpers.
    #[tracing::instrument(skip(state), fields(resource = "invoices"))]
    pub async fn summary(State(state): State<AppState>) -> Json<InvoiceSummary> {
        let by_status = state.db.invoices.summary().await;
        let total = by_status.values().sum::<usize>();
        let recent_events = state.db.audit.recent("invoices", 10).await;
        Json(InvoiceSummary {
            resource: "invoices",
            by_status,
            total,
            weighted_total: total.saturating_mul(priority_weight(0) as usize),
            retention_days: domain::retention_days(),
            service: service_label(),
            recent_events,
        })
    }
}

/// HTTP DTOs and handlers for notes.
pub mod notes {
    use super::*;
    use domain::Note;

    /// Incoming creation payload, independent of the persisted record layout.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CreateNote {
        pub title: String,
        #[serde(default)]
        pub description: String,
        pub author: String,
        pub word_limit: i64,
        #[serde(default)]
        pub tags: Vec<String>,
    }

    impl CreateNote {
        pub fn validate(&self) -> Result<(), ApiError> {
            domain::validate_text("title", &self.title, 160)?;
            if self.description.chars().count() > 8000 {
                return Err(ApiError::BadRequest(
                    "description exceeds 8000 characters".into(),
                ));
            }
            domain::validate_text("author", &self.author, 128)?;
            domain::validate_nonnegative("word_limit", self.word_limit)?;
            helpers::validate_tags(&self.tags)?;

            if !self.author.contains('@') {
                return Err(ApiError::BadRequest(
                    "author must be an email address".into(),
                ));
            }

            Ok(())
        }

        pub fn into_entity(self) -> Note {
            let now = Utc::now();
            Note {
                id: Uuid::new_v4(),
                title: self.title.trim().to_owned(),
                description: self.description,
                author: self.author,
                word_limit: self.word_limit,
                status: RecordStatus::Draft,
                tags: helpers::normalize_tags(self.tags),
                version: VersionStamp::default(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    /// PATCH fields are optional; revision is required to prevent lost updates.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UpdateNote {
        pub revision: u64,
        pub title: Option<String>,
        pub description: Option<String>,
        pub author: Option<String>,
        pub word_limit: Option<i64>,
        pub status: Option<RecordStatus>,
        pub tags: Option<Vec<String>>,
    }

    impl UpdateNote {
        pub fn validate(&self) -> Result<(), ApiError> {
            if let Some(title) = &self.title {
                domain::validate_text("title", title, 160)?;
            }
            if let Some(description) = &self.description {
                if description.chars().count() > 8000 {
                    return Err(ApiError::BadRequest(
                        "description exceeds 8000 characters".into(),
                    ));
                }
            }
            if let Some(value) = &self.author {
                domain::validate_text("author", value, 128)?;
            }
            if let Some(value) = self.word_limit {
                domain::validate_nonnegative("word_limit", value)?;
            }
            if let Some(tags) = &self.tags {
                helpers::validate_tags(tags)?;
            }
            if self.revision == 0 {
                return Err(ApiError::BadRequest("revision must be positive".into()));
            }
            Ok(())
        }

        pub fn apply(self, record: &mut Note) {
            if let Some(title) = self.title {
                record.title = title.trim().to_owned();
            }
            if let Some(description) = self.description {
                record.description = description;
            }
            if let Some(value) = self.author {
                record.author = value;
            }
            if let Some(value) = self.word_limit {
                record.word_limit = value;
            }
            if let Some(status) = self.status {
                record.status = status;
            }
            if let Some(tags) = self.tags {
                record.tags = helpers::normalize_tags(tags);
            }
        }
    }

    /// Navigation links are generated from the canonical API resource name.
    #[derive(Debug, Serialize)]
    pub struct NoteLinks {
        pub detail: String,
        pub collection: &'static str,
        pub summary: &'static str,
    }

    impl NoteLinks {
        pub fn for_id(id: Uuid) -> Self {
            Self {
                detail: format!("/v1/notes/{id}"),
                collection: "/v1/notes",
                summary: "/v1/notes/summary",
            }
        }
    }

    /// Explicit response conversion makes the public JSON schema stable.
    #[derive(Debug, Serialize)]
    pub struct NoteView {
        pub id: Uuid,
        pub title: String,
        pub description: String,
        pub author: String,
        pub word_limit: i64,
        pub status: RecordStatus,
        pub tags: Vec<String>,
        pub revision: u64,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub archived: bool,
        pub retention_days: u16,
        pub links: NoteLinks,
    }

    impl From<Note> for NoteView {
        fn from(record: Note) -> Self {
            let revision = helpers::revision(&record);
            let archived = record.status() == RecordStatus::Archived;
            Self {
                id: record.id,
                title: record.title,
                description: record.description,
                author: record.author,
                word_limit: record.word_limit,
                status: record.status,
                tags: record.tags,
                revision,
                created_at: record.created_at,
                updated_at: record.updated_at,
                archived,
                retention_days: domain::retention_days(),
                links: NoteLinks::for_id(record.id),
            }
        }
    }

    #[derive(Serialize)]
    pub struct NoteSummary {
        pub resource: &'static str,
        pub by_status: std::collections::BTreeMap<String, usize>,
        pub total: usize,
        pub weighted_total: usize,
        pub retention_days: u16,
        pub service: &'static str,
        pub recent_events: Vec<store::AuditEvent>,
    }

    /// Paginated, filtered collection endpoint.
    #[tracing::instrument(skip(state), fields(resource = "notes"))]
    pub async fn list(
        State(state): State<AppState>,
        Query(query): Query<ListQuery>,
    ) -> Result<Json<Collection<NoteView>>, ApiError> {
        let options = query.options()?;
        let page = state.db.notes.list(&options).await;
        Ok(helpers::collection(page, &options))
    }

    /// POST returns 201 and the canonical DTO.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "notes"))]
    pub async fn create(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(payload): Json<CreateNote>,
    ) -> Result<(StatusCode, Json<Envelope<NoteView>>), ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let entity = state.db.notes.insert(payload.into_entity()).await?;
        state
            .db
            .audit
            .record("notes", entity.id, "created", actor)
            .await;
        tracing::info!(id = %entity.id, "created note");
        Ok((
            StatusCode::CREATED,
            helpers::envelope("notes", entity.into()),
        ))
    }

    /// Fetching a missing ID maps repository errors to HTTP 404.
    #[tracing::instrument(skip(state), fields(resource = "notes"))]
    pub async fn get(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
    ) -> Result<Json<Envelope<NoteView>>, ApiError> {
        let entity = state.db.notes.get(id).await?;
        Ok(helpers::envelope("notes", entity.into()))
    }

    /// Update and revision comparison execute under one repository write lock.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "notes"))]
    pub async fn update(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        headers: HeaderMap,
        Json(payload): Json<UpdateNote>,
    ) -> Result<Json<Envelope<NoteView>>, ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let expected = payload.revision;
        let entity = state
            .db
            .notes
            .update(id, expected, |record| payload.apply(record))
            .await?;
        state.db.audit.record("notes", id, "updated", actor).await;
        Ok(helpers::envelope("notes", entity.into()))
    }

    /// Deletion requires the last known revision in the query string.
    #[tracing::instrument(skip(state, headers), fields(resource = "notes"))]
    pub async fn delete(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        Query(query): Query<DeleteQuery>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let actor = helpers::actor(&headers)?;
        state.db.notes.delete(id, query.revision).await?;
        state.db.audit.record("notes", id, "deleted", actor).await;
        Ok(StatusCode::NO_CONTENT)
    }

    /// Dashboard aggregation also exercises the shared policy and leaf helpers.
    #[tracing::instrument(skip(state), fields(resource = "notes"))]
    pub async fn summary(State(state): State<AppState>) -> Json<NoteSummary> {
        let by_status = state.db.notes.summary().await;
        let total = by_status.values().sum::<usize>();
        let recent_events = state.db.audit.recent("notes", 10).await;
        Json(NoteSummary {
            resource: "notes",
            by_status,
            total,
            weighted_total: total.saturating_mul(priority_weight(0) as usize),
            retention_days: domain::retention_days(),
            service: service_label(),
            recent_events,
        })
    }
}

/// HTTP DTOs and handlers for labels.
pub mod labels {
    use super::*;
    use domain::Label;

    /// Incoming creation payload, independent of the persisted record layout.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CreateLabel {
        pub title: String,
        #[serde(default)]
        pub description: String,
        pub color: String,
        pub sort_order: i64,
        #[serde(default)]
        pub tags: Vec<String>,
    }

    impl CreateLabel {
        pub fn validate(&self) -> Result<(), ApiError> {
            domain::validate_text("title", &self.title, 160)?;
            if self.description.chars().count() > 8000 {
                return Err(ApiError::BadRequest(
                    "description exceeds 8000 characters".into(),
                ));
            }
            domain::validate_text("color", &self.color, 128)?;
            domain::validate_nonnegative("sort_order", self.sort_order)?;
            helpers::validate_tags(&self.tags)?;

            domain::validate_text("color", &self.color, 128)?;

            Ok(())
        }

        pub fn into_entity(self) -> Label {
            let now = Utc::now();
            Label {
                id: Uuid::new_v4(),
                title: self.title.trim().to_owned(),
                description: self.description,
                color: self.color,
                sort_order: self.sort_order,
                status: RecordStatus::Active,
                tags: helpers::normalize_tags(self.tags),
                version: VersionStamp::default(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    /// PATCH fields are optional; revision is required to prevent lost updates.
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct UpdateLabel {
        pub revision: u64,
        pub title: Option<String>,
        pub description: Option<String>,
        pub color: Option<String>,
        pub sort_order: Option<i64>,
        pub status: Option<RecordStatus>,
        pub tags: Option<Vec<String>>,
    }

    impl UpdateLabel {
        pub fn validate(&self) -> Result<(), ApiError> {
            if let Some(title) = &self.title {
                domain::validate_text("title", title, 160)?;
            }
            if let Some(description) = &self.description {
                if description.chars().count() > 8000 {
                    return Err(ApiError::BadRequest(
                        "description exceeds 8000 characters".into(),
                    ));
                }
            }
            if let Some(value) = &self.color {
                domain::validate_text("color", value, 128)?;
            }
            if let Some(value) = self.sort_order {
                domain::validate_nonnegative("sort_order", value)?;
            }
            if let Some(tags) = &self.tags {
                helpers::validate_tags(tags)?;
            }
            if self.revision == 0 {
                return Err(ApiError::BadRequest("revision must be positive".into()));
            }
            Ok(())
        }

        pub fn apply(self, record: &mut Label) {
            if let Some(title) = self.title {
                record.title = title.trim().to_owned();
            }
            if let Some(description) = self.description {
                record.description = description;
            }
            if let Some(value) = self.color {
                record.color = value;
            }
            if let Some(value) = self.sort_order {
                record.sort_order = value;
            }
            if let Some(status) = self.status {
                record.status = status;
            }
            if let Some(tags) = self.tags {
                record.tags = helpers::normalize_tags(tags);
            }
        }
    }

    /// Navigation links are generated from the canonical API resource name.
    #[derive(Debug, Serialize)]
    pub struct LabelLinks {
        pub detail: String,
        pub collection: &'static str,
        pub summary: &'static str,
    }

    impl LabelLinks {
        pub fn for_id(id: Uuid) -> Self {
            Self {
                detail: format!("/v1/labels/{id}"),
                collection: "/v1/labels",
                summary: "/v1/labels/summary",
            }
        }
    }

    /// Explicit response conversion makes the public JSON schema stable.
    #[derive(Debug, Serialize)]
    pub struct LabelView {
        pub id: Uuid,
        pub title: String,
        pub description: String,
        pub color: String,
        pub sort_order: i64,
        pub status: RecordStatus,
        pub tags: Vec<String>,
        pub revision: u64,
        pub created_at: DateTime<Utc>,
        pub updated_at: DateTime<Utc>,
        pub archived: bool,
        pub retention_days: u16,
        pub links: LabelLinks,
    }

    impl From<Label> for LabelView {
        fn from(record: Label) -> Self {
            let revision = helpers::revision(&record);
            let archived = record.status() == RecordStatus::Archived;
            Self {
                id: record.id,
                title: record.title,
                description: record.description,
                color: record.color,
                sort_order: record.sort_order,
                status: record.status,
                tags: record.tags,
                revision,
                created_at: record.created_at,
                updated_at: record.updated_at,
                archived,
                retention_days: domain::retention_days(),
                links: LabelLinks::for_id(record.id),
            }
        }
    }

    #[derive(Serialize)]
    pub struct LabelSummary {
        pub resource: &'static str,
        pub by_status: std::collections::BTreeMap<String, usize>,
        pub total: usize,
        pub weighted_total: usize,
        pub retention_days: u16,
        pub service: &'static str,
        pub recent_events: Vec<store::AuditEvent>,
    }

    /// Paginated, filtered collection endpoint.
    #[tracing::instrument(skip(state), fields(resource = "labels"))]
    pub async fn list(
        State(state): State<AppState>,
        Query(query): Query<ListQuery>,
    ) -> Result<Json<Collection<LabelView>>, ApiError> {
        let options = query.options()?;
        let page = state.db.labels.list(&options).await;
        Ok(helpers::collection(page, &options))
    }

    /// POST returns 201 and the canonical DTO.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "labels"))]
    pub async fn create(
        State(state): State<AppState>,
        headers: HeaderMap,
        Json(payload): Json<CreateLabel>,
    ) -> Result<(StatusCode, Json<Envelope<LabelView>>), ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let entity = state.db.labels.insert(payload.into_entity()).await?;
        state
            .db
            .audit
            .record("labels", entity.id, "created", actor)
            .await;
        tracing::info!(id = %entity.id, "created label");
        Ok((
            StatusCode::CREATED,
            helpers::envelope("labels", entity.into()),
        ))
    }

    /// Fetching a missing ID maps repository errors to HTTP 404.
    #[tracing::instrument(skip(state), fields(resource = "labels"))]
    pub async fn get(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
    ) -> Result<Json<Envelope<LabelView>>, ApiError> {
        let entity = state.db.labels.get(id).await?;
        Ok(helpers::envelope("labels", entity.into()))
    }

    /// Update and revision comparison execute under one repository write lock.
    #[tracing::instrument(skip(state, headers, payload), fields(resource = "labels"))]
    pub async fn update(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        headers: HeaderMap,
        Json(payload): Json<UpdateLabel>,
    ) -> Result<Json<Envelope<LabelView>>, ApiError> {
        let actor = helpers::actor(&headers)?;
        payload.validate()?;
        let expected = payload.revision;
        let entity = state
            .db
            .labels
            .update(id, expected, |record| payload.apply(record))
            .await?;
        state.db.audit.record("labels", id, "updated", actor).await;
        Ok(helpers::envelope("labels", entity.into()))
    }

    /// Deletion requires the last known revision in the query string.
    #[tracing::instrument(skip(state, headers), fields(resource = "labels"))]
    pub async fn delete(
        State(state): State<AppState>,
        Path(id): Path<Uuid>,
        Query(query): Query<DeleteQuery>,
        headers: HeaderMap,
    ) -> Result<StatusCode, ApiError> {
        let actor = helpers::actor(&headers)?;
        state.db.labels.delete(id, query.revision).await?;
        state.db.audit.record("labels", id, "deleted", actor).await;
        Ok(StatusCode::NO_CONTENT)
    }

    /// Dashboard aggregation also exercises the shared policy and leaf helpers.
    #[tracing::instrument(skip(state), fields(resource = "labels"))]
    pub async fn summary(State(state): State<AppState>) -> Json<LabelSummary> {
        let by_status = state.db.labels.summary().await;
        let total = by_status.values().sum::<usize>();
        let recent_events = state.db.audit.recent("labels", 10).await;
        Json(LabelSummary {
            resource: "labels",
            by_status,
            total,
            weighted_total: total.saturating_mul(priority_weight(0) as usize),
            retention_days: domain::retention_days(),
            service: service_label(),
            recent_events,
        })
    }
}

/// Async shape safety probe used by the extra four saves.
pub async fn refresh_async_label() -> &'static str {
    tokio::task::yield_now().await;
    ""
}

fn refresh_panic128() { if false { panic!("EXP128 caught web panic"); } }
