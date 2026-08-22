import { Add, Renew } from "@carbon/icons-react";
import {
  ActionableNotification,
  Button,
  InlineNotification,
  Loading,
  Modal,
  Tag,
} from "@carbon/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";

import {
  createPolicyRule,
  deletePolicyRule,
  disablePolicyRule,
  enablePolicyRule,
  listPolicyRules,
  testPolicyRule,
  updatePolicyRule,
} from "../../api/admin";
import { ApiError } from "../../api/client";
import type {
  PolicyRule,
  PolicyRuleInput,
  PolicyRulePage,
  PolicyTestRequest,
  PolicyTestRun,
} from "../../api/types";
import {
  EntityTable,
  type EntityColumn,
  type EntityRow,
} from "../../components/EntityTable";
import { PageHeader } from "../../components/PageHeader";
import { StatusTag } from "../../components/StatusTag";
import { formatNumber } from "../../i18n/format";
import { dispositionKind } from "../review/review";
import { PolicyEditor } from "./PolicyEditor";

/**
 * Policies / Release Rules (PRD-POLICY-008, SPEC §19.4): the active
 * rule set as an ordered EntityTable plus the structured Policy Editor.
 * Every mutation appends a new immutable rule-set version server-side; the
 * listing refetches after each mutation so the displayed version, order,
 * and match counts always reflect durable state. Disabled rules are listed
 * from immutable history (priority null) and can be re-enabled.
 *
 * The PRD-UI-004 Matching Rules nav entry is removed, not placeholder-routed:
 * the engine has exactly one rule model (Release policy rules) and identity
 * matching is evidence-based, so a separate surface would be fake ( * decision log).
 */

interface Notice {
  kind: "success" | "error" | "info" | "warning";
  text: string;
}

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError
    ? cause
    : new ApiError(0, "unknown", String(cause), null);
}

export function PoliciesScreen() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();

  const selectedRuleId = searchParams.get("rule");

  const [pageData, setPageData] = useState<PolicyRulePage | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<PolicyRule | null>(null);
  const [deleteBusy, setDeleteBusy] = useState(false);

  const fetchRules = useCallback((signal: { cancelled: boolean }) => {
    return listPolicyRules()
      .then((result) => {
        if (!signal.cancelled) {
          setPageData(result);
          setListError(null);
          setLoading(false);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setListError(toApiError(cause));
          setLoading(false);
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchRules(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchRules]);

  const selectRule = useCallback(
    (id: string | null) => {
      const next = new URLSearchParams(searchParams);
      if (id === null) {
        next.delete("rule");
      } else {
        next.set("rule", id);
      }
      setSearchParams(next, { replace: true });
    },
    [searchParams, setSearchParams],
  );

  const selectedRule = useMemo(
    () => pageData?.items.find((item) => item.id === selectedRuleId),
    [pageData, selectedRuleId],
  );
  const editorMode: "create" | "edit" | null =
    selectedRuleId === "new" ? "create" : selectedRule ? "edit" : null;

  const reload = useCallback(() => {
    selectRule(null);
    void fetchRules({ cancelled: false });
  }, [fetchRules, selectRule]);

  /** Create or replace; resolves after the listing has been refetched. */
  const saveRule = async (input: PolicyRuleInput): Promise<void> => {
    if (editorMode === null) {
      return;
    }
    setBusy(true);
    try {
      const mutation =
        editorMode === "create"
          ? await createPolicyRule(input)
          : await updatePolicyRule(input.id, input);
      await fetchRules({ cancelled: false });
      setNotice({
        kind: "success",
        text: t("policies.editor.saved", { version: mutation.ruleSet.version }),
      });
      selectRule(mutation.rule.id);
    } finally {
      setBusy(false);
    }
  };

  const toggleEnabled = async (
    rule: PolicyRule,
    enable: boolean,
  ): Promise<void> => {
    setBusy(true);
    try {
      if (enable) {
        await enablePolicyRule(rule.id);
      } else {
        await disablePolicyRule(rule.id);
      }
      await fetchRules({ cancelled: false });
      setNotice({
        kind: "success",
        text: enable
          ? t("policies.editor.enableSuccess")
          : t("policies.editor.disableSuccess"),
      });
    } finally {
      setBusy(false);
    }
  };

  const confirmDelete = () => {
    if (!deleteTarget) {
      return;
    }
    setDeleteBusy(true);
    deletePolicyRule(deleteTarget.id)
      .then(async () => {
        await fetchRules({ cancelled: false });
        setNotice({
          kind: "success",
          text: t("policies.editor.deleteSuccess"),
        });
        selectRule(null);
      })
      .catch((cause: unknown) => {
        const error = toApiError(cause);
        if (error.status === 409 && error.code === "rule_set_conflict") {
          setNotice({ kind: "warning", text: t("policies.editor.conflict") });
        } else if (error.status === 409 && error.code === "protected_rule") {
          setNotice({
            kind: "warning",
            text: t("policies.editor.protectedError"),
          });
        } else {
          setNotice({
            kind: "error",
            text: t("policies.editor.deleteError", { message: error.message }),
          });
        }
      })
      .finally(() => {
        setDeleteBusy(false);
        setDeleteTarget(null);
      });
  };

  const runTest = (request: PolicyTestRequest): Promise<PolicyTestRun> =>
    testPolicyRule(request);

  const columns: EntityColumn[] = useMemo(
    () => [
      { key: "policy", header: t("policies.table.column.policy") },
      { key: "type", header: t("policies.table.column.type") },
      { key: "disposition", header: t("policies.table.column.disposition") },
      { key: "priority", header: t("policies.table.column.priority") },
      { key: "matches", header: t("policies.table.column.matches") },
      { key: "status", header: t("policies.table.column.status") },
    ],
    [t],
  );

  const rows: EntityRow[] = useMemo(
    () =>
      (pageData?.items ?? []).map((rule) => ({
        id: rule.id,
        cells: {
          policy: <span className="melodee-work-item__title">{rule.id}</span>,
          type: <Tag type="outline">{t(`policies.type.${rule.ruleType}`)}</Tag>,
          disposition: (
            <StatusTag
              kind={dispositionKind(rule.disposition)}
              label={t(`review.disposition.${rule.disposition}`)}
            />
          ),
          priority:
            rule.priority === null
              ? t("common.none")
              : formatNumber(rule.priority),
          matches: formatNumber(rule.matchCount),
          status: (
            <StatusTag
              kind={rule.enabled ? "success" : "neutral"}
              label={rule.enabled ? t("status.enabled") : t("status.disabled")}
            />
          ),
        },
      })),
    [pageData, t],
  );

  const toolbar = (
    <Button
      kind="ghost"
      size="sm"
      renderIcon={Renew}
      aria-label={t("actions.refresh")}
      onClick={() => {
        setLoading(pageData === null);
        void fetchRules({ cancelled: false });
      }}
    >
      {t("actions.refresh")}
    </Button>
  );

  return (
    <div className="melodee-queue-main">
      <PageHeader
        title={t("policies.releaseRulesTitle")}
        subtitle={t("policies.subtitle")}
        actions={
          <Button
            kind="primary"
            size="md"
            renderIcon={Add}
            onClick={() => selectRule("new")}
          >
            {t("policies.create")}
          </Button>
        }
      />
      {pageData ? (
        <p className="melodee-panel__subtitle" role="note">
          {t("policies.ruleSet.summary", {
            name: pageData.ruleSet.name,
            version: formatNumber(pageData.ruleSet.version),
          })}{" "}
          {t("policies.ruleSet.orderNote")}
        </p>
      ) : null}
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.text}
          onCloseButtonClick={() => setNotice(null)}
        />
      ) : null}
      {listError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t("policies.loadErrorTitle")}
          subtitle={listError.message}
          hideCloseButton
          actionButtonLabel={t("actions.retry")}
          onActionButtonClick={() => void fetchRules({ cancelled: false })}
        />
      ) : null}
      <div className="melodee-system-columns">
        <div className="melodee-system-main">
          {loading && pageData === null ? (
            <Loading description={t("common.loading")} withOverlay={false} />
          ) : (
            <EntityTable
              caption={t("policies.table.caption")}
              columns={columns}
              rows={rows}
              totalRows={pageData?.total ?? 0}
              toolbar={toolbar}
              compact
              emptyTitle={t("policies.table.emptyTitle")}
              emptyDescription={t("policies.table.emptyDescription")}
              activeRowId={selectedRule?.id ?? null}
              onRowActivate={(id) => selectRule(id)}
            />
          )}
        </div>
        <div className="melodee-system-rail">
          {editorMode === null ? (
            <section
              className="melodee-panel"
              aria-labelledby="policy-editor-hint"
            >
              <h2 id="policy-editor-hint" className="melodee-panel__title">
                {t("policies.editor.titleEdit")}
              </h2>
              <p className="melodee-panel__subtitle">
                {t("policies.editor.selectHint")}
              </p>
            </section>
          ) : (
            <PolicyEditor
              // Remount on selection or version change so form state never
              // drifts from durable rule state.
              key={`${selectedRuleId}-${pageData?.ruleSet.version ?? "none"}`}
              mode={editorMode}
              rule={selectedRule}
              busy={busy}
              onSave={saveRule}
              onCancel={() => selectRule(null)}
              onToggleEnabled={toggleEnabled}
              onDelete={(rule) => setDeleteTarget(rule)}
              onReload={reload}
              onTest={runTest}
            />
          )}
        </div>
      </div>

      <Modal
        open={deleteTarget !== null}
        danger
        modalHeading={t("policies.editor.deleteConfirmTitle", {
          id: deleteTarget?.id ?? "",
        })}
        primaryButtonText={t("actions.delete")}
        secondaryButtonText={t("actions.cancel")}
        primaryButtonDisabled={deleteBusy}
        onRequestClose={() => setDeleteTarget(null)}
        onRequestSubmit={confirmDelete}
      >
        <p>{t("policies.editor.deleteConfirmBody")}</p>
      </Modal>
    </div>
  );
}
