-- PRD-FP-001/PRD-FP-002: durable acoustic identity evidence is queryable.
--
-- Normalization fingerprints are retained in the sealed manifest, but the
-- *conclusion* (this Track's audio has been fingerprinted) and the
-- catalogue-wide comparison key both need to be cheap to look up: novelty
-- evaluation runs once per imported Release and consults every other Release's
-- digests. Additive indexes only; a downgrade leaves the evidence rows intact
-- and simply stops using them.

-- Confirmation lookup: "does this AudioIdentity carry durable acoustic evidence?"
CREATE INDEX IF NOT EXISTS audio_identity_evidence_acoustic_by_identity
    ON audio_identity_evidence (source_audio_identity_id)
    WHERE evidence_kind = 'acoustic_fingerprint';

-- Catalogue-wide acoustic duplicate detection compares digests across Releases.
CREATE INDEX IF NOT EXISTS audio_identity_evidence_acoustic_digest
    ON audio_identity_evidence ((payload ->> 'fingerprint_digest'))
    WHERE evidence_kind = 'acoustic_fingerprint';
