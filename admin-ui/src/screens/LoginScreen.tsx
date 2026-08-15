import {
  Button,
  Column,
  Form,
  Grid,
  Heading,
  InlineNotification,
  PasswordInput,
  Stack,
  TextInput,
} from '@carbon/react';
import { useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { Navigate, useLocation, useNavigate } from 'react-router-dom';

import { ApiError } from '../api/client';
import { useAuth } from '../auth/AuthContext';

/**
 * Cookie sign-in screen (ADR-0139). Submits to the frozen native-v1 cookie
 * endpoint via the auth provider; the server owns the session cookie.
 */
export function LoginScreen() {
  const { t } = useTranslation();
  const { session, signIn } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();

  const [identifier, setIdentifier] = useState('');
  const [password, setPassword] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const from =
    typeof location.state === 'object' && location.state !== null && 'from' in location.state
      ? String((location.state as { from: unknown }).from)
      : '/';

  if (session.kind === 'admin') {
    return <Navigate to={from} replace />;
  }

  const onSubmit = async (event: FormEvent) => {
    event.preventDefault();
    if (!identifier.trim() || !password) {
      setError(!identifier.trim() ? t('auth.usernameRequired') : t('auth.passwordRequired'));
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      await signIn(identifier.trim(), password);
      navigate(from, { replace: true });
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 401) {
        setError(t('auth.invalidCredentials'));
      } else if (cause instanceof ApiError && cause.status === 429) {
        setError(cause.message);
      } else {
        setError(t('auth.serverUnavailable'));
      }
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Grid style={{ minBlockSize: '100vh', alignItems: 'center' }}>
      <Column sm={4} md={{ span: 4, offset: 2 }} lg={{ span: 4, offset: 6 }}>
        <Form onSubmit={(event) => void onSubmit(event)} aria-label={t('auth.signInTitle')}>
          <Stack gap={6}>
            <div>
              <Heading style={{ fontSize: '1.75rem' }}>{t('auth.signInTitle')}</Heading>
              <p style={{ color: 'var(--cds-text-secondary)', marginBlockStart: '0.5rem' }}>
                {t('auth.signInToContinue')}
              </p>
            </div>
            {error ? (
              <InlineNotification
                kind="error"
                lowContrast
                hideCloseButton
                title={t('common.error')}
                subtitle={error}
              />
            ) : null}
            {session.kind === 'anonymous' && from !== '/' ? (
              <InlineNotification
                kind="info"
                lowContrast
                hideCloseButton
                title={t('auth.sessionExpired')}
              />
            ) : null}
            <TextInput
              id="login-identifier"
              labelText={t('auth.usernameOrEmail')}
              value={identifier}
              onChange={(event) => setIdentifier(event.target.value)}
              autoComplete="username"
              autoFocus
            />
            <PasswordInput
              id="login-password"
              labelText={t('auth.password')}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              autoComplete="current-password"
              showPasswordLabel={t('actions.viewDetails')}
              hidePasswordLabel={t('actions.close')}
            />
            <Button type="submit" disabled={submitting}>
              {submitting ? t('auth.signingIn') : t('auth.signIn')}
            </Button>
          </Stack>
        </Form>
      </Column>
    </Grid>
  );
}
