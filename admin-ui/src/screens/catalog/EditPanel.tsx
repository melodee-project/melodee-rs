import {
  Button,
  Form,
  InlineNotification,
  NumberInput,
  TextInput,
} from '@carbon/react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { ReleaseMetadataPatch, ArtistMetadataPatch, TrackMetadataPatch } from '../../api/types';

/** Result of a successful metadata save. */
interface EditNotice {
  kind: 'success' | 'error';
  text: string;
}

/** Generic form field for a nullable string value. */
function StringField({
  label,
  name,
  initial,
}: {
  label: string;
  name: string;
  initial: string | null | undefined;
}): React.ReactElement {
  const [value, setValue] = useState(initial ?? '');

  return (
    <TextInput
      id={`edit-${String(name)}`}
      labelText={label}
      value={value}
      onChange={(event) => setValue(event.target.value)}
      clearable
    />
  );
}

/** Generic form field for a nullable number value. */
function NumberField({
  label,
  name,
  initial,
}: {
  label: string;
  name: string;
  initial: number | null | undefined;
}): React.ReactElement {
  const [value, setValue] = useState(initial);

  return (
    <NumberInput
      id={`edit-${String(name)}`}
      label={label}
      value={value}
      onChange={(event) => setValue(Number(event.target.value) || undefined)}
      hideControlButtons
      min={0}
    />
  );
}

/** Form props supplied per entity kind. */
interface FormFieldConfig {
  fields: Array<{
    name: string;
    label: string;
    type: 'string' | 'number';
    value: unknown;
  }>;
  /** Build the patch object from current form state. */
  buildPatch(state: Record<string, unknown>): ReleaseMetadataPatch | ArtistMetadataPatch | TrackMetadataPatch;
}

/**
 * EditPanel — an inline collapsible edit form for catalog entity metadata.
 *
 * The panel is closed by default and opens when the caller passes `editing={true}`.
 * On save it calls `onSave(patch)`, shows a brief notification, and calls `refresh`.
 */
export function EditPanel({
  title,
  fields,
  buildPatch,
  onSave,
  refresh,
}: {
  title: string;
  fields: FormFieldConfig['fields'];
  buildPatch: FormFieldConfig['buildPatch'];
  onSave: (patch: ReleaseMetadataPatch | ArtistMetadataPatch | TrackMetadataPatch) => Promise<void>;
  refresh: () => void;
}): React.ReactElement {
  const { t } = useTranslation();
  const [localValues, setLocalValues] = useState<Record<string, unknown>>(() => {
    const initial: Record<string, unknown> = {};
    for (const field of fields) {
      initial[field.name] = field.value;
    }
    return initial;
  });
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<EditNotice | null>(null);

  const handleSave = async () => {
    const patch = buildPatch(localValues);
    // Strip entries whose value is undefined (the backend treats missing keys as "do not change")
    const filtered = Object.fromEntries(
      Object.entries(patch).filter(([, v]) => v !== undefined),
    );
    setSaving(true);
    setNotice(null);
    try {
      await onSave(filtered as ReleaseMetadataPatch | ArtistMetadataPatch | TrackMetadataPatch);
      refresh();
      setNotice({ kind: 'success', text: t('catalog.edit.saved') });
    } catch (cause) {
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
          field.type === 'number' ? (
            <NumberField key={field.name} label={field.label} name={field.name} initial={field.value as number | null | undefined} />
          ) : (
            <StringField key={field.name} label={field.label} name={field.name} initial={field.value as string | null | undefined} />
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
