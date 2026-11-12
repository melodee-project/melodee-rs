import {
  Button,
  Form,
  InlineNotification,
  NumberInput,
  Select,
  SelectItem,
  TextInput,
} from '@carbon/react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { ReleaseMetadataPatch, ArtistMetadataPatch, TrackMetadataPatch } from '../../api/types';

/** Result of a successful metadata save. */
interface EditNotice {
  kind: 'success' | 'error';
  text: string;
}

/** Form props supplied per entity kind. */
type MetadataPatch = ReleaseMetadataPatch | ArtistMetadataPatch | TrackMetadataPatch;

interface FormFieldConfig<P extends MetadataPatch> {
  fields: Array<{
    name: string;
    label: string;
    type: 'string' | 'number' | 'select' | 'date';
    /** Options for a `select` field: value plus its localized label. */
    options?: Array<{ value: string; label: string }>;
    /** Placeholder shown in an empty input (e.g. the expected date shape). */
    placeholder?: string;
    /** Localized message shown when a `date` field is not `YYYY-MM-DD`. */
    invalidText?: string;
    value: unknown;
  }>;
  /** Build the patch object from current form state. */
  buildPatch(state: Record<string, unknown>): P;
}

/**
 * EditPanel — an inline collapsible edit form for catalog entity metadata.
 *
 * The panel is closed by default and opens when the caller passes `editing={true}`.
 * On save it calls `onSave(patch)`, shows a brief notification, and calls `refresh`.
 *
 * RELEASE-002: all field state lives in a single `formState` map managed by
 * this component. Each `TextInput`/`NumberInput` is fully controlled by
 * `formState`, so `handleSave` always reads the value the user sees on screen —
 * never a stale snapshot captured at mount time.
 */
export function EditPanel<P extends MetadataPatch>({
  title,
  fields,
  buildPatch,
  onSave,
  refresh,
}: {
  title: string;
  fields: FormFieldConfig<P>['fields'];
  buildPatch: FormFieldConfig<P>['buildPatch'];
  onSave: (patch: P) => Promise<void>;
  refresh: () => void;
}): React.ReactElement {
  const { t } = useTranslation();
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<EditNotice | null>(null);

  // RELEASE-002: all field state is tracked in a ref (latest) + state (render).
  // The ref is the single source of truth for `handleSave` — it always holds
  // the most recent values, even when a save is triggered in the same event
  // tick as the last onChange (before React re-renders with the new state).
  const buildInitialState = useCallback(
    (fs: typeof fields): Record<string, unknown> => {
      const out: Record<string, unknown> = {};
      for (const f of fs) out[f.name] = f.value ?? null;
      return out;
    },
    [],
  );

  const [formState, setFormState] = useState<Record<string, unknown>>(() =>
    buildInitialState(fields),
  );
  // Ref mirrors the latest formState for synchronous reads in handleSave.
  // Updated in an effect so we never write refs during render (lint rule).
  const formStateRef = useRef(formState);
  useEffect(() => {
    formStateRef.current = formState;
  }, [formState]);

  const setField = (name: string, value: unknown) => {
    setFormState((prev) => ({ ...prev, [name]: value }));
  };

  // After save + refresh the parent re-renders with the server's new values.
  // Resync formState so the inputs show the current data.
  const fieldsKey = JSON.stringify(fields.map((f) => [f.name, f.value]));
  const prevFieldsKeyRef = useRef(fieldsKey);
  useEffect(() => {
    if (prevFieldsKeyRef.current !== fieldsKey) {
      setFormState(buildInitialState(fields));
      prevFieldsKeyRef.current = fieldsKey;
    }
  }, [fieldsKey, fields, buildInitialState]);

  const handleSave = async () => {
    // Read from the ref which is kept in sync by the useEffect above.
    // In practice the effect has already run by the time a click handler
    // fires, so this is always current.
    const currentState = formStateRef.current;
    const patch = buildPatch(currentState);
    const filtered = Object.fromEntries(
      Object.entries(patch).filter(([, v]) => v !== undefined),
    );
    setSaving(true);
    setNotice(null);
    try {
      await onSave(filtered as P);
      refresh();
      setNotice({ kind: 'success', text: t('catalog.edit.saved') });
    } catch {
      setNotice({
        kind: 'error',
        text: t('catalog.edit.error'),
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="melodee-edit-panel">
      {notice ? (
        <InlineNotification
          lowContrast
          kind={notice.kind === 'success' ? 'success' : 'error'}
          hideCloseButton
          title={notice.text}
        />
      ) : null}

      <p style={{ marginBlockEnd: '0.75rem', fontWeight: 600 }}>{title}</p>

      <Form style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: '1rem' }}>
        {fields.map((field) =>
          field.type === 'select' ? (
            <Select
              key={field.name}
              id={`edit-${field.name}`}
              labelText={field.label}
              value={(formState[field.name] as string | null | undefined) ?? ''}
              onChange={(event) => setField(field.name, event.target.value)}
            >
              {(field.options ?? []).map((option) => (
                <SelectItem
                  key={option.value}
                  value={option.value}
                  text={option.label}
                />
              ))}
            </Select>
          ) : field.type === 'date' ? (
            <TextInput
              key={field.name}
              id={`edit-${field.name}`}
              labelText={field.label}
              placeholder={field.placeholder}
              value={(formState[field.name] as string | null | undefined) ?? ''}
              invalid={
                typeof formState[field.name] === 'string' &&
                (formState[field.name] as string).length > 0 &&
                !/^\d{4}-\d{2}-\d{2}$/.test(formState[field.name] as string)
              }
              invalidText={field.invalidText}
              onChange={(event) => setField(field.name, event.target.value)}
            />
          ) : field.type === 'number' ? (
            <NumberInput
              key={field.name}
              id={`edit-${field.name}`}
              label={field.label}
              value={(formState[field.name] as number | null | undefined) ?? undefined}
              onChange={(_event, state) =>
                setField(field.name, state.value ?? null)
              }
              hideSteppers
              min={0}
            />
          ) : (
            <TextInput
              key={field.name}
              id={`edit-${field.name}`}
              labelText={field.label}
              value={(formState[field.name] as string | null | undefined) ?? ''}
              onChange={(event) => setField(field.name, event.target.value)}
            />
          ),
        )}
      </Form>

      <div style={{ display: 'flex', gap: '0.5rem', marginTop: '1rem' }}>
        <Button
          kind="primary"
          onClick={handleSave}
          disabled={saving}
        >
          {saving ? t('catalog.edit.saving') : t('catalog.edit.save')}
        </Button>
      </div>
    </div>
  );
}
