-- notification templates and delivery audit (ADR-0134 §4,
-- PRD-NOTIF-001..003).
--
-- Additive. A binary downgrade leaves both tables intact; schema rollback
-- restores a verified pre-migration backup into a new database.
--
-- Legacy parity (evidence:
-- legacy C# behavior audit "Email settings"):
-- the legacy server seeded only `email.resetPassword.subject` ("Reset your
-- Melodee password"); both legacy body-template keys were dead (never seeded,
-- never consumed). The seeds below therefore carry that one captured subject
-- plus minimal, neutral `{{placeholder}}` bodies written for this rewrite.
-- Templates are operator-manageable rows: updating or deleting a seeded row
-- is a supported operator action, never reverted by later migrations.

CREATE TABLE notification_templates (
    -- Dotted event key (e.g. user.password_reset). Never an identity for
    -- anything outside this table; admins may add keys for future events.
    template_key text NOT NULL,
    -- BCP-47-ish locale tag (en, en-US, pt-BR,...). The narrow shape is
    -- deliberate: localization owns any widening.
    locale text NOT NULL,
    subject text,
    body_text text,
    body_html text,
    -- Bumped on every upsert so operators can see template churn.
    version integer NOT NULL DEFAULT 1,
    updated_at timestamptz NOT NULL DEFAULT now(),

    PRIMARY KEY (template_key, locale),
    CONSTRAINT notification_templates_key_check CHECK (
        btrim(template_key) <> '' AND char_length(template_key) <= 100
    ),
    CONSTRAINT notification_templates_locale_check CHECK (
        char_length(locale) <= 20
        AND locale ~ '^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$'
    ),
    CONSTRAINT notification_templates_subject_check CHECK (
        subject IS NULL OR char_length(subject) <= 500
    ),
    CONSTRAINT notification_templates_body_text_size_check CHECK (
        body_text IS NULL OR octet_length(body_text) <= 65536
    ),
    CONSTRAINT notification_templates_body_html_size_check CHECK (
        body_html IS NULL OR octet_length(body_html) <= 65536
    ),
    CONSTRAINT notification_templates_version_check CHECK (version >= 1)
);

-- Default `en` templates for the wired events. Neutral wording;
-- user-visible text lives here (not in code) so localization can
-- translate it (PRD-NOTIF-003).
INSERT INTO notification_templates (template_key, locale, subject, body_text, body_html) VALUES
    (
        'user.password_reset',
        'en',
        'Reset your Melodee password',
        'Hello,

Use this link to reset your Melodee password:

{{resetUrl}}

If you did not request a password reset, you can ignore this message.
',
        '<p>Hello,</p><p>Use this link to reset your Melodee password:</p><p><a href="{{resetUrl}}">{{resetUrl}}</a></p><p>If you did not request a password reset, you can ignore this message.</p>'
    ),
    (
        'share.created',
        'en',
        'Your new Melodee share',
        'Hello,

A share was created for your Melodee account:

{{shareUrl}}

If you did not create this share, sign in and review your shares.
',
        '<p>Hello,</p><p>A share was created for your Melodee account:</p><p><a href="{{shareUrl}}">{{shareUrl}}</a></p><p>If you did not create this share, sign in and review your shares.</p>'
    ),
    (
        'request.fulfilled',
        'en',
        'Your Melodee request was fulfilled',
        'Hello,

Your Melodee request was fulfilled:

{{description}}
',
        '<p>Hello,</p><p>Your Melodee request was fulfilled:</p><p>{{description}}</p>'
    )
ON CONFLICT (template_key, locale) DO NOTHING;

-- Append-only delivery audit (ADR-0134 §4): one row per attempted or
-- intentionally skipped delivery. Never stores subjects, bodies, or any
-- secret material (PRD-NOTIF-002); retention is bounded in code
-- (`NotificationDeliveryStore::purge_older_than`).
CREATE TABLE notification_deliveries (
    id uuid PRIMARY KEY,
    template_key text NOT NULL,
    recipient text NOT NULL,
    status text NOT NULL,
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),

    CONSTRAINT notification_deliveries_key_check CHECK (
        btrim(template_key) <> '' AND char_length(template_key) <= 100
    ),
    CONSTRAINT notification_deliveries_recipient_check CHECK (
        btrim(recipient) <> '' AND char_length(recipient) <= 320
    ),
    CONSTRAINT notification_deliveries_status_check CHECK (
        status IN ('sent', 'failed', 'skipped')
    ),
    CONSTRAINT notification_deliveries_error_check CHECK (
        error IS NULL OR char_length(error) <= 2000
    )
);

-- list_recent / retention purge both order or range-scan by creation time.
CREATE INDEX notification_deliveries_recent
    ON notification_deliveries (created_at DESC, id DESC);
