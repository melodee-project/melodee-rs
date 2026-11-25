import {
  Button,
  InlineNotification,
  Modal,
  Select,
  SelectItem,
  TextArea,
  TextInput,
} from '@carbon/react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { reviewSources, saveReviewCorrection, searchReviewMatches } from '../../api/admin';
import type {
  ReviewCorrection,
  ReviewManifestSummary,
  ReviewMatch,
  WorkItemEvent,
} from '../../api/types';

interface Props {
  id: string;
  manifest: ReviewManifestSummary;
  mode: 'edit' | 'identify';
  retarget: boolean;
  timeline: WorkItemEvent[];
  onClose: () => void;
  onSaved: () => void;
}

function findings(events: WorkItemEvent[]) {
  const event = events.filter((e) => e.reasonCode === 'validation.needs_review').at(-1);
  const data = event?.metadata;
  if (!data || typeof data !== 'object' || !('findings' in data) || !Array.isArray(data.findings))
    return [];
  return data.findings.flatMap((f: unknown) => {
    if (!f || typeof f !== 'object' || !('message' in f) || typeof f.message !== 'string')
      return [];
    return ['file' in f && typeof f.file === 'string' ? `${f.file} — ${f.message}` : f.message];
  });
}

export function ReviewMetadataDialog({
  id,
  manifest,
  mode,
  retarget,
  timeline,
  onClose,
  onSaved,
}: Props) {
  const { t } = useTranslation();
  const identify = mode === 'identify';
  const [title, setTitle] = useState(manifest.releaseTitle ?? '');
  const [artists, setArtists] = useState(manifest.releaseArtists.join('\n'));
  const [queryTitle, setQueryTitle] = useState(title);
  const [queryArtist, setQueryArtist] = useState(manifest.releaseArtists[0] ?? '');
  const [provider, setProvider] = useState('melodee');
  const [sources, setSources] = useState([{ id: 'melodee', name: 'Melodee' }]);
  const [results, setResults] = useState<{
    matches: ReviewMatch[];
    cacheKey: string | null;
  } | null>(null);
  const [choice, setChoice] = useState<ReviewMatch | null>(null);
  const [searching, setSearching] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [tracks, setTracks] = useState(() =>
    manifest.tracks.map((track) => ({
      index: track.index,
      title: track.title ?? '',
      disc: String(track.disc ?? ''),
      number: String(track.number ?? ''),
    })),
  );
  useEffect(() => {
    let active = true;
    if (identify && !retarget)
      reviewSources(id)
        .then((items) => {
          if (active) setSources(items);
        })
        .catch((e: Error) => {
          if (active) setError(e.message);
        });
    return () => {
      active = false;
    };
  }, [id, identify, retarget]);

  const resetResults = () => {
    setChoice(null);
    setResults(null);
    setError('');
  };
  const search = async () => {
    resetResults();
    setSearching(true);
    try {
      setResults(await searchReviewMatches(id, provider, queryArtist, queryTitle));
    } catch (e) {
      setError(String(e));
    } finally {
      setSearching(false);
    }
  };
  const editTrack = (index: number, field: 'title' | 'disc' | 'number', value: string) => {
    setTracks((rows) =>
      rows.map((row) => (row.index === index ? { ...row, [field]: value } : row)),
    );
  };
  const chosenArtists = (choice?.artists.length ? choice.artists : artists.split('\n'))
    .map((a) => a.trim())
    .filter(Boolean);
  const validNumber = (value: string) =>
    !value || (/^\d+$/.test(value) && Number(value) >= 1 && Number(value) <= 65535);
  const valid =
    retarget ||
    ((choice?.title ?? title).trim() &&
      chosenArtists.length &&
      tracks.every((r) => r.title.trim() && validNumber(r.disc) && validNumber(r.number)));
  const disabled = searching || saving;
  const save = async () => {
    if (disabled || !valid || (identify && !choice)) return;
    setSaving(true);
    setError('');
    const changes: ReviewCorrection['tracks'] = tracks.flatMap((r, i) => {
      const original = manifest.tracks[i];
      const disc = r.disc ? Number(r.disc) : null;
      const number = r.number ? Number(r.number) : null;
      if (
        original &&
        r.title === original.title &&
        disc === (original.disc ?? null) &&
        number === (original.number ?? null)
      )
        return [];
      return [{ index: r.index, title: r.title, disc, number }];
    });
    try {
      await saveReviewCorrection(id, {
        expectedHash: manifest.manifestHash,
        title: choice?.title ?? title,
        artists: chosenArtists,
        tracks: retarget ? [] : changes,
        selection: choice
          ? { provider, externalId: choice.externalId, cacheKey: results?.cacheKey ?? null }
          : null,
      });
      onSaved();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  };
  const notes = findings(timeline);
  const prior = timeline
    .filter((event) => event.reasonCode === 'review.identified')
    .at(-1)?.metadata;
  const previousMatch =
    prior && typeof prior === 'object' && 'provider' in prior && 'externalId' in prior
      ? `${String(prior.provider)} · ${String(prior.externalId)}`
      : null;
  return (
    <Modal
      open
      size="lg"
      modalHeading={t(identify ? 'review.metadata.identifyHeading' : 'review.metadata.editHeading')}
      preventCloseOnClickOutside
      primaryButtonText={t(identify ? 'review.metadata.identify' : 'review.metadata.save')}
      secondaryButtonText={t('actions.cancel')}
      primaryButtonDisabled={disabled || !valid || (identify && !choice)}
      onRequestSubmit={() => void save()}
      onRequestClose={() => {
        if (!saving) onClose();
      }}
    >
      <div className="melodee-review-metadata">
        {error && <InlineNotification kind="error" lowContrast hideCloseButton title={error} />}
        {identify && (
          <>
            <Select
              id="review-provider"
              labelText={t('review.metadata.provider')}
              value={provider}
              disabled={disabled}
              onChange={(e) => {
                resetResults();
                setProvider(e.target.value);
              }}
            >
              {sources.map((s) => (
                <SelectItem key={s.id} value={s.id} text={s.name} />
              ))}
            </Select>
            <TextInput
              id="review-query-artist"
              labelText={t('review.metadata.artist')}
              value={queryArtist}
              disabled={disabled}
              onChange={(e) => {
                resetResults();
                setQueryArtist(e.target.value);
              }}
            />
            <TextInput
              id="review-query-title"
              labelText={t('review.metadata.release')}
              value={queryTitle}
              disabled={disabled}
              onChange={(e) => {
                resetResults();
                setQueryTitle(e.target.value);
              }}
            />
            <Button
              kind="tertiary"
              disabled={disabled || !queryTitle.trim()}
              onClick={() => void search()}
            >
              {t(searching ? 'review.metadata.searching' : 'review.metadata.search')}
            </Button>
            {results && (
              <fieldset disabled={disabled}>
                <legend>{t('review.metadata.matches')}</legend>
                {results.matches.length === 0 && (
                  <p role="status">{t('review.metadata.noMatches')}</p>
                )}
                {results.matches.map((r) => (
                  <label className="melodee-match-option" key={r.externalId}>
                    <input
                      type="radio"
                      name="review-match"
                      checked={choice?.externalId === r.externalId}
                      onChange={() => setChoice(r)}
                    />
                    <span>
                      {r.title} · {r.artists.join(', ')} {r.date} <small>{r.externalId}</small>
                    </span>
                  </label>
                ))}
              </fieldset>
            )}
            {choice && (
              <p>{t(provider === 'melodee' ? 'review.metadata.merge' : 'review.metadata.add')}</p>
            )}
          </>
        )}
        {previousMatch && !identify && (
          <p>{t('review.metadata.previous', { match: previousMatch })}</p>
        )}
        {!identify && (
          <TextInput
            disabled={saving}
            id="review-edit-title"
            labelText={t('review.metadata.release')}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
        )}
        {(!identify || (choice && !choice.artists.length && !retarget)) && (
          <TextArea
            disabled={saving}
            id="review-edit-artists"
            labelText={t('review.metadata.artists')}
            rows={2}
            value={artists}
            onChange={(e) => setArtists(e.target.value)}
          />
        )}
        {!retarget && (
          <>
            {notes.length > 0 && (
              <section>
                <h3>{t('review.metadata.findings')}</h3>
                <ul>
                  {notes.map((s, i) => (
                    <li key={i}>{s}</li>
                  ))}
                </ul>
              </section>
            )}
            {tracks.map((track, i) => (
              <fieldset key={track.index} className="melodee-review-track-fields">
                <legend>{manifest.tracks[i]?.filePath}</legend>
                {(['title', 'disc', 'number'] as const).map((field) => (
                  <TextInput
                    key={field}
                    id={`review-track-${track.index}-${field}`}
                    labelText={t(`review.metadata.${field}`)}
                    value={track[field]}
                    disabled={saving}
                    onChange={(e) => editTrack(track.index, field, e.target.value)}
                  />
                ))}
              </fieldset>
            ))}
          </>
        )}
      </div>
    </Modal>
  );
}
