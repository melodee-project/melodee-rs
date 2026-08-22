import { describe, expect, it } from "vitest";

import type { PolicyRule } from "../../api/types";
import {
  allowedDispositions,
  conditionsToPredicate,
  draftToInput,
  isProtectedRule,
  predicateToConditions,
  ruleToDraft,
  validateDraft,
  type RuleDraft,
} from "./policies";

/**
 * Model-helper tests : the editor must only build predicates and
 * payloads the real engine accepts (PolicyRule::validate, RulePredicate).
 */

function makeRule(overrides: Partial<PolicyRule> = {}): PolicyRule {
  return {
    id: "POL-TEST",
    ruleType: "trait_disposition",
    disposition: "review",
    confidenceThreshold: null,
    predicate: {},
    priority: 3,
    enabled: true,
    matchCount: 0,
    ...overrides,
  };
}

function makeDraft(overrides: Partial<RuleDraft> = {}): RuleDraft {
  return {
    id: "POL-NEW",
    ruleType: "trait_disposition",
    disposition: "review",
    confidenceThreshold: null,
    conditions: [{ field: "release_trait", value: "greatest_hits" }],
    ...overrides,
  };
}

describe("conditionsToPredicate", () => {
  it("emits only the engine predicate schema, deduplicated", () => {
    expect(
      conditionsToPredicate([
        { field: "release_trait", value: "greatest_hits" },
        { field: "release_trait", value: "live" },
        { field: "release_trait", value: "greatest_hits" },
        { field: "existing_disposition", value: "review" },
        { field: "existing_disposition", value: "review" },
      ]),
    ).toEqual({
      required_traits: ["greatest_hits", "live"],
      existing_dispositions: ["review"],
    });
  });

  it("omits existing_dispositions entirely when no row restricts it", () => {
    expect(
      conditionsToPredicate([{ field: "release_trait", value: "live" }]),
    ).toEqual({
      required_traits: ["live"],
    });
    expect(conditionsToPredicate([])).toEqual({});
  });

  it("drops values outside the real enums", () => {
    expect(
      conditionsToPredicate([
        { field: "release_trait", value: "not_a_trait" },
        { field: "existing_disposition", value: "explode" },
      ]),
    ).toEqual({});
  });
});

describe("predicateToConditions", () => {
  it("round-trips through conditionsToPredicate", () => {
    const predicate = {
      required_traits: ["greatest_hits" as const, "compilation" as const],
      existing_dispositions: ["ignore" as const],
    };
    expect(conditionsToPredicate(predicateToConditions(predicate))).toEqual(
      predicate,
    );
  });
});

describe("draftToInput", () => {
  it("builds the exact RuleInput payload", () => {
    expect(
      draftToInput(
        makeDraft({
          disposition: "quarantine",
          confidenceThreshold: 9000,
          conditions: [
            { field: "release_trait", value: "live" },
            { field: "existing_disposition", value: "review" },
          ],
        }),
      ),
    ).toEqual({
      id: "POL-NEW",
      ruleType: "trait_disposition",
      disposition: "quarantine",
      confidenceThreshold: 9000,
      predicate: {
        required_traits: ["live"],
        existing_dispositions: ["review"],
      },
    });
  });

  it("forces an empty predicate for safety rule types", () => {
    expect(
      draftToInput(
        makeDraft({
          ruleType: "corrupt_media",
          disposition: "quarantine",
          conditions: [{ field: "release_trait", value: "live" }],
        }),
      ).predicate,
    ).toEqual({});
  });
});

describe("validateDraft", () => {
  it("accepts a valid trait rule", () => {
    expect(validateDraft(makeDraft())).toEqual([]);
  });

  it("requires a rule id", () => {
    expect(validateDraft(makeDraft({ id: "  " }))).toContain("id");
  });

  it("rejects dispositions outside the per-type safe set", () => {
    expect(
      validateDraft(
        makeDraft({ ruleType: "greatest_hits_novelty", disposition: "delete" }),
      ),
    ).toContain("disposition");
  });

  it("requires a confidence threshold for the delete disposition", () => {
    expect(
      validateDraft(
        makeDraft({
          ruleType: "greatest_hits_redundancy",
          disposition: "delete",
        }),
      ),
    ).toContain("confidence");
    expect(
      validateDraft(
        makeDraft({
          ruleType: "greatest_hits_redundancy",
          disposition: "delete",
          confidenceThreshold: 9500,
        }),
      ),
    ).not.toContain("confidence");
  });

  it("rejects out-of-range confidence values", () => {
    expect(validateDraft(makeDraft({ confidenceThreshold: 10001 }))).toContain(
      "confidence",
    );
    expect(validateDraft(makeDraft({ confidenceThreshold: -1 }))).toContain(
      "confidence",
    );
  });

  it("requires at least one trait condition for trait_disposition rules", () => {
    expect(validateDraft(makeDraft({ conditions: [] }))).toContain("traits");
  });
});

describe("allowedDispositions", () => {
  it("mirrors PolicyRule::validate per type", () => {
    expect(allowedDispositions("greatest_hits_redundancy")).toEqual([
      "ignore",
      "delete",
    ]);
    expect(allowedDispositions("greatest_hits_novelty")).toEqual([
      "accept",
      "review",
    ]);
    expect(allowedDispositions("greatest_hits_uncertain")).toEqual([
      "review",
      "quarantine",
    ]);
    expect(allowedDispositions("corrupt_media")).toEqual(["quarantine"]);
    expect(allowedDispositions("unknown_classification")).toEqual(["review"]);
    expect(allowedDispositions("trait_disposition")).not.toContain("delete");
    expect(allowedDispositions("administrator_override")).toContain("delete");
  });
});

describe("isProtectedRule", () => {
  it("protects the two anchored safety rules by position and type", () => {
    expect(
      isProtectedRule(
        makeRule({ priority: 0, ruleType: "administrator_override" }),
      ),
    ).toBe(true);
    expect(
      isProtectedRule(
        makeRule({
          priority: 1,
          ruleType: "corrupt_media",
          disposition: "quarantine",
        }),
      ),
    ).toBe(true);
    expect(isProtectedRule(makeRule({ priority: 2 }))).toBe(false);
    expect(
      isProtectedRule(
        makeRule({
          priority: 0,
          ruleType: "administrator_override",
          enabled: false,
        }),
      ),
    ).toBe(false);
  });
});

describe("ruleToDraft", () => {
  it("maps a stored rule into editable state", () => {
    const rule = makeRule({
      confidenceThreshold: 9000,
      predicate: {
        required_traits: ["live"],
        existing_dispositions: ["ignore"],
      },
    });
    expect(ruleToDraft(rule)).toEqual({
      id: "POL-TEST",
      ruleType: "trait_disposition",
      disposition: "review",
      confidenceThreshold: 9000,
      conditions: [
        { field: "release_trait", value: "live" },
        { field: "existing_disposition", value: "ignore" },
      ],
    });
  });
});
