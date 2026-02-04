use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;

#[derive(Debug, Deserialize, ToSchema)]
pub struct PaginationQuery {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PageMeta {
    pub offset: i64,
    pub limit: i64,
    pub total: i64,
}

pub fn normalize_pagination(query: &PaginationQuery) -> (i64, i64) {
    let offset = query.offset.unwrap_or(0).max(0);
    let limit = query
        .limit
        .unwrap_or(DEFAULT_LIMIT)
        .max(1)
        .min(MAX_LIMIT);
    (offset, limit)
}
