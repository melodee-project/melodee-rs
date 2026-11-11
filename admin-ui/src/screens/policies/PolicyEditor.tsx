import { Add, TrashCan } from '@carbon/icons-react';
import {
  Button,
  IconButton,
  InlineNotification,
  Link,
  Select,
  SelectItem,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
  TextInput,
} from '@carbon/react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link as RouterLink } from 'react-router-dom';

import { ApiError } from '../../api/client';
import type {
  PolicyDisposition,
  PolicyRule,
  PolicyRuleInput,
  PolicyRuleType,
  PolicyTestRequest,
  PolicyTestRun,
} from '../../api/types';
import { StatusTag } from '../../components/StatusTag';
import { formatNumber } from '../../i18n/format';
import { dispositionKind } from '../review/review';
import {
  DISPOSITIONS,
  RELEASE_TRAITS,
  RULE_TYPES,
  allowedDispositions,
  draftMatchesRule,
  draftToInput,
  isProtectedRule,
  predicateLocked,
  ruleToDraft,
  validateDraft,
  type PredicateCondition,
  type RuleDraft,
} from './policies';

/**
 * Policy Editor (PRD-POLICY-008): the structured rule editor. The
 * conditions builder only produces rows for the two dimensions the engine
 * predicate schema knows (`required_traits`, `existing_dispositions`); the
 * operator column is fixed per field because the predicate model has no
 * free-form operators. Disposition choices are filtered to the per-type
 * safe set enforced by PolicyRule::validate server-side.
 *
 * "Test against catalog" is a read-only dry-run: stored rules unchanged in
 * the editor run by ruleId (persisted to policy_test_runs); any unsaved
 * definition runs as an ad-hoc candidate and the panel states honestly that
 * the run is not persisted.
 */

/** Upper bound on catalog Releases evaluated by one test run (backend). */
const MAX_TEST_RUN_RELEASES = 500;

export interface PolicyEditorProps {
  mode: 'create' | 'edit';
  /** The loaded rule in edit mode. */
  rule?: PolicyRule;
  busy: boolean;
  onSave: (input: PolicyRuleInput) => Promise<void>;
  onCancel: () => void;
  /** Enable/disable a stored rule (idempotent server-side). */
  onToggleEnabled: (rule: PolicyRule, enable: boolean) => Promise<void>;
  /** Opens the delete confirmation owned by the screen. */
  onDelete: (rule: PolicyRule) => void;
  /** Recovery action for 409 rule_set_conflict: refetch and close. */
  onReload: () => void;
  onTest: (request: PolicyTestRequest) => Promise<PolicyTestRun>;
}

interface Notice {
  kind: 'success' | 'error' | 'info' | 'warning';
  text: string;
  /** Show the conflict recovery reload action. */
  reload?: boolean;
}

function defaultDraft(): RuleDraft {
  return {
    id: '',
    ruleType: 'trait_disposition',
    disposition: 'review',
    confidenceThreshold: null,
    conditions: [{ field: 'release_trait', value: 'greatest_hits' }],
  };
}

export function PolicyEditor({
  mode,
  rule,
  busy,
  onSave,
  onCancel,
  onToggleEnabled,
  onDelete,
  onReload,
  onTest,
}: PolicyEditorProps) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState<RuleDraft>(() =>
    mode === 'edit' && rule ? ruleToDraft(rule) : defaultDraft(),
  );
  const [confidenceText, setConfidenceText] = useState(() =>
    mode === 'edit' && rule?.confidenceThreshold != null ? String(rule.confidenceThreshold) : '',
  );
  const [notice, setNotice] = useState<Notice | null>(null);
  const [showValidation, setShowValidation] = useState(false);
  const [testBusy, setTestBusy] = useState(false);
  const [limitText, setLimitText] = useState(String(MAX_TEST_RUN_RELEASES));
  const [testRun, setTestRun] = useState<PolicyTestRun | null>(null);

  const isProtected = mode === 'edit' && rule ? isProtectedRule(rule) : false;
  const disabledRule = mode === 'edit' && rule ? !rule.enabled : false;
  const conditionsLocked = predicateLocked(draft.ruleType);
  // Disabled rules are no longer in the active set; the backend replaces
  // only active-set rules, so saving a disabled rule is not possible.
  const formLocked = busy || disabledRule;

  const confidenceValue = useMemo(() => {
    const trimmed = confidenceText.trim();
    if (trimmed === '') {
      return null;
    }
    const parsed = Number(trimmed);
    return Number.isFinite(parsed) ? parsed : Number.NaN;
  }, [confidenceText]);

  const effectiveDraft: RuleDraft = useMemo(
    () => ({ ...draft, confidenceThreshold: confidenceValue }),
    [draft, confidenceValue],
  );

  const validationErrors = validateDraft(effectiveDraft);

  const update = (patch: Partial<RuleDraft>) => {
    setDraft((current) => ({ ...current, ...patch }));
    setTestRun(null);
  };

  const setRuleType = (ruleType: PolicyRuleType) => {
    const allowed = allowedDispositions(ruleType);
    update({
      ruleType,
      // Snap the disposition into the new type's safe set.
      disposition: allowed.includes(draft.disposition) ? draft.disposition : allowed[0]!,
      // Safety rules carry an empty predicate by engine invariant.
      conditions: predicateLocked(ruleType) ? [] : draft.conditions,
    });
  };

  const setCondition = (index: number, patch: Partial<PredicateCondition>) => {
    const next = draft.conditions.map((condition, position) =>
      position === index ? { ...condition, ...patch } : condition,
    );
    update({ conditions: next });
  };

  const removeCondition = (index: number) => {
    update({ conditions: draft.conditions.filter((_, position) => position !== index) });
  };

  const addCondition = () => {
    update({
      conditions: [...draft.conditions, { field: 'release_trait', value: RELEASE_TRAITS[0]! }],
    });
  };

  const toErrorNotice = (cause: unknown, fallbackKey: string): Notice => {
    if (cause instanceof ApiError) {
      if (cause.status === 409 && cause.code === 'rule_set_conflict') {
        return { kind: 'warning', text: t('policies.editor.conflict'), reload: true };
      }
      if (cause.status === 409 && cause.code === 'protected_rule') {
        return { kind: 'warning', text: t('policies.editor.protectedError') };
      }
      return {
        kind: 'error',
        text: t(fallbackKey, { message: cause.message }),
      };
    }
    return { kind: 'error', text: t(fallbackKey, { message: String(cause) }) };
  };

  const save = () => {
    setShowValidation(true);
    if (validationErrors.length > 0) {
      return;
    }
    setNotice(null);
    onSave(draftToInput(effectiveDraft)).catch((cause: unknown) => {
      setNotice(toErrorNotice(cause, 'policies.editor.saveError'));
    });
  };

  const runTest = () => {
    setShowValidation(true);
    if (validationErrors.length > 0) {
      return;
    }
    const parsedLimit = Number(limitText);
    const limit =
      Number.isInteger(parsedLimit) && parsedLimit >= 1
        ? Math.min(parsedLimit, MAX_TEST_RUN_RELEASES)
        : MAX_TEST_RUN_RELEASES;
    // Unchanged stored rules run by ruleId (audited); anything else runs as
    // an ad-hoc candidate, which the backend never persists.
    const request: PolicyTestRequest =
      mode === 'edit' && rule && rule.enabled && draftMatchesRule(effectiveDraft, rule)
        ? { ruleId: rule.id, limit }
        : { rule: draftToInput(effectiveDraft), limit };
    setTestBusy(true);
    setNotice(null);
    onTest(request)
      .then((run) => setTestRun(run))
      .catch((cause: unknown) => {
        setNotice(toErrorNotice(cause, 'policies.test.error'));
      })
      .finally(() => setTestBusy(false));
  };

  const allowed = allowedDispositions(draft.ruleType);

  return (
    <section className="melodee-panel" aria-labelledby="policy-editor-title">
      <div className="melodee-policy-editor__header">
        <h2 id="policy-editor-title" className="melodee-surface__title">{mode === 'create' ? t('policies.editor.titleCreate') : t('policies.editor.titleEdit')}</h2>
        {mode === 'edit' && rule ? (
          <StatusTag
            kind={rule.enabled ? 'success' : 'neutral'}
            label={rule.enabled ? t('status.enabled') : t('status.disabled')}
            description={
              rule.enabled
                ? t('policies.editor.enabledDescription')
                : t('policies.editor.disabledDescription')
            }
          />
        ) : null}
      </div>

      {isProtected ? (
        <InlineNotification
          kind="info"
          lowContrast
          hideCloseButton
          title={t('policies.editor.protectedNote')}
        />
      ) : null}
      {disabledRule ? (
        <InlineNotification
          kind="info"
          lowContrast
          hideCloseButton
          title={t('policies.editor.disabledNote')}
        />
      ) : null}
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.text}
          onCloseButtonClick={() => setNotice(null)}
        />
      ) : null}
      {notice?.reload ? (
        <div className="melodee-panel__footer">
          <Button kind="secondary" size="sm" onClick={onReload}>
            {t('policies.editor.reload')}
          </Button>
        </div>
      ) : null}

      <TextInput
        id="policy-rule-id"
        labelText={t('policies.editor.name')}
        helperText={
          mode === 'create' ? t('policies.editor.nameHelp') : t('policies.editor.nameReadonlyHelp')
        }
        value={draft.id}
        disabled={formLocked || mode === 'edit'}
        invalid={showValidation && validationErrors.includes('id')}
        invalidText={t('policies.validation.id')}
        onChange={(event) => update({ id: event.target.value })}
      />

      <Select
        id="policy-rule-type"
        labelText={t('policies.editor.type')}
        value={draft.ruleType}
        disabled={formLocked || isProtected}
        helperText={
          isProtected ? t('policies.editor.typeLockedHelp') : t('policies.editor.typeHelp')
        }
        onChange={(event) => setRuleType(event.target.value as PolicyRuleType)}
      >
        {RULE_TYPES.map((ruleType) => (
          <SelectItem key={ruleType} value={ruleType} text={t(`policies.type.${ruleType}`)} />
        ))}
      </Select>

      <h2 className="melodee-surface__title">{t('policies.editor.conditions.title')}</h2>
      <p className="melodee-panel__subtitle">{t('policies.editor.conditions.description')}</p>
      {conditionsLocked ? (
        <InlineNotification
          kind="info"
          lowContrast
          hideCloseButton
          title={t('policies.editor.conditions.locked')}
        />
      ) : (
        <>
          {draft.conditions.length === 0 ? (
            <p className="melodee-panel__subtitle">{t('policies.editor.conditions.empty')}</p>
          ) : null}
          {draft.conditions.map((condition, index) => (
            <div className="melodee-policy-condition" key={`${index}-${condition.field}`}>
              <Select
                id={`policy-condition-field-${index}`}
                labelText={t('policies.editor.conditions.field')}
                hideLabel
                value={condition.field}
                disabled={formLocked}
                onChange={(event) => {
                  const field = event.target.value as PredicateCondition['field'];
                  setCondition(index, {
                    field,
                    value: field === 'release_trait' ? RELEASE_TRAITS[0]! : DISPOSITIONS[0]!,
                  });
                }}
              >
                <SelectItem
                  value="release_trait"
                  text={t('policies.editor.conditions.field.releaseTrait')}
                />
                <SelectItem
                  value="existing_disposition"
                  text={t('policies.editor.conditions.field.existingDisposition')}
                />
              </Select>
              <span className="melodee-policy-condition__operator">
                {condition.field === 'release_trait'
                  ? t('policies.editor.conditions.operator.requiresPresence')
                  : t('policies.editor.conditions.operator.isOneOf')}
              </span>
              <Select
                id={`policy-condition-value-${index}`}
                labelText={t('policies.editor.conditions.value')}
                hideLabel
                value={condition.value}
                disabled={formLocked}
                onChange={(event) => setCondition(index, { value: event.target.value })}
              >
                {condition.field === 'release_trait'
                  ? RELEASE_TRAITS.map((trait) => (
                      <SelectItem key={trait} value={trait} text={t(`policies.trait.${trait}`)} />
                    ))
                  : DISPOSITIONS.map((disposition) => (
                      <SelectItem
                        key={disposition}
                        value={disposition}
                        text={t(`review.disposition.${disposition}`)}
                      />
                    ))}
              </Select>
              <IconButton
                kind="ghost"
                size="md"
                label={t('policies.editor.conditions.remove')}
                disabled={formLocked}
                onClick={() => removeCondition(index)}
              >
                <TrashCan />
              </IconButton>
            </div>
          ))}
          <div className="melodee-panel__footer">
            <Button
              kind="ghost"
              size="sm"
              renderIcon={Add}
              disabled={formLocked}
              onClick={addCondition}
            >
              {t('policies.editor.conditions.add')}
            </Button>
          </div>
          {showValidation && validationErrors.includes('traits') ? (
            <InlineNotification
              kind="error"
              lowContrast
              hideCloseButton
              title={t('policies.validation.traits')}
            />
          ) : null}
        </>
      )}

      <h2 className="melodee-surface__title">{t('policies.editor.actions.title')}</h2>
      <Select
        id="policy-rule-disposition"
        labelText={t('policies.editor.disposition')}
        helperText={t('policies.editor.dispositionHelp')}
        value={draft.disposition}
        disabled={formLocked}
        invalid={showValidation && validationErrors.includes('disposition')}
        invalidText={t('policies.validation.disposition')}
        onChange={(event) => update({ disposition: event.target.value as PolicyDisposition })}
      >
        {allowed.map((disposition) => (
          <SelectItem
            key={disposition}
            value={disposition}
            text={t(`review.disposition.${disposition}`)}
          />
        ))}
      </Select>
      {draft.disposition === 'delete' ? (
        <InlineNotification
          kind="warning"
          lowContrast
          hideCloseButton
          title={t('policies.editor.deleteDispositionWarning')}
        />
      ) : null}
      <TextInput
        id="policy-rule-confidence"
        type="number"
        min={0}
        max={10000}
        step={1}
        labelText={t('policies.editor.confidenceThreshold')}
        helperText={t('policies.editor.confidenceThresholdHelp')}
        value={confidenceText}
        disabled={formLocked}
        invalid={showValidation && validationErrors.includes('confidence')}
        invalidText={t('policies.validation.confidence')}
        onChange={(event) => setConfidenceText(event.target.value)}
      />

      <h2 className="melodee-surface__title">{t('policies.test.title')}</h2>
      <p className="melodee-panel__subtitle">{t('policies.test.description')}</p>
      <TextInput
        id="policy-test-limit"
        type="number"
        min={1}
        max={MAX_TEST_RUN_RELEASES}
        step={1}
        labelText={t('policies.test.limit')}
        value={limitText}
        disabled={busy || testBusy}
        onChange={(event) => setLimitText(event.target.value)}
      />
      <div className="melodee-panel__footer">
        <Button
          kind="secondary"
          size="sm"
          disabled={busy || testBusy}
          onClick={runTest}
        >
          {testBusy ? t('policies.test.running') : t('policies.test.run')}
        </Button>
      </div>

      {testRun ? (
        <div className="melodee-policy-test-results">
          {testRun.destructive || testRun.destructiveImpact > 0 ? (
            <InlineNotification
              kind="warning"
              lowContrast
              hideCloseButton
              title={t('policies.test.destructiveWarning', {
                count: formatNumber(testRun.destructiveImpact),
              })}
              subtitle={t('policies.test.destructiveWarningDetail')}
            />
          ) : null}
          <dl className="melodee-fact-list">
            <div className="melodee-fact">
              <dt>{t('policies.test.evaluated')}</dt>
              <dd>{formatNumber(testRun.evaluated)}</dd>
            </div>
            <div className="melodee-fact">
              <dt>{t('policies.test.matched')}</dt>
              <dd>{formatNumber(testRun.matchedCount)}</dd>
            </div>
            <div className="melodee-fact">
              <dt>{t('policies.test.ruleSetVersion')}</dt>
              <dd>{formatNumber(testRun.ruleSetVersion)}</dd>
            </div>
            <div className="melodee-fact">
              <dt>{t('policies.test.proposedDispositions')}</dt>
              <dd>
                {Object.keys(testRun.proposedDispositionCounts).length === 0 ? (
                  t('common.none')
                ) : (
                  <span className="melodee-policy-disposition-counts">
                    {Object.entries(testRun.proposedDispositionCounts).map(
                      ([disposition, count]) => (
                        <span key={disposition}>
                          <StatusTag
                            kind={dispositionKind(disposition)}
                            label={t(`review.disposition.${disposition}`, {
                              defaultValue: disposition,
                            })}
                          />{' '}
                          {formatNumber(count)}
                        </span>
                      ),
                    )}
                  </span>
                )}
              </dd>
            </div>
          </dl>
          <InlineNotification
            kind="info"
            lowContrast
            hideCloseButton
            title={
              testRun.persisted
                ? t('policies.test.persistedNote')
                : t('policies.test.adHocNote')
            }
          />
          {testRun.sample.length === 0 ? (
            <p className="melodee-panel__subtitle">{t('policies.test.sampleEmpty')}</p>
          ) : (
            <TableContainer title={t('policies.test.sampleCaption')}>
              <Table size="sm" aria-label={t('policies.test.sampleCaption')}>
                <TableHead>
                  <TableRow>
                    <TableHeader>{t('common.release')}</TableHeader>
                    <TableHeader>{t('common.artist')}</TableHeader>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {testRun.sample.map((sample) => (
                    <TableRow key={sample.releaseId}>
                      <TableCell>
                        {/* The sample carries the internal Release identity,
                            which the catalog detail route does not address;
                            link to the catalog filtered by title instead. */}
                        <Link
                          as={RouterLink}
                          to={`/catalog/releases?query=${encodeURIComponent(sample.title)}`}
                        >
                          {sample.title}
                        </Link>
                      </TableCell>
                      <TableCell>{sample.artist ?? t('common.unknown')}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </TableContainer>
          )}
        </div>
      ) : null}

      <div className="melodee-panel__footer melodee-policy-editor__footer">
        <Button kind="ghost" size="md" disabled={busy} onClick={onCancel}>
          {t('actions.cancel')}
        </Button>
        {mode === 'edit' && rule && !isProtected ? (
          <>
            <Button
              kind="danger--tertiary"
              size="md"
              disabled={busy}
              onClick={() => onDelete(rule)}
            >
              {t('actions.delete')}
            </Button>
            <Button
              kind="tertiary"
              size="md"
              disabled={busy}
              onClick={() => {
                setNotice(null);
                onToggleEnabled(rule, !rule.enabled).catch((cause: unknown) => {
                  setNotice(toErrorNotice(cause, 'policies.editor.toggleError'));
                });
              }}
            >
              {rule.enabled ? t('actions.disable') : t('actions.enable')}
            </Button>
          </>
        ) : null}
        <Button
          kind="primary"
          size="md"
          disabled={formLocked}
          onClick={save}
        >
          {mode === 'create' ? t('actions.create') : t('actions.saveChanges')}
        </Button>
      </div>
    </section>
  );
}
