import { Add } from "@carbon/icons-react";
import {
  Accordion,
  AccordionItem,
  ActionableNotification,
  Button,
  Heading,
  InlineNotification,
  Loading,
  Modal,
  Select,
  SelectItem,
  TextArea,
  TextInput,
} from "@carbon/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";

import {
  getObservationDetail,
  listIdentityDecisions,
  listUnresolvedObservations,
  mergeArtists,
  neverMatchCandidate,
  resolveObservation,
  splitArtist,
} from "../../api/admin";
import { ApiError } from "../../api/client";
import type {
  IdentityDecisionEntry,
  IdentityResolution,
  ObservationDetail,
  Page,
  SortParam,
  SplitAssignmentInput,
  SplitAssignmentKind,
  UnresolvedObservation,
} from "../../api/types";
import { AuditTimeline } from "../../components/AuditTimeline";
import {
  DecisionPanel,
  type DecisionChoice,
} from "../../components/DecisionPanel";
import { EmptyState } from "../../components/EmptyState";
import { EntityAvatar } from "../../components/EntityAvatar";
import {
  EntityTable,
  type EntityColumn,
  type EntityRow,
} from "../../components/EntityTable";
import { EvidenceItem } from "../../components/EvidenceItem";
import { EvidenceSummary } from "../../components/EvidenceSummary";
import { PageHeader } from "../../components/PageHeader";
import { StatusTag } from "../../components/StatusTag";
import {
  formatDateTime,
  formatNumber,
  formatRelativeTime,
} from "../../i18n/format";
import {
  armFlashNotice,
  artistStatusKind,
  confidencePercent,
  evidenceCounts,
  IDENTITY_ACTIONS,
  isPublicId,
  nextSelection,
  observationContext,
  selectedCandidate,
  SPLIT_ASSIGNMENT_KINDS,
  takeFlashNotice,
  toAuditEvent,
  toEvidence,
  type IdentityAction,
} from "./identity";

/**
 * Artist Identity Review workspace (PRD-API-ADMIN-002, SPEC §20):
 * the three-pane review pattern (UI-SPEC §7) — unresolved-observations
 * queue, incoming observation with candidate matches and persisted evidence,
 * and the ordered decision rail. A string is never an Artist identity:
 * every mutation binds an observation to a Melodee-owned Artist id (or
 * records a durable do-not-match assertion) with an audited reason.
 *
 * All mutations require a non-blank reason; 400/404/409 envelope errors
 * surface as localized inline notifications. On success the queue refetches
 * and the next observation is selected without losing paging position.
 * Below the panes: the recent identity decisions feed and the Artist
 * merge/split operations (merge is destructive-class with typed
 * confirmation; split honestly takes internal link row ids until the picker
 * UI lands).
 */

/** Sort whitelist shared with the backend (ObservedArtistSort::parse). */
const SORT_KEYS = new Set(["createdAt", "displayValue"]);

const FEED_LIMIT = 20;

interface Notice {
  kind: "success" | "error" | "info" | "warning";
  text: string;
}

function parseSort(raw: string | null): SortParam | undefined {
  if (!raw) {
    return undefined;
  }
  const [key, direction] = raw.split(":");
  if (
    !key ||
    (direction !== "asc" && direction !== "desc") ||
    !SORT_KEYS.has(key)
  ) {
    return undefined;
  }
  return `${key}:${direction}` as SortParam;
}

function parsePage(raw: string | null): number {
  return Math.max(1, Number.parseInt(raw ?? "1", 10) || 1);
}

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError
    ? cause
    : new ApiError(0, "unknown", String(cause), null);
}

/** Reason-required confirmation for confirm / create-new / leave-provisional. */
function ResolveDialog({
  action,
  busy,
  candidateName,
  onClose,
  onConfirm,
}: {
  action: Exclude<IdentityAction, "never-match"> | null;
  busy: boolean;
  candidateName: string | null;
  onClose: () => void;
  onConfirm: (reason: string) => void;
}) {
  const { t } = useTranslation();
  const [reason, setReason] = useState("");
  if (!action) {
    return null;
  }
  const reasonValid = reason.trim().length > 0;
  return (
    <Modal
      open
      modalHeading={t("identity.decision.confirmHeading", {
        action: t(`identity.decision.action.${action}`),
        candidate: candidateName ?? "",
      })}
      primaryButtonText={t("identity.decision.apply")}
      secondaryButtonText={t("actions.cancel")}
      primaryButtonDisabled={!reasonValid || busy}
      onRequestClose={() => {
        setReason("");
        onClose();
      }}
      onRequestSubmit={() => {
        const submitted = reason.trim();
        setReason("");
        onConfirm(submitted);
      }}
    >
      <TextArea
        id="identity-decision-reason"
        labelText={t("identity.decision.reasonLabel")}
        value={reason}
        invalid={!reasonValid && reason.length > 0}
        invalidText={t("identity.decision.reasonRequired")}
        onChange={(event) => setReason(event.target.value)}
      />
    </Modal>
  );
}

/** Destructive do-not-match confirmation; danger-styled, reason required. */
function NeverMatchDialog({
  open,
  busy,
  candidateName,
  onClose,
  onConfirm,
}: {
  open: boolean;
  busy: boolean;
  candidateName: string;
  onClose: () => void;
  onConfirm: (reason: string) => void;
}) {
  const { t } = useTranslation();
  const [reason, setReason] = useState("");
  const reasonValid = reason.trim().length > 0;
  return (
    <Modal
      open={open}
      danger
      modalHeading={t("identity.neverMatch.title", {
        candidate: candidateName,
      })}
      primaryButtonText={t("identity.decision.action.never-match")}
      secondaryButtonText={t("actions.cancel")}
      primaryButtonDisabled={!reasonValid || busy}
      onRequestClose={() => {
        setReason("");
        onClose();
      }}
      onRequestSubmit={() => {
        const submitted = reason.trim();
        setReason("");
        onConfirm(submitted);
      }}
    >
      <p style={{ marginBlockEnd: "0.5rem" }}>
        {t("identity.neverMatch.description")}
      </p>
      <TextArea
        id="identity-never-match-reason"
        labelText={t("identity.decision.reasonLabel")}
        value={reason}
        invalid={!reasonValid && reason.length > 0}
        invalidText={t("identity.decision.reasonRequired")}
        onChange={(event) => setReason(event.target.value)}
      />
    </Modal>
  );
}

/** Selectable candidate card (radio semantics inside the candidates group). */
function CandidateCard({
  detail,
  artistId,
  selected,
  onSelect,
}: {
  detail: ObservationDetail;
  artistId: string;
  selected: boolean;
  onSelect: (artistId: string) => void;
}) {
  const { t } = useTranslation();
  const candidate = detail.candidates.find(
    (entry) => entry.artistId === artistId,
  );
  if (!candidate) {
    return null;
  }
  const neverMatched = detail.negativeAssertions.some(
    (assertion) =>
      assertion.active && assertion.candidateArtistId === candidate.artistId,
  );
  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      className={`melodee-identity-candidate${
        selected ? " melodee-identity-candidate--selected" : ""
      }`}
      onClick={() => onSelect(candidate.artistId)}
    >
      <span className="melodee-identity-candidate__header">
        {/* The adjacent artist name labels the card; keep the decorative
            fallback initial out of the button's accessible name. */}
        <span aria-hidden="true">
          <EntityAvatar name={candidate.artistName} size={48} decorative />
        </span>
        <span className="melodee-identity-candidate__title">
          <strong>{candidate.artistName}</strong>
          <span className="melodee-identity-candidate__meta">
            <StatusTag
              kind={artistStatusKind(candidate.artistStatus)}
              label={t(`identity.artistStatus.${candidate.artistStatus}`, {
                defaultValue: candidate.artistStatus,
              })}
            />
            {candidate.latestLocked ? (
              <StatusTag kind="neutral" label={t("status.locked")} />
            ) : null}
            {neverMatched ? (
              <StatusTag
                kind="error"
                label={t("identity.candidates.neverMatched")}
              />
            ) : null}
          </span>
        </span>
        <span className="melodee-identity-candidate__confidence">
          <span className="melodee-review-fact__label">
            {t("identity.candidates.confidence")}
          </span>
          <span className="melodee-identity-candidate__confidence-value">
            {formatNumber(confidencePercent(candidate.latestConfidence))}%
          </span>
        </span>
      </span>
      <span className="melodee-identity-candidate__facts">
        {t("identity.candidates.decisionCount", {
          count: candidate.decisionCount,
        })}
        {" · "}
        {formatRelativeTime(candidate.latestDecidedAt)}
      </span>
      {candidate.latestReason ? (
        <span className="melodee-identity-candidate__reason">
          {candidate.latestReason}
        </span>
      ) : null}
      {candidate.evidence.length > 0 ? (
        <span className="melodee-identity-candidate__evidence">
          {candidate.evidence.map((evidence, index) => (
            <EvidenceItem
              key={`${evidence.source}-${index}`}
              evidence={toEvidence(evidence)}
            />
          ))}
        </span>
      ) : null}
    </button>
  );
}

/** Merge form: two public ids, audited reason, typed confirmation (destructive-class). */
function MergeForm({ onCompleted }: { onCompleted: () => void }) {
  const { t } = useTranslation();
  const [sourceId, setSourceId] = useState("");
  const [targetId, setTargetId] = useState("");
  const [reason, setReason] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);

  const phrase = t("identity.ops.merge.confirmPhrase");
  const valid =
    isPublicId(sourceId) &&
    isPublicId(targetId) &&
    sourceId.trim() !== targetId.trim() &&
    reason.trim().length > 0 &&
    confirmation.trim() === phrase;

  const submit = () => {
    setBusy(true);
    setNotice(null);
    mergeArtists({
      sourceArtistId: sourceId.trim(),
      targetArtistId: targetId.trim(),
      reason: reason.trim(),
    })
      .then(() => {
        setNotice({ kind: "success", text: t("identity.ops.merge.success") });
        setSourceId("");
        setTargetId("");
        setReason("");
        setConfirmation("");
        onCompleted();
      })
      .catch((cause: unknown) => {
        const error = toApiError(cause);
        setNotice({
          kind: "error",
          text: t("identity.ops.merge.error", { message: error.message }),
        });
      })
      .finally(() => setBusy(false));
  };

  return (
    <section aria-label={t("identity.ops.merge.title")}>
      <p style={{ color: "var(--cds-text-secondary)", marginBlockEnd: "1rem" }}>
        {t("identity.ops.merge.description")}
      </p>
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          hideCloseButton
          title={notice.text}
          style={{ marginBlockEnd: "0.5rem" }}
        />
      ) : null}
      <TextInput
        id="identity-merge-source"
        labelText={t("identity.ops.merge.sourceLabel")}
        value={sourceId}
        invalid={sourceId.length > 0 && !isPublicId(sourceId)}
        invalidText={t("identity.ops.invalidPublicId")}
        onChange={(event) => setSourceId(event.target.value)}
      />
      <TextInput
        id="identity-merge-target"
        labelText={t("identity.ops.merge.targetLabel")}
        value={targetId}
        invalid={targetId.length > 0 && !isPublicId(targetId)}
        invalidText={t("identity.ops.invalidPublicId")}
        onChange={(event) => setTargetId(event.target.value)}
        style={{ marginBlockStart: "0.5rem" }}
      />
      <TextArea
        id="identity-merge-reason"
        labelText={t("identity.decision.reasonLabel")}
        value={reason}
        onChange={(event) => setReason(event.target.value)}
        style={{ marginBlockStart: "0.5rem" }}
      />
      <TextInput
        id="identity-merge-confirm"
        labelText={t("destructive.typeToConfirm", { phrase })}
        value={confirmation}
        onChange={(event) => setConfirmation(event.target.value)}
        style={{ marginBlockStart: "0.5rem" }}
      />
      <Button
        kind="danger"
        size="sm"
        disabled={!valid || busy}
        onClick={submit}
        style={{ marginBlockStart: "1rem" }}
      >
        {t("identity.ops.merge.action")}
      </Button>
    </section>
  );
}

/**
 * Split form matching the backend SplitRequest DTO. Assignment ids are the
 * internal durable row identities of the selected links (per the API docs
 * caveat); the helper text says so until the picker UI lands.
 */
function SplitForm({ onCompleted }: { onCompleted: () => void }) {
  const { t } = useTranslation();
  const [sourceId, setSourceId] = useState("");
  const [canonicalName, setCanonicalName] = useState("");
  const [sortName, setSortName] = useState("");
  const [disambiguation, setDisambiguation] = useState("");
  const [reason, setReason] = useState("");
  const [assignments, setAssignments] = useState<SplitAssignmentInput[]>([
    { kind: "releaseArtist", id: "" },
  ]);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);

  const assignmentsValid =
    assignments.length > 0 &&
    assignments.every((assignment) => isPublicId(assignment.id));
  const valid =
    isPublicId(sourceId) &&
    canonicalName.trim().length > 0 &&
    reason.trim().length > 0 &&
    assignmentsValid;

  const updateAssignment = (
    index: number,
    patch: Partial<SplitAssignmentInput>,
  ) => {
    setAssignments((current) =>
      current.map((assignment, position) =>
        position === index ? { ...assignment, ...patch } : assignment,
      ),
    );
  };

  const submit = () => {
    setBusy(true);
    setNotice(null);
    splitArtist(sourceId.trim(), {
      reason: reason.trim(),
      newArtist: {
        canonicalName: canonicalName.trim(),
        ...(sortName.trim() ? { sortName: sortName.trim() } : {}),
        ...(disambiguation.trim()
          ? { disambiguation: disambiguation.trim() }
          : {}),
      },
      assignments: assignments.map((assignment) => ({
        kind: assignment.kind,
        id: assignment.id.trim(),
      })),
    })
      .then((result) => {
        setNotice({
          kind: "success",
          text: t("identity.ops.split.success", {
            count: result.movedAssignments,
          }),
        });
        setSourceId("");
        setCanonicalName("");
        setSortName("");
        setDisambiguation("");
        setReason("");
        setAssignments([{ kind: "releaseArtist", id: "" }]);
        onCompleted();
      })
      .catch((cause: unknown) => {
        const error = toApiError(cause);
        setNotice({
          kind: "error",
          text: t("identity.ops.split.error", { message: error.message }),
        });
      })
      .finally(() => setBusy(false));
  };

  return (
    <section aria-label={t("identity.ops.split.title")}>
      <p style={{ color: "var(--cds-text-secondary)", marginBlockEnd: "1rem" }}>
        {t("identity.ops.split.description")}
      </p>
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          hideCloseButton
          title={notice.text}
          style={{ marginBlockEnd: "0.5rem" }}
        />
      ) : null}
      <TextInput
        id="identity-split-source"
        labelText={t("identity.ops.split.sourceLabel")}
        value={sourceId}
        invalid={sourceId.length > 0 && !isPublicId(sourceId)}
        invalidText={t("identity.ops.invalidPublicId")}
        onChange={(event) => setSourceId(event.target.value)}
      />
      <TextInput
        id="identity-split-name"
        labelText={t("identity.ops.split.canonicalName")}
        value={canonicalName}
        onChange={(event) => setCanonicalName(event.target.value)}
        style={{ marginBlockStart: "0.5rem" }}
      />
      <TextInput
        id="identity-split-sort-name"
        labelText={t("identity.ops.split.sortName")}
        value={sortName}
        onChange={(event) => setSortName(event.target.value)}
        style={{ marginBlockStart: "0.5rem" }}
      />
      <TextInput
        id="identity-split-disambiguation"
        labelText={t("identity.ops.split.disambiguation")}
        value={disambiguation}
        onChange={(event) => setDisambiguation(event.target.value)}
        style={{ marginBlockStart: "0.5rem" }}
      />
      <fieldset style={{ marginBlockStart: "1rem" }}>
        <legend>{t("identity.ops.split.assignments")}</legend>
        <p
          style={{
            color: "var(--cds-text-secondary)",
            marginBlockEnd: "0.5rem",
          }}
        >
          {t("identity.ops.split.internalIdNote")}
        </p>
        {assignments.map((assignment, index) => (
          <div key={index} className="melodee-identity-assignment">
            <Select
              id={`identity-split-kind-${index}`}
              labelText={t("identity.ops.split.assignmentKind")}
              value={assignment.kind}
              onChange={(event) =>
                updateAssignment(index, {
                  kind: event.target.value as SplitAssignmentKind,
                })
              }
            >
              {SPLIT_ASSIGNMENT_KINDS.map((kind) => (
                <SelectItem
                  key={kind}
                  value={kind}
                  text={t(`identity.ops.split.kind.${kind}`, {
                    defaultValue: kind,
                  })}
                />
              ))}
            </Select>
            <TextInput
              id={`identity-split-assignment-${index}`}
              labelText={t("identity.ops.split.assignmentId")}
              value={assignment.id}
              invalid={assignment.id.length > 0 && !isPublicId(assignment.id)}
              invalidText={t("identity.ops.split.invalidAssignmentId")}
              onChange={(event) =>
                updateAssignment(index, { id: event.target.value })
              }
            />
            <Button
              kind="ghost"
              size="sm"
              disabled={assignments.length <= 1}
              onClick={() =>
                setAssignments((current) =>
                  current.filter((_, position) => position !== index),
                )
              }
            >
              {t("actions.remove")}
            </Button>
          </div>
        ))}
        <Button
          kind="tertiary"
          size="sm"
          renderIcon={Add}
          onClick={() =>
            setAssignments((current) => [
              ...current,
              { kind: "releaseArtist", id: "" },
            ])
          }
        >
          {t("identity.ops.split.addAssignment")}
        </Button>
      </fieldset>
      <TextArea
        id="identity-split-reason"
        labelText={t("identity.decision.reasonLabel")}
        value={reason}
        onChange={(event) => setReason(event.target.value)}
        style={{ marginBlockStart: "0.5rem" }}
      />
      <Button
        kind="primary"
        size="sm"
        disabled={!valid || busy}
        onClick={submit}
        style={{ marginBlockStart: "1rem" }}
      >
        {t("identity.ops.split.action")}
      </Button>
    </section>
  );
}

export function IdentityReviewScreen() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();

  const sort = parseSort(searchParams.get("sort"));
  const page = parsePage(searchParams.get("page"));
  const pageSize = Math.max(
    1,
    Number.parseInt(searchParams.get("pageSize") ?? "25", 10) || 25,
  );
  const selectedObservationId = searchParams.get("observation");

  const [pageData, setPageData] = useState<Page<UnresolvedObservation> | null>(
    null,
  );
  const [listError, setListError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const [detail, setDetail] = useState<ObservationDetail | null>(null);
  const [detailError, setDetailError] = useState<ApiError | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [feed, setFeed] = useState<Page<IdentityDecisionEntry> | null>(null);
  const [feedError, setFeedError] = useState<ApiError | null>(null);

  const [candidateId, setCandidateId] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<Exclude<
    IdentityAction,
    "never-match"
  > | null>(null);
  const [neverMatchOpen, setNeverMatchOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);

  // Consume a flash notice armed before a URL-driven remount (see
  // armFlashNotice in ./identity). setState is deferred to a microtask so
  // nothing updates synchronously inside the effect body.
  useEffect(() => {
    const armed = takeFlashNotice();
    if (armed) {
      void Promise.resolve().then(() => setNotice(armed));
    }
  }, []);

  const updateParams = useCallback(
    (updates: Record<string, string | null>) => {
      const next = new URLSearchParams(searchParams);
      for (const [key, value] of Object.entries(updates)) {
        if (value === null) {
          next.delete(key);
        } else {
          next.set(key, value);
        }
      }
      setSearchParams(next, { replace: true });
    },
    [searchParams, setSearchParams],
  );

  // Server-side queue page. Rows stay on screen while a refetch is in
  // flight so post-resolution refreshes do not flash an empty queue.
  const fetchPage = useCallback(
    (signal: { cancelled: boolean }) =>
      listUnresolvedObservations({
        limit: pageSize,
        offset: (page - 1) * pageSize,
        sort,
      })
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
        }),
    [sort, page, pageSize],
  );

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchPage(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchPage]);

  // Detail fetch for the URL-selected observation. State updates happen in
  // promise callbacks (never synchronously in the effect body) to avoid
  // cascading renders (react-hooks/set-state-in-effect).
  const fetchDetail = useCallback(
    (signal: { cancelled: boolean }) => {
      if (!selectedObservationId) {
        return Promise.resolve().then(() => {
          if (!signal.cancelled) {
            setDetail(null);
            setDetailError(null);
            setDetailLoading(false);
          }
        });
      }
      const id = selectedObservationId;
      return Promise.resolve()
        .then(() => {
          if (!signal.cancelled) {
            setDetailLoading(true);
          }
          return getObservationDetail(id);
        })
        .then((result) => {
          if (!signal.cancelled) {
            setDetail(result);
            setDetailError(null);
            setDetailLoading(false);
          }
        })
        .catch((cause: unknown) => {
          if (!signal.cancelled) {
            setDetail(null);
            setDetailError(toApiError(cause));
            setDetailLoading(false);
          }
        });
    },
    [selectedObservationId],
  );

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchDetail(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchDetail]);

  // Candidate selection refers to the previously selected observation when
  // the selection changes; reset it during render (React docs pattern). The
  // action notice is deliberately preserved: post-resolution the queue
  // advances the selection and the success notification must survive.
  const [prevObservationId, setPrevObservationId] = useState(
    selectedObservationId,
  );
  if (prevObservationId !== selectedObservationId) {
    setPrevObservationId(selectedObservationId);
    setCandidateId(null);
  }

  const fetchFeed = useCallback((signal: { cancelled: boolean }) => {
    return listIdentityDecisions({ limit: FEED_LIMIT })
      .then((result) => {
        if (!signal.cancelled) {
          setFeed(result);
          setFeedError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setFeedError(toApiError(cause));
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchFeed(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchFeed]);

  const columns: EntityColumn[] = useMemo(
    () => [
      {
        key: "displayValue",
        header: t("identity.queue.column.observation"),
        sortable: true,
      },
      { key: "pending", header: t("identity.queue.column.pending") },
      {
        key: "createdAt",
        header: t("identity.queue.column.added"),
        sortable: true,
      },
    ],
    [t],
  );

  const rows: EntityRow[] = useMemo(
    () =>
      (pageData?.items ?? []).map((item) => ({
        id: item.id,
        cells: {
          displayValue: (
            <>
              <span className="melodee-work-item__title">
                {item.displayValue}
              </span>
              <span className="melodee-work-item__id">
                {observationContext(item)}
              </span>
            </>
          ),
          pending: (
            <StatusTag
              kind="provisional"
              label={t("identity.queue.pendingCount", {
                count: item.pendingMatchCount,
              })}
            />
          ),
          createdAt: (
            <span title={formatDateTime(item.createdAt)}>
              {formatRelativeTime(item.createdAt)}
            </span>
          ),
        },
      })),
    [pageData, t],
  );

  const candidate = selectedCandidate(detail, candidateId);
  const evidence = useMemo(() => evidenceCounts(detail), [detail]);
  const evidenceItems = useMemo(
    () =>
      evidence.map((entry) => ({
        source: t(`identity.evidence.kind.${entry.kind}`, {
          defaultValue: entry.kind,
        }),
        observation:
          entry.negative > 0
            ? t("identity.evidence.countWithNegative", {
                count: entry.count,
                negative: entry.negative,
              })
            : formatNumber(entry.count),
      })),
    [evidence, t],
  );

  const choices: DecisionChoice[] = IDENTITY_ACTIONS.map((action) => ({
    id: action,
    label: t(`identity.decision.action.${action}`),
    description: t(`identity.decision.action.${action}Description`),
    kind:
      action === "never-match"
        ? "destructive"
        : action === "confirm"
          ? "recommended"
          : "safe",
  }));

  const mutationErrorNotice = (cause: unknown) => {
    const error = toApiError(cause);
    setNotice({
      kind: "error",
      text: t("identity.decision.error", { message: error.message }),
    });
  };

  /** After a successful mutation: refresh queue + feed, select the next row. */
  const advanceQueue = (resolvedId: string) => {
    const next = nextSelection(pageData?.items ?? [], resolvedId);
    updateParams({ observation: next });
    void fetchPage({ cancelled: false });
    void fetchFeed({ cancelled: false });
  };

  const submitResolve = (resolution: IdentityResolution, reason: string) => {
    if (!selectedObservationId) {
      return;
    }
    const observationId = selectedObservationId;
    setBusy(true);
    setNotice(null);
    resolveObservation(observationId, {
      resolution,
      ...(resolution === "confirm" && candidate
        ? { artistId: candidate.artistId }
        : {}),
      reason,
    })
      .then(() => {
        const success: Notice = {
          kind: "success",
          text: t(`identity.decision.success.${resolution}`),
        };
        setNotice(success);
        // Advancing the queue changes the URL, which remounts this screen;
        // the flash slot carries the notice across the remount.
        armFlashNotice(success);
        advanceQueue(observationId);
      })
      .catch(mutationErrorNotice)
      .finally(() => setBusy(false));
  };

  const submitNeverMatch = (reason: string) => {
    if (!selectedObservationId || !candidate) {
      return;
    }
    const observationId = selectedObservationId;
    setBusy(true);
    setNotice(null);
    neverMatchCandidate(observationId, { artistId: candidate.artistId, reason })
      .then(() => {
        setNotice({
          kind: "success",
          text: t("identity.decision.success.never-match"),
        });
        void fetchDetail({ cancelled: false });
        void fetchFeed({ cancelled: false });
      })
      .catch(mutationErrorNotice)
      .finally(() => setBusy(false));
  };

  const onSelectAction = (id: string) => {
    setNotice(null);
    if (id === "never-match") {
      if (!candidate) {
        setNotice({
          kind: "warning",
          text: t("identity.decision.selectCandidateFirst"),
        });
        return;
      }
      setNeverMatchOpen(true);
      return;
    }
    if (id === "confirm" && !candidate) {
      setNotice({
        kind: "warning",
        text: t("identity.decision.selectCandidateFirst"),
      });
      return;
    }
    if (id === "confirm" || id === "create-new" || id === "leave-provisional") {
      setPendingAction(id);
    }
  };

  const observation = detail?.observation ?? null;

  return (
    <div>
      <PageHeader
        title={t("identity.title")}
        subtitle={t("identity.subtitle")}
      />
      <div className="melodee-identity-layout">
        <div className="melodee-identity-queue">
          <div className="melodee-review-card">
            <Heading style={{ fontSize: "1rem" }}>
              {t("identity.queue.title")}
            </Heading>
            <p className="melodee-page-header__subtitle">
              {t("identity.queue.remaining", { count: pageData?.total ?? 0 })}
            </p>
            {listError ? (
              <ActionableNotification
                inline
                kind="error"
                lowContrast
                title={t("identity.queue.loadErrorTitle")}
                subtitle={listError.message}
                hideCloseButton
                actionButtonLabel={t("actions.retry")}
                onActionButtonClick={() => void fetchPage({ cancelled: false })}
              />
            ) : null}
            {loading && pageData === null ? (
              <Loading description={t("common.loading")} withOverlay={false} />
            ) : (
              <EntityTable
                caption={t("identity.queue.caption")}
                columns={columns}
                rows={rows}
                totalRows={pageData?.total ?? 0}
                compact
                emptyTitle={t("identity.queue.emptyTitle")}
                emptyDescription={t("identity.queue.emptyDescription")}
                activeRowId={selectedObservationId}
                onRowActivate={(id) => updateParams({ observation: id })}
              />
            )}
          </div>
        </div>

        <div className="melodee-identity-main">
          <div className="melodee-review-card">
            {!selectedObservationId ? (
              <EmptyState
                title={t("identity.observation.emptyTitle")}
                description={t("identity.observation.emptyDescription")}
              />
            ) : detailLoading && detail === null ? (
              <Loading description={t("common.loading")} withOverlay={false} />
            ) : detailError ? (
              <ActionableNotification
                inline
                kind="error"
                lowContrast
                title={t("identity.observation.loadErrorTitle")}
                subtitle={detailError.message}
                hideCloseButton
                actionButtonLabel={t("actions.retry")}
                onActionButtonClick={() =>
                  void fetchDetail({ cancelled: false })
                }
              />
            ) : observation ? (
              <>
                <span className="melodee-review-fact__label">
                  {t("identity.observation.incoming")}
                </span>
                <div className="melodee-identity-observation__header">
                  <Heading style={{ fontSize: "1.25rem" }}>
                    {observation.displayValue}
                  </Heading>
                  <StatusTag
                    kind="provisional"
                    label={t("identity.observation.reviewTag")}
                  />
                </div>
                <div className="melodee-review-header__facts">
                  <div>
                    <div className="melodee-review-fact__label">
                      {t("identity.observation.release")}
                    </div>
                    <div>{observation.releaseTitle}</div>
                  </div>
                  {observation.trackTitle ? (
                    <div>
                      <div className="melodee-review-fact__label">
                        {t("identity.observation.track")}
                      </div>
                      <div>{observation.trackTitle}</div>
                    </div>
                  ) : null}
                  <div>
                    <div className="melodee-review-fact__label">
                      {t("identity.observation.scope")}
                    </div>
                    <div>
                      {t(
                        `identity.observation.scopeValue.${observation.scope}`,
                        {
                          defaultValue: observation.scope,
                        },
                      )}
                    </div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">
                      {t("identity.observation.normalized")}
                    </div>
                    <div>{observation.normalizedValue}</div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">
                      {t("identity.observation.source")}
                    </div>
                    <div>{observation.sourcePointer}</div>
                  </div>
                  <div>
                    <div className="melodee-review-fact__label">
                      {t("identity.observation.added")}
                    </div>
                    <div>
                      <span title={formatDateTime(observation.createdAt)}>
                        {formatRelativeTime(observation.createdAt)}
                      </span>
                    </div>
                  </div>
                </div>
                {detail?.current ? (
                  <div style={{ marginBlockStart: "0.75rem" }}>
                    <div className="melodee-review-fact__label">
                      {t("identity.observation.currentDecision")}
                    </div>
                    <div>
                      {detail.current.artistName}
                      {" · "}
                      {t(`identity.outcome.${detail.current.outcome}`, {
                        defaultValue: detail.current.outcome,
                      })}
                      {" · "}
                      {formatNumber(
                        confidencePercent(detail.current.confidence),
                      )}
                      %{detail.current.locked ? ` · ${t("status.locked")}` : ""}
                    </div>
                  </div>
                ) : null}
              </>
            ) : null}
          </div>

          {detail ? (
            <div className="melodee-review-card">
              <Heading style={{ fontSize: "1rem", marginBlockEnd: "0.5rem" }}>
                {t("identity.candidates.title")}
              </Heading>
              {detail.candidates.length === 0 ? (
                <EmptyState title={t("identity.candidates.empty")} compact />
              ) : (
                <div
                  role="radiogroup"
                  aria-label={t("identity.candidates.title")}
                >
                  {detail.candidates.map((entry) => (
                    <CandidateCard
                      key={entry.artistId}
                      detail={detail}
                      artistId={entry.artistId}
                      selected={candidateId === entry.artistId}
                      onSelect={setCandidateId}
                    />
                  ))}
                </div>
              )}
            </div>
          ) : null}

          {detail ? (
            <div className="melodee-review-card">
              <EvidenceSummary
                title={t("identity.evidence.summaryTitle")}
                items={evidenceItems}
              />
            </div>
          ) : null}
        </div>

        <div className="melodee-identity-rail">
          <div className="melodee-review-card">
            {notice ? (
              <InlineNotification
                kind={notice.kind}
                lowContrast
                hideCloseButton
                title={notice.text}
                style={{ marginBlockEnd: "0.5rem" }}
              />
            ) : null}
            {detail ? (
              <DecisionPanel choices={choices} onSelect={onSelectAction} />
            ) : (
              <>
                <Heading style={{ fontSize: "1rem", marginBlockEnd: "0.5rem" }}>
                  {t("identity.decision.title")}
                </Heading>
                <p style={{ color: "var(--cds-text-secondary)" }}>
                  {t("identity.decision.unavailable")}
                </p>
              </>
            )}
          </div>
        </div>
      </div>

      <div className="melodee-review-card">
        <Heading style={{ fontSize: "1rem", marginBlockEnd: "0.5rem" }}>
          {t("identity.feed.title")}
        </Heading>
        {feedError ? (
          <InlineNotification
            kind="error"
            lowContrast
            hideCloseButton
            title={t("identity.feed.loadErrorTitle")}
            subtitle={feedError.message}
          />
        ) : (
          <AuditTimeline
            events={(feed?.items ?? []).map(toAuditEvent)}
            relative
          />
        )}
      </div>

      <div className="melodee-review-card">
        <Accordion>
          <AccordionItem title={t("identity.ops.title")}>
            <MergeForm
              onCompleted={() => {
                void fetchFeed({ cancelled: false });
              }}
            />
            <hr
              style={{ marginBlockStart: "1.5rem", marginBlockEnd: "1.5rem" }}
            />
            <SplitForm
              onCompleted={() => {
                void fetchFeed({ cancelled: false });
              }}
            />
          </AccordionItem>
        </Accordion>
      </div>

      <ResolveDialog
        action={pendingAction}
        busy={busy}
        candidateName={candidate?.artistName ?? null}
        onClose={() => setPendingAction(null)}
        onConfirm={(reason) => {
          const action = pendingAction;
          setPendingAction(null);
          if (action) {
            submitResolve(action, reason);
          }
        }}
      />
      {/* Modals mount only while open: Carbon keeps a closed Modal in the
          DOM, which would duplicate labels/dialog roles for assistive tech. */}
      {neverMatchOpen && candidate ? (
        <NeverMatchDialog
          open
          busy={busy}
          candidateName={candidate.artistName}
          onClose={() => setNeverMatchOpen(false)}
          onConfirm={(reason) => {
            setNeverMatchOpen(false);
            submitNeverMatch(reason);
          }}
        />
      ) : null}
    </div>
  );
}
