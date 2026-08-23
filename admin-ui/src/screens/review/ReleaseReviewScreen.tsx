import {
  ActionableNotification,
  Button,
  Heading,
  InlineNotification,
  Link,
  Loading,
  Modal,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
  Tag,
  TextArea,
} from '@carbon/react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link as RouterLink, useParams } from 'react-router-dom';

import {
  getReviewDetail,
  postReviewDecision,
  postReviewReprocess,
  reviewTrackPreviewUrl,
} from '../../api/admin';
import { ApiError } from '../../api/client';
import type { ReviewDetail, ReviewTrack } from '../../api/types';
import { AudioPreview } from '../../components/AudioPreview';
import { AuditTimeline } from '../../components/AuditTimeline';
import { DecisionPanel, type DecisionChoice } from '../../components/DecisionPanel';
import { DestructiveActionDialog } from '../../components/DestructiveActionDialog';
import { EntityAvatar } from '../../components/EntityAvatar';
import { EmptyState } from '../../components/EmptyState';
import { EvidenceItem, type Evidence } from '../../components/EvidenceItem';
import { EvidenceSummary } from '../../components/EvidenceSummary';
import { PageHeader } from '../../components/PageHeader';
import { ProcessingStateTag } from '../../components/ProcessingStateTag';
import { StatusTag } from '../../components/StatusTag';
import { formatDateTime, formatDuration, formatNumber } from '../../i18n/format';
import {
  dispositionKind,
  noveltyByIndex,
  noveltyKind,
  noveltyLabelKey,
  recommendedAction,
  releaseDisplayTitle,
  REVIEW_ACTIONS,
  toAuditEvent,
  totalDurationMs,
  type ReviewAction,
} from './review';

/**
 * Release Review workspace (PRD-UI-007/008, SPEC §20): evidence
 * first, choices last (UI-SPEC §7). The main column carries the Release
 * header, classification evidence, catalog comparison, and per-track
 * breakdown with inline audio preview; the right rail carries the ordered
 * decision actions, the policy match, and the review timeline.
 *
 * Decision POSTs go to /review/{id}/decision with a required audited reason;
 * delete additionally sends `override: true` after the typed confirmation in
 * DestructiveActionDialog. 409 illegal-transition/conflict and 400
 * override_required responses surface as localized inline notifications.
 */

interface Notice {
  kind: 'success' | 'error' | 'info';
  text: string;
}

/** Percentage fragment for a comparison count ("12 (75%)") — numbers only. */
function countWithPercent(count: number, total: number): string {
  const percent = total > 0 ? Math.round((count / total) * 100) : 0;
  return `${formatNumber(count)} (${formatNumber(percent)}%)`;
}

/** Per-track preview cell: labeled transport, no autoplay, graceful 404. */
function TrackPreview({
  workItemId,
  track,
  title,
}: {
  workItemId: string;
  track: ReviewTrack;
  title: string;
}) {
  const { t } = useTranslation();
  const [unavailable, setUnavailable] = useState(false);
  if (unavailable) {
    return <span className="melodee-review-preview-note">{t('review.preview.unavailable')}</span>;
  }
  return (
    <AudioPreview
      src={reviewTrackPreviewUrl(workItemId, track.index)}
      label={t('review.preview.trackLabel', { index: formatNumber(track.index + 1), title })}
      onError={() => setUnavailable(true)}
    />
  );
}

/** Confirmation for the non-destructive decisions; reason is required. */
function DecisionConfirmDialog({
  action,
  busy,
  onClose,
  onConfirm,
}: {
  action: ReviewAction | null;
  busy: boolean;
  onClose: () => void;
  onConfirm: (reason: string) => void;
}) {
  const { t } = useTranslation();
  const [reason, setReason] = useState('');
  if (!action) {
    return null;
  }
  const reasonValid = reason.trim().length > 0;
  return (
    <Modal
      open
      modalHeading={t('review.decision.confirmHeading', {
        action: t(`review.decision.action.${action}`),
      })}
      primaryButtonText={t('review.decision.apply')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={!reasonValid || busy}
      onRequestClose={() => {
        setReason('');
        onClose();
      }}
      onRequestSubmit={() => {
        const submitted = reason.trim();
        setReason('');
        onConfirm(submitted);
      }}
    >
      <TextArea
        id="review-decision-reason"
        labelText={t('review.decision.reasonLabel')}
        value={reason}
        invalid={!reasonValid && reason.length > 0}
        invalidText={t('review.decision.reasonRequired')}
        onChange={(event) => setReason(event.target.value)}
      />
    </Modal>
  );
}

export function ReleaseReviewScreen() {
  const { t } = useTranslation();
  const { workItemId = '' } = useParams();

  const [detail, setDetail] = useState<ReviewDetail | null>(null);
  const [loadError, setLoadError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const [pendingAction, setPendingAction] = useState<ReviewAction | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [reprocessOpen, setReprocessOpen] = useState(false);
  const [reprocessReason, setReprocessReason] = useState('');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);

  const fetchDetail = useCallback(
    (signal: { cancelled: boolean }) =>
      getReviewDetail(workItemId)
        .then((result) => {
          if (!signal.cancelled) {
            setDetail(result);
            setLoadError(null);
            setLoading(false);
          }
        })
        .catch((cause: unknown) => {
          if (!signal.cancelled) {
            setLoadError(
              cause instanceof ApiError ? cause : new ApiError(0, 'unknown', String(cause), null),
            );
            setLoading(false);
          }
        }),
    [workItemId],
  );

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchDetail(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchDetail]);

  const manifest = detail?.manifest ?? null;
  const classification = detail?.classification ?? null;
  const comparison = detail?.catalogComparison ?? null;
  const decision = detail?.decision ?? null;
  const state = detail?.workItem.state;

  const title = detail
    ? releaseDisplayTitle(detail, t('review.workspace.untitled'))
    : t('review.workspace.untitled');

  // Duration is only a fact when the manifest recorded at least one.
  const durationMs = useMemo(() => totalDurationMs(manifest), [manifest]);
  const novelty = useMemo(() => noveltyByIndex(manifest, comparison), [manifest, comparison]);

  const headerFacts: { label: string; value: string }[] = useMemo(() => {
    if (!detail) {
      return [];
    }
    const facts: { label: string; value: string | null }[] = [
      {
        label: t('review.header.tracks'),
        value: manifest ? formatNumber(manifest.tracks.length) : null,
      },
      {
        label: t('review.header.duration'),
        value: durationMs != null ? formatDuration(durationMs) : null,
      },
      {
        label: t('review.header.findings'),
        value: manifest ? formatNumber(manifest.findingCount) : null,
      },
      { label: t('review.header.stateReason'), value: detail.workItem.stateReason },
      { label: t('review.header.attempts'), value: formatNumber(detail.workItem.attemptCount) },
      { label: t('review.header.added'), value: formatDateTime(detail.workItem.createdAt) },
    ];
    return facts.flatMap(({ label, value }) => (value ? [{ label, value }] : []));
  }, [detail, manifest, durationMs, t]);

  const evidenceItems: Evidence[] = useMemo(
    () =>
      (classification?.evidence ?? []).map((evidence) => ({
        source: `${evidence.source} · ${evidence.kind}`,
        observation: evidence.detail,
        resolved: evidence.trait,
        confidence: evidence.confidence / 10_000,
      })),
    [classification],
  );

  const comparisonItems: Evidence[] = useMemo(() => {
    if (!comparison) {
      return [];
    }
    const total = comparison.totalTracks;
    return [
      { source: t('review.comparison.known'), observation: countWithPercent(comparison.known, total) },
      { source: t('review.comparison.new'), observation: countWithPercent(comparison.new, total) },
      {
        source: t('review.comparison.alternate'),
        observation: countWithPercent(comparison.alternate, total),
      },
      {
        source: t('review.comparison.uncertain'),
        observation: countWithPercent(comparison.uncertain, total),
      },
    ];
  }, [comparison, t]);

  const recommended = useMemo(() => recommendedAction(detail), [detail]);

  const choices: DecisionChoice[] = REVIEW_ACTIONS.map((action) => ({
    id: action,
    label: t(`review.decision.action.${action}`),
    description: t(`review.decision.action.${action}Description`),
    kind:
      action === 'delete' ? 'destructive' : action === recommended ? 'recommended' : 'safe',
  }));

  const onSelectAction = (id: string) => {
    setNotice(null);
    if (id === 'delete') {
      setDeleteOpen(true);
    } else if (id === 'accept' || id === 'quarantine' || id === 'ignore') {
      setPendingAction(id);
    }
  };

  const submitDecision = (action: ReviewAction, reason: string) => {
    setBusy(true);
    setNotice(null);
    postReviewDecision(workItemId, {
      action,
      reason,
      ...(action === 'delete' ? { override: true } : {}),
    })
      .then((result) => {
        setNotice({
          kind: 'success',
          text: action === 'delete' && !result.pendingExecution
            ? t('review.decision.deleteSuccess')
            : result.pendingExecution
            ? t('review.decision.deletePending')
            : t('review.decision.success'),
        });
        void fetchDetail({ cancelled: false });
      })
      .catch((cause: unknown) => {
        if (cause instanceof ApiError) {
          if (cause.code === 'override_required') {
            setNotice({ kind: 'error', text: t('review.decision.overrideRequired') });
          } else if (cause.status === 409) {
            setNotice({
              kind: 'error',
              text: t('review.decision.conflict', { message: cause.message }),
            });
          } else {
            setNotice({
              kind: 'error',
              text: t('review.decision.conflict', { message: cause.message }),
            });
          }
        } else {
          setNotice({
            kind: 'error',
            text: t('review.decision.conflict', { message: String(cause) }),
          });
        }
      })
      .finally(() => setBusy(false));
  };

  const submitReprocess = (reason: string) => {
    setBusy(true);
    setNotice(null);
    postReviewReprocess(workItemId, reason || undefined)
      .then(() => {
        setNotice({ kind: 'success', text: t('review.reprocess.success') });
        void fetchDetail({ cancelled: false });
      })
      .catch((cause: unknown) => {
        const message = cause instanceof ApiError ? cause.message : String(cause);
        setNotice({ kind: 'error', text: t('review.reprocess.error', { message }) });
      })
      .finally(() => setBusy(false));
  };

  const reprocessable = state === 'failed' || state === 'quarantined';
  const decisionAffected = manifest
    ? {
        affectedReleases: 1,
        affectedTracks: manifest.tracks.length,
        affectedFiles:
          manifest.tracks.length + manifest.artwork.length + manifest.sidecars.length,
      }
    : {};

  return (
    <div>
      <PageHeader title={t('review.workspace.title')} subtitle={t('review.workspace.subtitle')} />
      <p>
        <Link as={RouterLink} to="/processing/review">
          {t('review.workspace.back')}
        </Link>
      </p>
      {loadError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t('review.workspace.loadErrorTitle')}
          subtitle={loadError.message}
          hideCloseButton
          actionButtonLabel={t('actions.retry')}
          onActionButtonClick={() => {
            setLoading(detail === null);
            void fetchDetail({ cancelled: false });
          }}
        />
      ) : null}
      {loading && detail === null ? (
        <Loading description={t('common.loading')} withOverlay={false} />
      ) : null}
      {detail ? (
        <div className="melodee-review-layout">
          <div className="melodee-review-main">
            <div className="melodee-review-card">
              <div className="melodee-review-header">
                <EntityAvatar name={title} size={96} decorative />
                <div className="melodee-review-header__title">
                  <Heading style={{ fontSize: '1.25rem' }}>{title}</Heading>
                  <div className="melodee-review-header__facts">
                    {headerFacts.map((fact) => (
                      <div key={fact.label}>
                        <div className="melodee-review-fact__label">{fact.label}</div>
                        <div>{fact.value}</div>
                      </div>
                    ))}
                  </div>
                </div>
                <ProcessingStateTag state={detail.workItem.state} />
              </div>
            </div>

            <div className="melodee-review-card">
              <Heading style={{ fontSize: '1rem', marginBlockEnd: '0.5rem' }}>
                {t('review.classification.title')}
              </Heading>
              {classification ? (
                <>
                  <div className="melodee-review-header__facts">
                    <div>
                      <div className="melodee-review-fact__label">
                        {t('review.classification.confidence')}
                      </div>
                      <div>{formatNumber(Math.round(classification.confidence / 100))}%</div>
                    </div>
                    <div>
                      <div className="melodee-review-fact__label">
                        {t('review.classification.classifiedAt')}
                      </div>
                      <div>{formatDateTime(classification.classifiedAt)}</div>
                    </div>
                    <div>
                      <div className="melodee-review-fact__label">
                        {t('review.classification.version')}
                      </div>
                      <div>{classification.classifierVersion}</div>
                    </div>
                  </div>
                  {classification.traits.length > 0 ? (
                    <p>
                      {classification.traits.map((trait) => (
                        <Tag key={trait} type="outline" style={{ marginInlineEnd: '0.25rem' }}>
                          {trait}
                        </Tag>
                      ))}
                    </p>
                  ) : null}
                  {evidenceItems.map((item, index) => (
                    <EvidenceItem key={`${item.source}-${index}`} evidence={item} />
                  ))}
                </>
              ) : (
                <EmptyState title={t('review.classification.empty')} compact />
              )}
            </div>

            <div className="melodee-review-card">
              {comparison ? (
                <>
                  <EvidenceSummary title={t('review.comparison.title')} items={comparisonItems} />
                  <div className="melodee-review-header__facts">
                    <div>
                      <div className="melodee-review-fact__label">
                        {t('review.comparison.evaluatedAt')}
                      </div>
                      <div>{formatDateTime(comparison.evaluatedAt)}</div>
                    </div>
                    <div>
                      <div className="melodee-review-fact__label">
                        {t('review.comparison.version')}
                      </div>
                      <div>{comparison.evaluatorVersion}</div>
                    </div>
                  </div>
                </>
              ) : (
                <>
                  <Heading style={{ fontSize: '1rem', marginBlockEnd: '0.5rem' }}>
                    {t('review.comparison.title')}
                  </Heading>
                  <EmptyState title={t('review.comparison.empty')} compact />
                </>
              )}
            </div>

            <div className="melodee-review-card">
              <Heading style={{ fontSize: '1rem', marginBlockEnd: '0.5rem' }}>
                {t('review.tracks.title')}
              </Heading>
              {manifest && manifest.tracks.length > 0 ? (
                <div className="melodee-review-table-scroll">
                  <TableContainer title={t('review.tracks.caption')}>
                  <Table size="sm" aria-label={t('review.tracks.caption')}>
                    <TableHead>
                      <TableRow>
                        <TableHeader>{t('review.tracks.column.number')}</TableHeader>
                        <TableHeader>{t('review.tracks.column.title')}</TableHeader>
                        <TableHeader>{t('review.tracks.column.duration')}</TableHeader>
                        <TableHeader>{t('review.tracks.column.format')}</TableHeader>
                        {novelty.size > 0 ? (
                          <TableHeader>{t('review.tracks.column.result')}</TableHeader>
                        ) : null}
                        <TableHeader>{t('review.tracks.column.preview')}</TableHeader>
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {manifest.tracks.map((track) => {
                        const trackTitle = track.title ?? t('review.tracks.untitled');
                        const result = novelty.get(track.index);
                        return (
                          <TableRow key={track.index}>
                            <TableCell>{formatNumber(track.index + 1)}</TableCell>
                            <TableCell>
                              {trackTitle}
                              <span className="melodee-review-track__file">{track.filePath}</span>
                            </TableCell>
                            <TableCell>
                              {track.durationMs != null
                                ? formatDuration(track.durationMs)
                                : t('common.none')}
                            </TableCell>
                            <TableCell>
                              {[track.container, track.codec].filter(Boolean).join(' / ') ||
                                t('common.none')}
                            </TableCell>
                            {novelty.size > 0 ? (
                              <TableCell>
                                {result ? (
                                  <span title={result.detail}>
                                    <StatusTag
                                      kind={noveltyKind(result.category)}
                                      label={t(noveltyLabelKey(result.category))}
                                    />
                                  </span>
                                ) : null}
                              </TableCell>
                            ) : null}
                            <TableCell>
                              <TrackPreview
                                workItemId={workItemId}
                                track={track}
                                title={trackTitle}
                              />
                            </TableCell>
                          </TableRow>
                        );
                      })}
                    </TableBody>
                  </Table>
                  </TableContainer>
                </div>
              ) : (
                <EmptyState title={t('review.tracks.empty')} compact />
              )}
            </div>
          </div>

          <div className="melodee-review-rail">
            <div className="melodee-review-card">
              <Heading style={{ fontSize: '1rem', marginBlockEnd: '0.5rem' }}>
                {t('review.decision.title')}
              </Heading>
              {notice ? (
                <InlineNotification
                  kind={notice.kind}
                  lowContrast
                  hideCloseButton
                  title={notice.text}
                  style={{ marginBlockEnd: '0.5rem' }}
                />
              ) : null}
              {state === 'needs_review' ? (
                <DecisionPanel choices={choices} onSelect={onSelectAction} />
              ) : (
                <>
                  {reprocessable ? (
                    <Button
                      kind="tertiary"
                      disabled={busy}
                      onClick={() => {
                        setNotice(null);
                        setReprocessOpen(true);
                      }}
                    >
                      {t('review.reprocess.action')}
                    </Button>
                  ) : null}
                  <p style={{ color: 'var(--cds-text-secondary)', marginBlockStart: '0.5rem' }}>
                    {reprocessable
                      ? t('review.reprocess.description')
                      : t('review.decision.unavailable')}
                  </p>
                </>
              )}
            </div>

            <div className="melodee-review-card">
              <Heading style={{ fontSize: '1rem', marginBlockEnd: '0.5rem' }}>
                {t('review.policy.title')}
              </Heading>
              {/* HTML permits one level of <div> grouping inside <dl>; axe's
                  definition-list/dlitem rules reject deeper nesting, so the
                  grid div IS the <dl> and each fact wraps dt+dd. */}
              {decision ? (
                <dl className="melodee-review-header__facts">
                  <div>
                    <dt className="melodee-review-fact__label">
                      {t('review.policy.matchedRule')}
                    </dt>
                    <dd style={{ marginInlineStart: 0 }}>
                      {decision.matchedRuleIds.length > 0
                        ? decision.matchedRuleIds.join(', ')
                        : t('common.none')}
                    </dd>
                  </div>
                  <div>
                    <dt className="melodee-review-fact__label">
                      {t('review.policy.recommendation')}
                    </dt>
                    <dd style={{ marginInlineStart: 0 }}>
                      <StatusTag
                        kind={dispositionKind(decision.disposition)}
                        label={t(`review.disposition.${decision.disposition}`, {
                          defaultValue: decision.disposition,
                        })}
                      />
                    </dd>
                  </div>
                  <div>
                    <dt className="melodee-review-fact__label">
                      {t('review.policy.completionState')}
                    </dt>
                    <dd style={{ marginInlineStart: 0 }}>{decision.completionState}</dd>
                  </div>
                  <div>
                    <dt className="melodee-review-fact__label">{t('review.policy.decidedAt')}</dt>
                    <dd style={{ marginInlineStart: 0 }}>
                      {formatDateTime(decision.decidedAt)}
                    </dd>
                  </div>
                  <div>
                    <dt className="melodee-review-fact__label">
                      {t('review.policy.ruleSetVersion')}
                    </dt>
                    <dd style={{ marginInlineStart: 0 }}>
                      {formatNumber(decision.ruleSetVersion)}
                    </dd>
                  </div>
                  <div className="melodee-review-fact--full">
                    <dt className="melodee-review-fact__label">{t('review.policy.reason')}</dt>
                    <dd style={{ marginInlineStart: 0 }}>{decision.evidenceSummary}</dd>
                  </div>
                  {decision.overrideReason ? (
                    <div className="melodee-review-fact--full">
                      <dt className="melodee-review-fact__label">
                        {t('review.policy.overrideReason')}
                      </dt>
                      <dd style={{ marginInlineStart: 0 }}>{decision.overrideReason}</dd>
                    </div>
                  ) : null}
                </dl>
              ) : (
                <EmptyState title={t('review.policy.empty')} compact />
              )}
            </div>

            <div className="melodee-review-card">
              <Heading style={{ fontSize: '1rem', marginBlockEnd: '0.5rem' }}>
                {t('review.timeline.title')}
              </Heading>
              <AuditTimeline events={detail.timeline.map(toAuditEvent)} />
            </div>
          </div>
        </div>
      ) : null}

      <DecisionConfirmDialog
        action={pendingAction}
        busy={busy}
        onClose={() => setPendingAction(null)}
        onConfirm={(reason) => {
          const action = pendingAction;
          setPendingAction(null);
          if (action) {
            submitDecision(action, reason);
          }
        }}
      />
      {/* Modals mount only while open: Carbon keeps a closed Modal in the
          DOM, which would duplicate labels/dialog roles for assistive tech. */}
      {deleteOpen ? (
        <DestructiveActionDialog
          open
          title={t('review.decision.deleteTitle')}
          actionLabel={t('review.decision.deleteActionLabel')}
          deleteClass
          {...decisionAffected}
          matchedRule={
            decision && decision.matchedRuleIds.length > 0
              ? decision.matchedRuleIds.join(', ')
              : undefined
          }
          onClose={() => setDeleteOpen(false)}
          onConfirm={(reason) => {
            setDeleteOpen(false);
            submitDecision('delete', reason);
          }}
        />
      ) : null}
      {reprocessOpen ? (
        <Modal
          open
          modalHeading={t('review.reprocess.title')}
          primaryButtonText={t('review.reprocess.action')}
          secondaryButtonText={t('actions.cancel')}
          primaryButtonDisabled={busy}
          onRequestClose={() => {
            setReprocessOpen(false);
            setReprocessReason('');
          }}
          onRequestSubmit={() => {
            setReprocessOpen(false);
            submitReprocess(reprocessReason);
            setReprocessReason('');
          }}
        >
          <p style={{ marginBlockEnd: '0.5rem' }}>{t('review.reprocess.description')}</p>
          <TextArea
            id="review-reprocess-reason"
            labelText={t('review.decision.reasonLabel')}
            value={reprocessReason}
            onChange={(event) => setReprocessReason(event.target.value)}
          />
        </Modal>
      ) : null}
    </div>
  );
}
