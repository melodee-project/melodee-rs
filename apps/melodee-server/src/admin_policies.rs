//! Phase 10 WP-16 Policy Rule API (PRD-POLICY-008, PRD-API-ADMIN-002,
//! SPEC §15/§19.4/§20): versioned rule CRUD over the durable rule-set model
//! and the audited "test against catalog" dry-run.
//!
//! Rule sets are immutable and versioned (ADR-0120): every mutation loads the
//! latest version of the default rule set, applies the change, validates the
//! result through the real [`PolicyEngine`] invariants (administrator-override
//! first, corrupt-media quarantine second, per-type disposition safety), and
//! persists it as a new `(name, version)` row. Nothing is edited in place and
//! history is never rewritten; a concurrent mutation loses with
//! `409 rule_set_conflict`.
//!
//! `enable`/`disable` are expressed as rule membership in the latest version
//! — the durable model has no per-rule enabled flag. Disabling removes the
//! rule from a new version (its definition survives in immutable history);
//! enabling restores the most recent historical definition at the lowest
//! priority. The two safety rules (administrator override, corrupt-media
//! quarantine) can be neither deleted nor disabled.
//!
//! The running policy worker binds its engine to the configured rule-set
//! version at startup (`MELODEE__POLICY__RULE_SET_VERSION`); a new version
//! authored here is durable immediately and adopted by the worker when its
//! configured version matches (restart/redeploy). This surface never mutates
//! a work item or decision.
//!
//! Test runs are read-only dry-runs over a bounded catalog snapshot. Runs
//! against a persisted rule (by `ruleId`) are stored in `policy_test_runs`
//! for audit. Runs carrying an ad-hoc `rule` definition cannot be persisted:
//! `policy_test_runs` has a foreign key to `policy_rule_sets`, and persisting
//! the candidate set would silently change the active policy. Those runs
//! return `persisted: false` (schema/ADR gap, recorded in the WP-16 report).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use melodee_domain::{
    Confidence, Disposition, PolicyActor, PolicyRule, PolicyRuleSet, PolicyRuleType,
};
use melodee_persistence::{PolicyStore, PolicyStoreError, PolicyTestSampleRelease};
use melodee_policy::{
    DEFAULT_RULE_SET_NAME, PolicyConfig, PolicyDryRunner, PolicyEngine, PolicyInput,
    default_rule_set_id, evaluate_catalog_context,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::admin_access::{AdminAccessErrorBody, AdminAccessState, AdminPrincipal};

/// Upper bound on catalog Releases evaluated by one test run. The durable
/// bound is `MAX_POLICY_TEST_RELEASES`; the HTTP surface keeps test runs
/// interactive by capping at the engine batch bound.
const MAX_TEST_RUN_RELEASES: usize = 500;

fn policy_error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(AdminAccessErrorBody {
            code,
            message: message.into(),
            correlation_id: Uuid::now_v7(),
        }),
    )
        .into_response()
}

fn store_failure(context: &'static str, error: &impl std::fmt::Display) -> Response {
    tracing::error!(%error, context, "admin policy store failure");
    policy_error(StatusCode::INTERNAL_SERVER_ERROR, "store_error", context)
}

/// One rule as the operator sees it. `priority` is the zero-based evaluation
/// position inside the active rule set (lower wins; evaluation is ordered,
/// first match applies); it is `null` for disabled rules, which are listed
/// from immutable history so they can be re-enabled.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRuleDto {
    pub id: String,
    pub rule_type: String,
    pub disposition: String,
    /// Confidence floor in basis points (0–10000); required by the domain
    /// invariant for destructive dispositions.
    pub confidence_threshold: Option<u16>,
    pub predicate: serde_json::Value,
    pub priority: Option<usize>,
    pub enabled: bool,
    /// How many durable policy decisions this rule produced.
    pub match_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRuleSetDto {
    pub id: Uuid,
    pub name: String,
    pub version: u32,
    pub created_at: OffsetDateTime,
    pub created_by: serde_json::Value,
}

fn rule_set_dto(rule_set: &PolicyRuleSet) -> PolicyRuleSetDto {
    PolicyRuleSetDto {
        id: rule_set.id.into_uuid(),
        name: rule_set.name.clone(),
        version: rule_set.version,
        created_at: rule_set.created_at,
        created_by: serde_json::to_value(&rule_set.created_by).unwrap_or(serde_json::Value::Null),
    }
}

fn rule_dto(
    rule: &PolicyRule,
    priority: Option<usize>,
    enabled: bool,
    match_count: i64,
) -> PolicyRuleDto {
    PolicyRuleDto {
        id: rule.id.clone(),
        rule_type: rule.rule_type.as_str().to_string(),
        disposition: rule.disposition.as_str().to_string(),
        confidence_threshold: rule.confidence_threshold.map(Confidence::basis_points),
        predicate: rule.predicate_json.clone(),
        priority,
        enabled,
        match_count,
    }
}

/// Paginated-envelope rule listing (uniform with the other admin listings).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRulePageDto {
    pub rule_set: PolicyRuleSetDto,
    pub items: Vec<PolicyRuleDto>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// Rule definition accepted by create/update/test. `predicate` uses the
/// engine predicate schema (`required_traits`, `existing_dispositions`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleInput {
    pub id: String,
    pub rule_type: String,
    pub disposition: String,
    #[serde(default)]
    pub confidence_threshold: Option<u16>,
    #[serde(default)]
    pub predicate: serde_json::Value,
}

impl RuleInput {
    #[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
    fn into_rule(self) -> Result<PolicyRule, Response> {
        let id = self.id.trim().to_string();
        if id.is_empty() || id.len() > 200 {
            return Err(policy_error(
                StatusCode::BAD_REQUEST,
                "invalid_rule_id",
                "rule id must be non-blank and at most 200 characters",
            ));
        }
        let Some(rule_type) = PolicyRuleType::parse(&self.rule_type) else {
            return Err(policy_error(
                StatusCode::BAD_REQUEST,
                "unknown_rule_type",
                format!("unknown rule type '{}'", self.rule_type),
            ));
        };
        let Some(disposition) = Disposition::parse(&self.disposition) else {
            return Err(policy_error(
                StatusCode::BAD_REQUEST,
                "unknown_disposition",
                format!("unknown disposition '{}'", self.disposition),
            ));
        };
        let confidence_threshold = self
            .confidence_threshold
            .map(Confidence::new)
            .transpose()
            .map_err(|error| {
                policy_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_confidence",
                    error.to_string(),
                )
            })?;
        let rule = PolicyRule {
            id,
            rule_type,
            predicate_json: self.predicate,
            disposition,
            confidence_threshold,
        };
        rule.validate().map_err(|error| {
            policy_error(StatusCode::BAD_REQUEST, "invalid_rule", error.to_string())
        })?;
        Ok(rule)
    }
}

/// Result of a rule mutation: the rule as it now stands plus the new
/// immutable rule-set version carrying the change.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleMutationDto {
    pub rule_set: PolicyRuleSetDto,
    pub rule: PolicyRuleDto,
    /// False when the request was an idempotent no-op (enable of an enabled
    /// rule, disable of an already-disabled rule).
    pub changed: bool,
}

/// Test-run request: exactly one of `ruleId` (test an existing enabled rule
/// in the active rule set) or `rule` (test an ad-hoc definition appended to
/// — or replacing — the active set, without persisting it).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRuleRequest {
    pub rule_id: Option<String>,
    pub rule: Option<RuleInput>,
    /// Maximum Releases to evaluate (clamped to MAX_TEST_RUN_RELEASES).
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyTestSampleDto {
    pub release_id: Uuid,
    pub title: String,
    pub artist: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyTestRunDto {
    /// Null when the run could not be persisted (ad-hoc rule candidate; see
    /// module docs).
    pub test_run_id: Option<Uuid>,
    pub persisted: bool,
    pub rule_set_id: Uuid,
    pub rule_set_version: u32,
    pub evaluated: u32,
    /// Releases matched by the tested rule.
    pub matched_count: u32,
    /// Releases matched per rule id (evaluation is ordered; first match wins).
    pub matched_rule_counts: std::collections::BTreeMap<String, u32>,
    pub proposed_disposition_counts: std::collections::BTreeMap<String, u32>,
    pub existing_disposition_counts: std::collections::BTreeMap<String, u32>,
    /// Releases whose proposed disposition is destructive (delete).
    pub destructive_impact: u32,
    /// Whether the tested rule's action is destructive-class.
    pub destructive: bool,
    pub sample: Vec<PolicyTestSampleDto>,
}

pub fn router() -> Router<AdminAccessState> {
    Router::new()
        .route(
            "/api/admin/v1/policies/rules",
            get(list_rules).post(create_rule),
        )
        .route(
            "/api/admin/v1/policies/rules/{id}",
            axum::routing::put(update_rule).delete(delete_rule),
        )
        .route(
            "/api/admin/v1/policies/rules/{id}/enable",
            post(enable_rule),
        )
        .route(
            "/api/admin/v1/policies/rules/{id}/disable",
            post(disable_rule),
        )
        .route("/api/admin/v1/policies/rules/test", post(test_rule))
        .route_layer(axum::extract::DefaultBodyLimit::max(64 * 1024))
}

async fn load_active_rule_set(pool: &sqlx::PgPool) -> Result<PolicyRuleSet, Response> {
    match PolicyStore.load_rule_set(pool, DEFAULT_RULE_SET_NAME).await {
        Ok(Some(rule_set)) => Ok(rule_set),
        Ok(None) => Err(policy_error(
            StatusCode::CONFLICT,
            "rule_set_not_found",
            "no policy rule set is persisted yet; start the policy worker once to provision the default rule set",
        )),
        Err(error) => Err(store_failure("rule set read failed", &error)),
    }
}

/// Validates a candidate rule set through the real engine invariants
/// (ordering of the safety rules, predicate schema, per-type disposition
/// safety, unique ids) without persisting anything.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
fn validate_candidate(rule_set: PolicyRuleSet) -> Result<PolicyRuleSet, Response> {
    let config = PolicyConfig {
        rule_set_version: rule_set.version,
        ..PolicyConfig::default()
    };
    PolicyEngine::with_rule_set(config, rule_set)
        .map(|engine| engine.rule_set().clone())
        .map_err(|error| {
            policy_error(
                StatusCode::BAD_REQUEST,
                "invalid_rule_set",
                format!("the resulting rule set violates engine invariants: {error}"),
            )
        })
}

/// Persists `mutate`'s result as a new immutable version of the active rule
/// set. Version allocation is read-latest-then-increment; a concurrent
/// mutation collides on `(name, version)` and loses with 409.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn mutate_rule_set(
    pool: &sqlx::PgPool,
    admin: &AdminPrincipal,
    mutate: impl FnOnce(&PolicyRuleSet) -> Result<Vec<PolicyRule>, Response>,
) -> Result<PolicyRuleSet, Response> {
    let latest = load_active_rule_set(pool).await?;
    let rules = mutate(&latest)?;
    let new_version = latest.version.saturating_add(1);
    let now = OffsetDateTime::now_utc();
    let candidate = PolicyRuleSet {
        id: default_rule_set_id(new_version),
        name: latest.name.clone(),
        version: new_version,
        rules,
        created_at: now,
        created_by: PolicyActor::Administrator {
            actor: admin.public_api_id.to_string(),
        },
    };
    let candidate = PolicyRuleSet::new(candidate).map_err(|error| {
        policy_error(
            StatusCode::BAD_REQUEST,
            "invalid_rule_set",
            error.to_string(),
        )
    })?;
    let candidate = validate_candidate(candidate)?;
    match PolicyStore.ensure_rule_set(pool, &candidate, now).await {
        Ok(stored) => Ok(stored),
        Err(PolicyStoreError::RuleSetConflict) => Err(policy_error(
            StatusCode::CONFLICT,
            "rule_set_conflict",
            "the rule set changed concurrently; reload and retry",
        )),
        Err(error) => Err(store_failure("rule set mutation failed", &error)),
    }
}

/// The administrator-override and corrupt-media safety rules anchor every
/// valid rule set (engine invariants); they can be neither removed nor
/// disabled through this surface.
fn protected_position(rule_set: &PolicyRuleSet, rule_id: &str) -> bool {
    let first_ok = rule_set
        .rules
        .first()
        .is_some_and(|rule| rule.rule_type == PolicyRuleType::AdministratorOverride);
    let second_ok = rule_set
        .rules
        .get(1)
        .is_some_and(|rule| rule.rule_type == PolicyRuleType::CorruptMedia);
    rule_set
        .rules
        .iter()
        .take(2)
        .enumerate()
        .any(|(index, rule)| {
            rule.id == rule_id && ((index == 0 && first_ok) || (index == 1 && second_ok))
        })
}

fn match_count_map(counts: Vec<(String, i64)>) -> std::collections::BTreeMap<String, i64> {
    counts.into_iter().collect()
}

/// GET /api/admin/v1/policies/rules — rules of the active rule set plus
/// disabled rules known from immutable history, with durable match counts.
async fn list_rules(State(state): State<AdminAccessState>, _admin: AdminPrincipal) -> Response {
    let pool = &state.pool;
    let latest = match load_active_rule_set(pool).await {
        Ok(rule_set) => rule_set,
        Err(response) => return response,
    };
    let versions = match PolicyStore.list_rule_set_versions(pool, &latest.name).await {
        Ok(versions) => versions,
        Err(error) => return store_failure("rule set history read failed", &error),
    };
    let match_counts = match PolicyStore.rule_match_counts(pool).await {
        Ok(counts) => match_count_map(counts),
        Err(error) => return store_failure("rule match counts failed", &error),
    };
    let mut items: Vec<PolicyRuleDto> = latest
        .rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            rule_dto(
                rule,
                Some(index),
                true,
                match_counts.get(&rule.id).copied().unwrap_or(0),
            )
        })
        .collect();
    // Disabled rules: present in history, absent from the latest version.
    let mut disabled: std::collections::BTreeMap<String, PolicyRule> =
        std::collections::BTreeMap::new();
    for version in &versions {
        for rule in &version.rules {
            if latest.find_rule(&rule.id).is_none() {
                disabled.insert(rule.id.clone(), rule.clone());
            }
        }
    }
    for rule in disabled.values() {
        items.push(rule_dto(
            rule,
            None,
            false,
            match_counts.get(&rule.id).copied().unwrap_or(0),
        ));
    }
    let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
    let limit = u32::try_from(items.len()).unwrap_or(u32::MAX);
    Json(PolicyRulePageDto {
        rule_set: rule_set_dto(&latest),
        items,
        total,
        limit,
        offset: 0,
    })
    .into_response()
}

/// POST /api/admin/v1/policies/rules — append a new rule (lowest priority)
/// as a new rule-set version.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn create_rule(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(input): Json<RuleInput>,
) -> Response {
    let rule = match input.into_rule() {
        Ok(rule) => rule,
        Err(response) => return response,
    };
    let rule_id = rule.id.clone();
    let disposition = rule.disposition;
    let stored = match mutate_rule_set(&state.pool, &admin, |latest| {
        if latest.find_rule(&rule_id).is_some() {
            return Err(policy_error(
                StatusCode::CONFLICT,
                "rule_exists",
                format!("rule '{rule_id}' already exists in the active rule set"),
            ));
        }
        let mut rules = latest.rules.clone();
        rules.push(rule);
        Ok(rules)
    })
    .await
    {
        Ok(stored) => stored,
        Err(response) => return response,
    };
    tracing::info!(
        actor = %admin.public_api_id,
        rule = %rule_id,
        disposition = disposition.as_str(),
        rule_set_version = stored.version,
        "admin created policy rule"
    );
    let Some(created) = stored.find_rule(&rule_id) else {
        return store_failure(
            "created rule missing from stored version",
            &"rule_set_drift",
        );
    };
    let priority = stored.rules.iter().position(|rule| rule.id == rule_id);
    (
        StatusCode::CREATED,
        Json(RuleMutationDto {
            rule_set: rule_set_dto(&stored),
            rule: rule_dto(created, priority, true, 0),
            changed: true,
        }),
    )
        .into_response()
}

/// PUT /api/admin/v1/policies/rules/{id} — replace a rule definition in
/// place within a new version (priority position preserved).
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn update_rule(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<String>,
    Json(input): Json<RuleInput>,
) -> Response {
    if input.id != id {
        return policy_error(
            StatusCode::BAD_REQUEST,
            "rule_id_mismatch",
            "the path id and body id must match; renaming a rule is delete + create",
        );
    }
    let rule = match input.into_rule() {
        Ok(rule) => rule,
        Err(response) => return response,
    };
    let stored = match mutate_rule_set(&state.pool, &admin, |latest| {
        let Some(position) = latest.rules.iter().position(|existing| existing.id == id) else {
            return Err(policy_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("rule '{id}' is not in the active rule set"),
            ));
        };
        let mut rules = latest.rules.clone();
        rules[position] = rule;
        Ok(rules)
    })
    .await
    {
        Ok(stored) => stored,
        Err(response) => return response,
    };
    tracing::info!(
        actor = %admin.public_api_id,
        rule = %id,
        rule_set_version = stored.version,
        "admin updated policy rule"
    );
    let Some(updated) = stored.find_rule(&id) else {
        return store_failure(
            "updated rule missing from stored version",
            &"rule_set_drift",
        );
    };
    let priority = stored.rules.iter().position(|rule| rule.id == id);
    Json(RuleMutationDto {
        rule_set: rule_set_dto(&stored),
        rule: rule_dto(updated, priority, true, 0),
        changed: true,
    })
    .into_response()
}

/// DELETE /api/admin/v1/policies/rules/{id} — remove a rule in a new
/// version. The safety rules are protected; history retains the definition.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn delete_rule(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<String>,
) -> Response {
    let stored = match mutate_rule_set(&state.pool, &admin, |latest| {
        if latest.find_rule(&id).is_none() {
            return Err(policy_error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("rule '{id}' is not in the active rule set"),
            ));
        }
        if protected_position(latest, &id) {
            return Err(policy_error(
                StatusCode::CONFLICT,
                "protected_rule",
                format!(
                    "rule '{id}' is a safety rule required by the engine and cannot be deleted"
                ),
            ));
        }
        Ok(latest
            .rules
            .iter()
            .filter(|rule| rule.id != id)
            .cloned()
            .collect())
    })
    .await
    {
        Ok(stored) => stored,
        Err(response) => return response,
    };
    tracing::info!(
        actor = %admin.public_api_id,
        rule = %id,
        rule_set_version = stored.version,
        "admin deleted policy rule"
    );
    Json(serde_json::json!({
        "ruleSet": rule_set_dto(&stored),
        "ruleId": id,
        "deleted": true,
    }))
    .into_response()
}

/// POST /api/admin/v1/policies/rules/{id}/enable — restore the rule's most
/// recent historical definition at the lowest priority of a new version.
/// Idempotent: enabling an enabled rule is a 200 no-op.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn enable_rule(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<String>,
) -> Response {
    let pool = &state.pool;
    let latest = match load_active_rule_set(pool).await {
        Ok(rule_set) => rule_set,
        Err(response) => return response,
    };
    if let Some(rule) = latest.find_rule(&id) {
        let priority = latest.rules.iter().position(|existing| existing.id == id);
        return Json(RuleMutationDto {
            rule_set: rule_set_dto(&latest),
            rule: rule_dto(rule, priority, true, 0),
            changed: false,
        })
        .into_response();
    }
    let versions = match PolicyStore.list_rule_set_versions(pool, &latest.name).await {
        Ok(versions) => versions,
        Err(error) => return store_failure("rule set history read failed", &error),
    };
    let Some(definition) = versions
        .iter()
        .rev()
        .find_map(|version| version.find_rule(&id).cloned())
    else {
        return policy_error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("rule '{id}' has never existed in this rule set"),
        );
    };
    let stored = match mutate_rule_set(pool, &admin, |latest| {
        let mut rules = latest.rules.clone();
        rules.push(definition);
        Ok(rules)
    })
    .await
    {
        Ok(stored) => stored,
        Err(response) => return response,
    };
    tracing::info!(
        actor = %admin.public_api_id,
        rule = %id,
        rule_set_version = stored.version,
        "admin enabled policy rule"
    );
    let Some(rule) = stored.find_rule(&id) else {
        return store_failure(
            "enabled rule missing from stored version",
            &"rule_set_drift",
        );
    };
    let priority = stored.rules.iter().position(|existing| existing.id == id);
    Json(RuleMutationDto {
        rule_set: rule_set_dto(&stored),
        rule: rule_dto(rule, priority, true, 0),
        changed: true,
    })
    .into_response()
}

/// POST /api/admin/v1/policies/rules/{id}/disable — remove the rule from a
/// new version (its definition survives in immutable history). Idempotent:
/// disabling an absent rule is a 200 no-op.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn disable_rule(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Path(id): Path<String>,
) -> Response {
    let pool = &state.pool;
    let latest = match load_active_rule_set(pool).await {
        Ok(rule_set) => rule_set,
        Err(response) => return response,
    };
    if latest.find_rule(&id).is_none() {
        return Json(serde_json::json!({
            "ruleSet": rule_set_dto(&latest),
            "ruleId": id,
            "enabled": false,
            "changed": false,
        }))
        .into_response();
    }
    if protected_position(&latest, &id) {
        return policy_error(
            StatusCode::CONFLICT,
            "protected_rule",
            format!("rule '{id}' is a safety rule required by the engine and cannot be disabled"),
        );
    }
    let stored = match mutate_rule_set(pool, &admin, |latest| {
        Ok(latest
            .rules
            .iter()
            .filter(|existing| existing.id != id)
            .cloned()
            .collect())
    })
    .await
    {
        Ok(stored) => stored,
        Err(response) => return response,
    };
    tracing::info!(
        actor = %admin.public_api_id,
        rule = %id,
        rule_set_version = stored.version,
        "admin disabled policy rule"
    );
    Json(serde_json::json!({
        "ruleSet": rule_set_dto(&stored),
        "ruleId": id,
        "enabled": false,
        "changed": true,
    }))
    .into_response()
}

/// POST /api/admin/v1/policies/rules/test — read-only dry-run of one rule
/// against a bounded catalog snapshot. Rule-definition runs are validated
/// against the candidate rule set but never persisted (module docs explain
/// the foreign-key boundary); ruleId runs are stored in `policy_test_runs`.
#[allow(clippy::result_large_err)] // Err is an axum Response (route error carrier)
async fn test_rule(
    State(state): State<AdminAccessState>,
    admin: AdminPrincipal,
    Json(request): Json<TestRuleRequest>,
) -> Response {
    let pool = &state.pool;
    let latest = match load_active_rule_set(pool).await {
        Ok(rule_set) => rule_set,
        Err(response) => return response,
    };
    let (candidate, tested_rule_id, persistable) = match (request.rule_id, request.rule) {
        (Some(rule_id), None) => {
            if latest.find_rule(&rule_id).is_none() {
                return policy_error(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("rule '{rule_id}' is not in the active rule set"),
                );
            }
            (latest.clone(), rule_id, true)
        }
        (None, Some(input)) => {
            let rule = match input.into_rule() {
                Ok(rule) => rule,
                Err(response) => return response,
            };
            let tested_id = rule.id.clone();
            let mut rules = latest
                .rules
                .iter()
                .filter(|existing| existing.id != tested_id)
                .cloned()
                .collect::<Vec<_>>();
            rules.push(rule);
            let candidate = PolicyRuleSet {
                id: default_rule_set_id(latest.version.saturating_add(1)),
                name: latest.name.clone(),
                version: latest.version.saturating_add(1),
                rules,
                created_at: OffsetDateTime::now_utc(),
                created_by: PolicyActor::Administrator {
                    actor: admin.public_api_id.to_string(),
                },
            };
            let candidate = match PolicyRuleSet::new(candidate)
                .map_err(|error| {
                    policy_error(
                        StatusCode::BAD_REQUEST,
                        "invalid_rule_set",
                        error.to_string(),
                    )
                })
                .and_then(validate_candidate)
            {
                Ok(candidate) => candidate,
                Err(response) => return response,
            };
            (candidate, tested_id, false)
        }
        _ => {
            return policy_error(
                StatusCode::BAD_REQUEST,
                "invalid_test_request",
                "exactly one of ruleId or rule is required",
            );
        }
    };
    let destructive = candidate
        .find_rule(&tested_rule_id)
        .is_some_and(|rule| rule.disposition.is_destructive());
    let config = PolicyConfig {
        rule_set_version: candidate.version,
        ..PolicyConfig::default()
    };
    let engine = match PolicyEngine::with_rule_set(config, candidate.clone()) {
        Ok(engine) => engine,
        Err(error) => {
            return store_failure("validated rule set failed engine construction", &error);
        }
    };
    let limit = request
        .limit
        .unwrap_or(MAX_TEST_RUN_RELEASES as u32)
        .clamp(1, MAX_TEST_RUN_RELEASES as u32)
        .min(u32::try_from(engine.batch_size()).unwrap_or(u32::MAX));
    let contexts = match PolicyStore.load_test_contexts(pool, limit).await {
        Ok(contexts) => contexts,
        Err(error) => return store_failure("test snapshot read failed", &error),
    };
    let now = OffsetDateTime::now_utc();
    let mut inputs = Vec::with_capacity(contexts.len());
    for context in &contexts {
        // Classify + assess through the same public mapping the policy worker
        // uses, then hand the validated facts to the dry-run aggregator.
        let evaluation = match evaluate_catalog_context(&engine, context, now) {
            Ok(evaluation) => evaluation,
            Err(error) => {
                return store_failure("policy test evaluation failed", &error);
            }
        };
        inputs.push(PolicyInput {
            classification: evaluation.classification.clone(),
            novelty: evaluation.novelty.clone(),
            existing_disposition: evaluation.existing_disposition,
            source_fingerprint: context.claim.source_fingerprint.clone(),
            has_corrupt_media: context.has_corrupt_media,
            administrator_override: None,
            now,
        });
    }
    let dry_run = match PolicyDryRunner::new(engine).run_dry_run(inputs, now) {
        Ok(dry_run) => dry_run,
        Err(error) => return store_failure("policy dry run failed", &error),
    };
    let evaluated = match u32::try_from(dry_run.results.len()) {
        Ok(evaluated) => evaluated,
        Err(error) => return store_failure("policy dry run count overflow", &error),
    };
    let matched_count = dry_run
        .matched_rule_counts
        .get(&tested_rule_id)
        .copied()
        .unwrap_or(0);

    // Persist the audit row when the candidate is a durably stored rule set
    // (the policy_test_runs foreign key requires it).
    let mut test_run_id = None;
    if persistable {
        let summary = melodee_domain::PolicyTestSummary {
            test_run_id: melodee_domain::PolicyTestRunId::new(),
            rule_set_id: candidate.id,
            rule_set_version: candidate.version,
            total_releases_evaluated: evaluated,
            matched_rule_counts: dry_run
                .matched_rule_counts
                .iter()
                .map(|(id, count)| (id.clone(), *count))
                .collect(),
            existing_disposition_counts: dry_run
                .existing_disposition_counts
                .iter()
                .map(|(key, count)| {
                    (
                        if key == "none" {
                            None
                        } else {
                            Disposition::parse(key)
                        },
                        *count,
                    )
                })
                .collect(),
            disposition_counts: dry_run
                .proposed_disposition_counts
                .iter()
                .filter_map(|(key, count)| Disposition::parse(key).map(|d| (d, *count)))
                .collect(),
            destructive_impact_count: dry_run.destructive_impact_count,
            sample_release_ids: dry_run.sample_release_ids.clone(),
            ran_by: PolicyActor::Administrator {
                actor: admin.public_api_id.to_string(),
            },
            ran_at: now,
        };
        let summary = match melodee_domain::PolicyTestSummary::new(summary) {
            Ok(summary) => summary,
            Err(error) => return store_failure("policy test summary construction failed", &error),
        };
        match PolicyStore.store_test_run(pool, &summary).await {
            Ok(()) => test_run_id = Some(summary.test_run_id.into_uuid()),
            Err(error) => return store_failure("policy test run persistence failed", &error),
        }
    }

    let sample_rows = match PolicyStore
        .test_sample_releases(pool, &dry_run.sample_release_ids)
        .await
    {
        Ok(rows) => rows,
        Err(error) => return store_failure("test sample enrichment failed", &error),
    };
    let titles: std::collections::BTreeMap<Uuid, (String, Option<String>)> = sample_rows
        .into_iter()
        .map(
            |PolicyTestSampleRelease {
                 release_id,
                 title,
                 artist,
             }| (release_id, (title, artist)),
        )
        .collect();
    let sample = dry_run
        .sample_release_ids
        .iter()
        .map(|release_id| {
            let (title, artist) = titles
                .get(&release_id.into_uuid())
                .cloned()
                .unwrap_or_else(|| (String::new(), None));
            PolicyTestSampleDto {
                release_id: release_id.into_uuid(),
                title,
                artist,
            }
        })
        .collect();
    tracing::info!(
        actor = %admin.public_api_id,
        rule = %tested_rule_id,
        rule_set_version = candidate.version,
        evaluated,
        matched_count,
        destructive_impact = dry_run.destructive_impact_count,
        persisted = persistable,
        "admin ran policy test against catalog"
    );
    Json(PolicyTestRunDto {
        test_run_id,
        persisted: persistable,
        rule_set_id: candidate.id.into_uuid(),
        rule_set_version: candidate.version,
        evaluated,
        matched_count,
        matched_rule_counts: dry_run.matched_rule_counts,
        proposed_disposition_counts: dry_run.proposed_disposition_counts,
        existing_disposition_counts: dry_run.existing_disposition_counts,
        destructive_impact: dry_run.destructive_impact_count,
        destructive,
        sample,
    })
    .into_response()
}
