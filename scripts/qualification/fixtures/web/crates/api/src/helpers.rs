use crate::error::ApiError;
use axum::{Json, http::HeaderMap};
use domain::{Entity, RecordStatus};
use serde::{Deserialize, Serialize};
use store::{ListOptions, Page};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {
    pub q: Option<String>,
    pub status: Option<RecordStatus>,
    pub page: Option<usize>,
    pub per_page: Option<usize>,
}

impl ListQuery {
    pub fn options(&self) -> Result<ListOptions, ApiError> {
        let page = self.page.unwrap_or(1);
        let limit = self.per_page.unwrap_or(25);
        if page == 0 || !(1..=100).contains(&limit) {
            return Err(ApiError::BadRequest(
                "page must be positive and per_page must be 1..=100".into(),
            ));
        }
        let offset = (page - 1)
            .checked_mul(limit)
            .ok_or_else(|| ApiError::BadRequest("pagination overflow".into()))?;
        Ok(ListOptions {
            query: self.q.clone(),
            status: self.status,
            offset,
            limit,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteQuery {
    pub revision: u64,
}

#[derive(Serialize)]
pub struct Envelope<T> {
    pub data: T,
    pub resource: &'static str,
}

#[derive(Serialize)]
pub struct Collection<T> {
    pub data: Vec<T>,
    pub total: usize,
    pub page: usize,
    pub per_page: usize,
    pub has_more: bool,
}

pub fn collection<T, D: From<T>>(page: Page<T>, options: &ListOptions) -> Json<Collection<D>> {
    let has_more = options.offset.saturating_add(page.items.len()) < page.total;
    Json(Collection {
        data: page.items.into_iter().map(D::from).collect(),
        total: page.total,
        page: options.offset / options.limit + 1,
        per_page: options.limit,
        has_more,
    })
}

pub fn envelope<T>(resource: &'static str, data: T) -> Json<Envelope<T>> {
    Json(Envelope { data, resource })
}

pub fn actor(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = match headers.get("x-actor") {
        Some(value) => value
            .to_str()
            .map_err(|_| ApiError::BadRequest("invalid x-actor header".into()))?,
        None => "anonymous",
    };
    domain::validate_text("x-actor", value, 128)?;
    Ok(value.to_owned())
}

pub fn validate_tags(tags: &[String]) -> Result<(), ApiError> {
    if tags.len() > 12 {
        return Err(ApiError::BadRequest("at most 12 tags are allowed".into()));
    }
    for tag in tags {
        domain::validate_text("tags", tag, 32)?;
    }
    Ok(())
}

/// Store one canonical spelling of each user-supplied tag.
pub fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut normalized: Vec<_> = tags
        .into_iter()
        .map(|tag| tag.trim().to_lowercase())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

pub fn revision<T: Entity>(entity: &T) -> u64 {
    entity.version().revision as u64
}
