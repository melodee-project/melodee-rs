//! Policy operator commands: dry-run testing, evaluation, inspection, and
//! audited administrator overrides over the durable policy store.

use std::collections::BTreeMap;

use clap::{Subcommand, ValueEnum};
use melodee_persistence::{PolicyAdminOverrideRequest, PolicyStore};
use melodee_policy::{
    PolicyConfig, PolicyEngine, PolicyEvaluation, default_rule_set, evaluate_catalog_context,
};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::common::pool;

#[derive(Debug, Subcommand)]
pub(crate) enum PolicyCommands {
    /// Test policy rules against a sample of existing catalog Releases.
    /// Reports matched-rule counts, existing dispositions, and destructive impact.
    Test {
        /// Maximum number of Releases to test.
        #[arg(
            long,
            default_value_t = 100,
            value_parser = clap::value_parser!(u32).range(1..=10_000)
        )]
        limit: u32,
        /// Stable administrator identity recorded with the test summary.
        #[arg(long, value_parser = super::common::parse_nonblank)]
        actor: String,
    },
    /// Evaluate a single Release: classify, assess novelty, and produce a
    /// disposition without persisting or executing.
    Evaluate {
        /// Release UUID (v7).
        id: Uuid,
    },
    /// Inspect the durable current policy decision and audit history.
    Inspect {
        /// Release UUID (v7).
        id: Uuid,
    },
    /// Record an explicit Administrator override as a pending decision.
    Decide {
        /// Release UUID (v7).
        id: Uuid,
        /// Administrator-selected disposition.
        disposition: PolicyDispositionArg,
        /// Stable administrator identity recorded in the audit trail.
        #[arg(long)]
        actor: String,
        /// Operator-safe reason recorded with the override.
        #[arg(long)]
        reason: String,
    },
    /// Show default policy rules.
    Rules,
    /// Show current policy configuration.
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum PolicyDispositionArg {
    Accept,
    Review,
    Ignore,
    Quarantine,
    Delete,
}

impl From<PolicyDispositionArg> for melodee_domain::Disposition {
    fn from(value: PolicyDispositionArg) -> Self {
        match value {
            PolicyDispositionArg::Accept => Self::Accept,
            PolicyDispositionArg::Review => Self::Review,
            PolicyDispositionArg::Ignore => Self::Ignore,
            PolicyDispositionArg::Quarantine => Self::Quarantine,
            PolicyDispositionArg::Delete => Self::Delete,
        }
    }
}

pub(crate) async fn run(command: PolicyCommands) {
    let policy_config = match PolicyConfig::from_env() {
        Ok(c) => c,
        Err(error) => {
            eprintln!("error: invalid policy configuration: {error}");
            std::process::exit(2);
        }
    };
    let pool = pool().await;
    let store = PolicyStore;
    let now = OffsetDateTime::now_utc();
    let engine = match load_policy_engine(&store, &pool, &policy_config, now).await {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("error: unable to load immutable policy rule set: {error}");
            std::process::exit(1);
        }
    };
    match command {
        PolicyCommands::Test { limit, actor } => {
            match run_policy_test(&engine, &store, &pool, limit, actor, now).await {
                Ok(report) => {
                    println!(
                        "test: evaluated={} errors={} destructive_impact={} samples={}",
                        report.summary.total_releases_evaluated,
                        report.errors,
                        report.summary.destructive_impact_count,
                        report.summary.sample_release_ids.len()
                    );
                    println!(
                        "  proposed_dispositions: {:?}",
                        report.proposed_dispositions
                    );
                    println!("  matched_rules: {:?}", report.matched_rules);
                    println!(
                        "  existing_dispositions: {:?}",
                        report.existing_dispositions
                    );
                    println!("  existing_states: {:?}", report.existing_states);
                    println!(
                        "  sample_release_ids: {:?}",
                        report.summary.sample_release_ids
                    );
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        PolicyCommands::Evaluate { id } => {
            let release_id = melodee_domain::ReleaseId::from_uuid(id);
            match run_policy_evaluate(&engine, &store, &pool, release_id, now).await {
                Ok(output) => {
                    println!(
                        "evaluate: release_id={} disposition={} potential_destructive={} confidence={} existing_disposition={} rules={:?} evidence={}",
                        id,
                        output.decision.disposition.as_str(),
                        output.decision.disposition.is_destructive(),
                        output.classification.confidence.basis_points(),
                        output
                            .existing_disposition
                            .map_or("none", melodee_domain::Disposition::as_str),
                        output.decision.matched_rule_ids,
                        output.decision.evidence_summary,
                    );
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        PolicyCommands::Inspect { id } => {
            let release_id = melodee_domain::ReleaseId::from_uuid(id);
            match run_policy_inspect(&store, &pool, release_id).await {
                Ok(()) => {}
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        PolicyCommands::Decide {
            id,
            disposition,
            actor,
            reason,
        } => {
            let release_id = melodee_domain::ReleaseId::from_uuid(id);
            match run_policy_decide(
                &store,
                &pool,
                release_id,
                disposition.into(),
                actor,
                reason,
                now,
            )
            .await
            {
                Ok(decision) => {
                    println!(
                        "decide: release_id={} disposition={} completion_state={} actor={:?} reason={}",
                        id,
                        decision.disposition.as_str(),
                        decision.completion_state.as_str(),
                        decision.actor,
                        decision.override_reason.as_deref().unwrap_or("none"),
                    );
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    std::process::exit(1);
                }
            }
        }
        PolicyCommands::Rules => {
            println!("rule_set_id: {}", engine.rule_set_id());
            println!("rule_set_version: {}", engine.rule_set_version());
            println!("rule_set_name: {}", engine.rule_set().name);
            for rule in &engine.rule_set().rules {
                println!(
                    "  {} type={} disposition={} confidence_threshold={:?}",
                    rule.id,
                    rule.rule_type.as_str(),
                    rule.disposition.as_str(),
                    rule.confidence_threshold.map(|value| value.basis_points()),
                );
            }
        }
        PolicyCommands::Config => {
            println!("policy config:");
            println!("  batch_size: {}", policy_config.batch_size);
            println!("  concurrency: {}", policy_config.concurrency);
            println!("  rule_set_version: {}", policy_config.rule_set_version);
            println!(
                "  destructive_confidence_bps: {}",
                policy_config.destructive_confidence_bps
            );
            println!(
                "  redundant_greatest_hits_disposition: {}",
                policy_config.redundant_greatest_hits_disposition.as_str()
            );
            println!(
                "  novel_greatest_hits_disposition: {}",
                policy_config.novel_greatest_hits_disposition.as_str()
            );
            println!(
                "  uncertain_greatest_hits_disposition: {}",
                policy_config.uncertain_greatest_hits_disposition.as_str()
            );
        }
    }
}

#[derive(Debug)]
struct PolicyTestReport {
    summary: melodee_domain::PolicyTestSummary,
    proposed_dispositions: BTreeMap<String, u32>,
    matched_rules: BTreeMap<String, u32>,
    existing_dispositions: BTreeMap<String, u32>,
    existing_states: BTreeMap<String, u32>,
    errors: u32,
}

#[derive(Debug)]
pub(crate) struct PolicyTestEvaluationError {
    pub(crate) release_id: melodee_domain::ReleaseId,
    source: melodee_policy::PolicyEngineError,
}

impl std::fmt::Display for PolicyTestEvaluationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "policy test evaluation failed for Release {}: {}",
            self.release_id, self.source
        )
    }
}

impl std::error::Error for PolicyTestEvaluationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

async fn load_policy_engine(
    store: &PolicyStore,
    pool: &PgPool,
    config: &PolicyConfig,
    now: OffsetDateTime,
) -> Result<PolicyEngine, Box<dyn std::error::Error>> {
    let proposed = default_rule_set(config)?;
    let stored = store.ensure_rule_set(pool, &proposed, now).await?;
    Ok(PolicyEngine::with_rule_set(config.clone(), stored)?)
}

async fn run_policy_test(
    engine: &PolicyEngine,
    store: &PolicyStore,
    pool: &PgPool,
    limit: u32,
    actor: String,
    now: OffsetDateTime,
) -> Result<PolicyTestReport, Box<dyn std::error::Error>> {
    let contexts = store.load_test_contexts(pool, limit).await?;
    let mut proposed_dispositions = BTreeMap::new();
    let mut matched_rules = BTreeMap::new();
    let mut existing_dispositions = BTreeMap::new();
    let mut existing_states = BTreeMap::new();
    let mut samples = Vec::new();
    let mut destructive_impact = 0u32;
    let mut evaluated = 0u32;

    for context in &contexts {
        let evaluation = require_policy_test_evaluation(
            context.claim.release_id,
            evaluate_catalog_context(engine, context, now),
        )?;
        evaluated = evaluated.saturating_add(1);
        increment(
            &mut proposed_dispositions,
            evaluation.decision.disposition.as_str(),
        );
        if evaluation.decision.disposition.is_destructive() {
            destructive_impact = destructive_impact.saturating_add(1);
        }
        for matched in &evaluation.matched_rules {
            increment(&mut matched_rules, &matched.rule_id);
        }
        increment(
            &mut existing_dispositions,
            context
                .existing_disposition
                .map_or("none", melodee_domain::Disposition::as_str),
        );
        increment(&mut existing_states, context.claim.item.state.as_str());
        if samples.len() < melodee_persistence::MAX_POLICY_TEST_SAMPLES {
            samples.push(context.claim.release_id);
        }
    }

    let disposition_counts = melodee_domain::Disposition::ALL
        .into_iter()
        .filter_map(|disposition| {
            proposed_dispositions
                .get(disposition.as_str())
                .copied()
                .filter(|count| *count > 0)
                .map(|count| (disposition, count))
        })
        .collect();
    let matched_rule_counts = matched_rules
        .iter()
        .map(|(rule, count)| (rule.clone(), *count))
        .collect();
    let mut existing_disposition_counts = existing_dispositions
        .get("none")
        .copied()
        .filter(|count| *count > 0)
        .map(|count| vec![(None, count)])
        .unwrap_or_default();
    existing_disposition_counts.extend(melodee_domain::Disposition::ALL.into_iter().filter_map(
        |disposition| {
            existing_dispositions
                .get(disposition.as_str())
                .copied()
                .filter(|count| *count > 0)
                .map(|count| (Some(disposition), count))
        },
    ));
    let summary = melodee_domain::PolicyTestSummary::new(melodee_domain::PolicyTestSummary {
        test_run_id: melodee_domain::PolicyTestRunId::new(),
        rule_set_id: engine.rule_set_id(),
        rule_set_version: engine.rule_set_version(),
        total_releases_evaluated: evaluated,
        disposition_counts,
        matched_rule_counts,
        existing_disposition_counts,
        destructive_impact_count: destructive_impact,
        sample_release_ids: samples,
        ran_by: melodee_domain::PolicyActor::Administrator { actor },
        ran_at: now,
    })?;
    store.store_test_run(pool, &summary).await?;

    Ok(PolicyTestReport {
        summary,
        proposed_dispositions,
        matched_rules,
        existing_dispositions,
        existing_states,
        errors: 0,
    })
}

pub(crate) fn require_policy_test_evaluation(
    release_id: melodee_domain::ReleaseId,
    result: Result<PolicyEvaluation, melodee_policy::PolicyEngineError>,
) -> Result<PolicyEvaluation, PolicyTestEvaluationError> {
    result.map_err(|source| PolicyTestEvaluationError { release_id, source })
}

async fn run_policy_evaluate(
    engine: &PolicyEngine,
    store: &PolicyStore,
    pool: &PgPool,
    release_id: melodee_domain::ReleaseId,
    now: OffsetDateTime,
) -> Result<PolicyEvaluation, Box<dyn std::error::Error>> {
    let context = store
        .load_snapshot(pool, release_id)
        .await?
        .ok_or_else(|| operator_error(format!("Release {release_id} is not cataloged")))?;
    Ok(evaluate_catalog_context(engine, &context, now)?)
}

async fn run_policy_inspect(
    store: &PolicyStore,
    pool: &PgPool,
    release_id: melodee_domain::ReleaseId,
) -> Result<(), Box<dyn std::error::Error>> {
    let (decision, history) = store.inspect_decision(pool, release_id).await?;
    let decision = decision.ok_or_else(|| {
        operator_error(format!(
            "Release {release_id} has no durable policy decision"
        ))
    })?;
    println!("release_id:          {}", decision.release_id);
    println!("work_item_id:        {}", decision.work_item_id);
    println!("decision_id:         {}", decision.id);
    println!(
        "rule_set:            {}@{}",
        decision.rule_set_id, decision.rule_set_version
    );
    println!("disposition:         {}", decision.disposition.as_str());
    println!(
        "completion_state:    {}",
        decision.completion_state.as_str()
    );
    println!("matched_rules:       {:?}", decision.matched_rule_ids);
    println!("source_fingerprint:  {}", decision.source_fingerprint);
    println!("actor:                {:?}", decision.actor);
    println!(
        "override_reason:      {}",
        decision.override_reason.as_deref().unwrap_or("none")
    );
    println!("evidence:             {}", decision.evidence_summary);
    println!("decided_at:           {}", decision.decided_at);
    println!("completed_at:         {:?}", decision.completed_at);
    println!("audit:");
    for entry in history {
        println!(
            "  {} event={} disposition={} completion={} actor={:?} reason={}",
            entry.created_at,
            entry.event,
            entry.decision.disposition.as_str(),
            entry.decision.completion_state.as_str(),
            entry.decision.actor,
            entry.decision.override_reason.as_deref().unwrap_or("none"),
        );
    }
    Ok(())
}

async fn run_policy_decide(
    store: &PolicyStore,
    pool: &PgPool,
    release_id: melodee_domain::ReleaseId,
    disposition: melodee_domain::Disposition,
    actor: String,
    reason: String,
    now: OffsetDateTime,
) -> Result<melodee_persistence::StoredPolicyDecision, Box<dyn std::error::Error>> {
    let context = store
        .load_snapshot(pool, release_id)
        .await?
        .ok_or_else(|| operator_error(format!("Release {release_id} is not cataloged")))?;
    let decision = context.current_decision.ok_or_else(|| {
        operator_error(format!("Release {release_id} has no current decision to override; allow the policy worker to evaluate it first"))
    })?;
    let result = store
        .apply_admin_override(
            pool,
            &PolicyAdminOverrideRequest {
                work_item_id: context.claim.item.id,
                decision_id: decision.id,
                chosen_disposition: disposition,
                actor,
                reason,
                source_fingerprint: context.claim.source_fingerprint,
                now,
            },
        )
        .await?;
    Ok(result.decision)
}

fn increment(counts: &mut BTreeMap<String, u32>, key: &str) {
    let count = counts.entry(key.to_owned()).or_insert(0);
    *count = count.saturating_add(1);
}

fn operator_error(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message)
}
