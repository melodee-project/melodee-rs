import type {
  PolicyDisposition,
  PolicyPredicate,
  PolicyRule,
  PolicyRuleInput,
  PolicyRuleType,
  ReleaseTraitValue,
} from "../../api/types";

/**
 * Client-side mirror of the policy rule model . Every constant here
 * mirrors the melodee-domain/melodee-policy source of truth so the editor
 * only ever builds predicates and disposition combinations the real engine
 * accepts:
 *
 * - Rule types: PolicyRuleType::as_str (crates/melodee-domain/src/policy.rs)
 * - Dispositions: Disposition::ALL (crates/melodee-domain/src/ingestion.rs)
 * - Traits: ReleaseTrait::ALL (crates/melodee-domain/src/release.rs)
 * - Predicate schema: RulePredicate in crates/melodee-policy/src/engine.rs
 * (`required_traits`, `existing_dispositions`; unknown fields denied)
 * - Per-type disposition safety: PolicyRule::validate
 */

export const RULE_TYPES: readonly PolicyRuleType[] = [
  "administrator_override",
  "corrupt_media",
  "greatest_hits_uncertain",
  "greatest_hits_novelty",
  "greatest_hits_redundancy",
  "unknown_classification",
  "trait_disposition",
];

export const DISPOSITIONS: readonly PolicyDisposition[] = [
  "accept",
  "review",
  "ignore",
  "quarantine",
  "delete",
];

export const RELEASE_TRAITS: readonly ReleaseTraitValue[] = [
  "studio",
  "ep",
  "single",
  "live",
  "compilation",
  "greatest_hits",
  "collection",
  "box_set",
  "bootleg",
  "demo",
  "promo",
  "broadcast",
  "radio_show",
  "dj_mix",
  "remix",
  "tribute",
  "soundtrack",
  "cast_recording",
  "spoken_word",
  "comedy",
  "audiobook",
  "reissue",
  "remaster",
  "other",
  "unknown",
];

/** Dispositions each rule type may carry (PolicyRule::validate). */
const ALLOWED_DISPOSITIONS: Record<
  PolicyRuleType,
  readonly PolicyDisposition[]
> = {
  administrator_override: DISPOSITIONS,
  corrupt_media: ["quarantine"],
  greatest_hits_uncertain: ["review", "quarantine"],
  greatest_hits_novelty: ["accept", "review"],
  greatest_hits_redundancy: ["ignore", "delete"],
  unknown_classification: ["review"],
  // TraitDisposition: anything non-destructive.
  trait_disposition: ["accept", "review", "ignore", "quarantine"],
};

export function allowedDispositions(
  ruleType: PolicyRuleType,
): readonly PolicyDisposition[] {
  return ALLOWED_DISPOSITIONS[ruleType];
}

/** Rule types whose engine invariant requires an empty predicate. */
export function predicateLocked(ruleType: PolicyRuleType): boolean {
  return ruleType === "administrator_override" || ruleType === "corrupt_media";
}

/**
 * Safety rules anchor every valid rule set (engine invariants): the
 * administrator override at priority 0 and corrupt-media quarantine at
 * priority 1 can be neither deleted nor disabled (409 `protected_rule`).
 * The DTO exposes no flag, so protection is derived from position + type,
 * matching `protected_position` in apps/melodee-server/src/admin_policies.rs.
 */
export function isProtectedRule(rule: PolicyRule): boolean {
  if (!rule.enabled) {
    return false;
  }
  return (
    (rule.priority === 0 && rule.ruleType === "administrator_override") ||
    (rule.priority === 1 && rule.ruleType === "corrupt_media")
  );
}

/** One row of the structured conditions builder. */
export interface PredicateCondition {
  field: "release_trait" | "existing_disposition";
  value: string;
}

/**
 * Serializes condition rows into the engine predicate schema. Traits and
 * dispositions are deduplicated; `existing_dispositions` is omitted entirely
 * when no such row exists (the engine treats an absent key as "no
 * restriction", and the deserializer denies unknown/extra keys).
 */
export function conditionsToPredicate(
  conditions: PredicateCondition[],
): PolicyPredicate {
  const traits = new Set<ReleaseTraitValue>();
  const dispositions = new Set<PolicyDisposition>();
  for (const condition of conditions) {
    if (condition.field === "release_trait" && isTrait(condition.value)) {
      traits.add(condition.value);
    }
    if (
      condition.field === "existing_disposition" &&
      isDisposition(condition.value)
    ) {
      dispositions.add(condition.value);
    }
  }
  const predicate: PolicyPredicate = {};
  if (traits.size > 0) {
    predicate.required_traits = [...traits];
  }
  if (dispositions.size > 0) {
    predicate.existing_dispositions = [...dispositions];
  }
  return predicate;
}

/** Reads a stored predicate into condition rows (unknown keys ignored). */
export function predicateToConditions(
  predicate: PolicyPredicate,
): PredicateCondition[] {
  const conditions: PredicateCondition[] = [];
  for (const trait of predicate.required_traits ?? []) {
    if (isTrait(trait)) {
      conditions.push({ field: "release_trait", value: trait });
    }
  }
  for (const disposition of predicate.existing_dispositions ?? []) {
    if (isDisposition(disposition)) {
      conditions.push({ field: "existing_disposition", value: disposition });
    }
  }
  return conditions;
}

function isTrait(value: string): value is ReleaseTraitValue {
  return (RELEASE_TRAITS as readonly string[]).includes(value);
}

function isDisposition(value: string): value is PolicyDisposition {
  return (DISPOSITIONS as readonly string[]).includes(value);
}

/** Editable editor state for one rule. */
export interface RuleDraft {
  id: string;
  ruleType: PolicyRuleType;
  disposition: PolicyDisposition;
  /** Basis points 0–10000, or null when unset. */
  confidenceThreshold: number | null;
  conditions: PredicateCondition[];
}

export function ruleToDraft(rule: PolicyRule): RuleDraft {
  return {
    id: rule.id,
    ruleType: rule.ruleType,
    disposition: rule.disposition,
    confidenceThreshold: rule.confidenceThreshold,
    conditions: predicateToConditions(rule.predicate),
  };
}

/** Builds the exact RuleInput payload accepted by the backend. */
export function draftToInput(draft: RuleDraft): PolicyRuleInput {
  return {
    id: draft.id.trim(),
    ruleType: draft.ruleType,
    disposition: draft.disposition,
    confidenceThreshold: draft.confidenceThreshold,
    predicate: predicateLocked(draft.ruleType)
      ? {}
      : conditionsToPredicate(draft.conditions),
  };
}

/** Draft validation failures, as i18n key suffixes under policies.validation.* */
export type DraftError = "id" | "disposition" | "confidence" | "traits";

export function validateDraft(draft: RuleDraft): DraftError[] {
  const errors: DraftError[] = [];
  const id = draft.id.trim();
  if (id.length === 0 || id.length > 200) {
    errors.push("id");
  }
  if (!allowedDispositions(draft.ruleType).includes(draft.disposition)) {
    errors.push("disposition");
  }
  if (draft.confidenceThreshold !== null) {
    if (
      !Number.isInteger(draft.confidenceThreshold) ||
      draft.confidenceThreshold < 0 ||
      draft.confidenceThreshold > 10000
    ) {
      errors.push("confidence");
    }
  } else if (draft.disposition === "delete") {
    // Domain invariant: delete requires a confidence floor.
    errors.push("confidence");
  }
  if (
    draft.ruleType === "trait_disposition" &&
    !draft.conditions.some((condition) => condition.field === "release_trait")
  ) {
    errors.push("traits");
  }
  return errors;
}

/** Whether the draft still equals the rule it was loaded from. */
export function draftMatchesRule(draft: RuleDraft, rule: PolicyRule): boolean {
  const input = draftToInput(draft);
  return (
    input.id === rule.id &&
    input.ruleType === rule.ruleType &&
    input.disposition === rule.disposition &&
    (input.confidenceThreshold ?? null) === rule.confidenceThreshold &&
    JSON.stringify(input.predicate) ===
      JSON.stringify(normalizePredicate(rule.predicate))
  );
}

/** Normalizes a stored predicate the way conditionsToPredicate emits it. */
function normalizePredicate(predicate: PolicyPredicate): PolicyPredicate {
  return conditionsToPredicate(predicateToConditions(predicate));
}
