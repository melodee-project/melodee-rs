import { lazy } from 'react';
import { BrowserRouter, Navigate, Route, Routes } from 'react-router-dom';

import { RequireAuth } from './auth/RequireAuth';
import { ErrorState } from './components/ErrorState';
import { LoginScreen } from './screens/LoginScreen';
import { PlaceholderScreen } from './screens/PlaceholderScreen';
import { AppShell } from './shell/AppShell';

// Route-level code splitting (Phase 10 W4-5 bundle budget): every
// authenticated screen is a lazy chunk so the initial bundle carries only
// the shell, auth, and login. LoginScreen stays eager — it is the entry
// route and must not wait on a second round trip.
const ArtistDetailScreen = lazy(() =>
  import('./screens/catalog/ArtistDetailScreen').then((m) => ({ default: m.ArtistDetailScreen })),
);
const ArtistsScreen = lazy(() =>
  import('./screens/catalog/ArtistsScreen').then((m) => ({ default: m.ArtistsScreen })),
);
const ReleaseDetailScreen = lazy(() =>
  import('./screens/catalog/ReleaseDetailScreen').then((m) => ({ default: m.ReleaseDetailScreen })),
);
const ReleasesScreen = lazy(() =>
  import('./screens/catalog/ReleasesScreen').then((m) => ({ default: m.ReleasesScreen })),
);
const TrackDetailScreen = lazy(() =>
  import('./screens/catalog/TrackDetailScreen').then((m) => ({ default: m.TrackDetailScreen })),
);
const TracksScreen = lazy(() =>
  import('./screens/catalog/TracksScreen').then((m) => ({ default: m.TracksScreen })),
);
const IdentityReviewScreen = lazy(() =>
  import('./screens/identity/IdentityReviewScreen').then((m) => ({ default: m.IdentityReviewScreen })),
);
const OnboardingWizard = lazy(() =>
  import('./screens/onboarding/OnboardingWizard').then((m) => ({ default: m.OnboardingWizard })),
);
const OverviewScreen = lazy(() =>
  import('./screens/overview/OverviewScreen').then((m) => ({ default: m.OverviewScreen })),
);
const PoliciesScreen = lazy(() =>
  import('./screens/policies/PoliciesScreen').then((m) => ({ default: m.PoliciesScreen })),
);
const HistoryScreen = lazy(() =>
  import('./screens/processing/HistoryScreen').then((m) => ({ default: m.HistoryScreen })),
);
const ProcessingQueueScreen = lazy(() =>
  import('./screens/processing/ProcessingQueueScreen').then((m) => ({ default: m.ProcessingQueueScreen })),
);
const QuarantineScreen = lazy(() =>
  import('./screens/processing/QuarantineScreen').then((m) => ({ default: m.QuarantineScreen })),
);
const ReleaseReviewScreen = lazy(() =>
  import('./screens/review/ReleaseReviewScreen').then((m) => ({ default: m.ReleaseReviewScreen })),
);
const ReviewQueueScreen = lazy(() =>
  import('./screens/review/ReviewQueueScreen').then((m) => ({ default: m.ReviewQueueScreen })),
);
const SettingsScreen = lazy(() =>
  import('./screens/settings/SettingsScreen').then((m) => ({ default: m.SettingsScreen })),
);
const SystemWorkersScreen = lazy(() =>
  import('./screens/system/SystemWorkersScreen').then((m) => ({ default: m.SystemWorkersScreen })),
);
const UsersScreen = lazy(() =>
  import('./screens/users/UsersScreen').then((m) => ({ default: m.UsersScreen })),
);

/**
 * Router for the admin SPA. basename is '/admin' (ADR-0138 static serving).
 * Every IA entry routes to a placeholder until its wave lands; /login is the
 * only public route. Unknown in-app paths render the localized, themed
 * not-found state inside the shell (PRD-WEB-002 `/Error` replacement).
 */

export default function App() {
  return (
    <BrowserRouter basename="/admin">
      <Routes>
        <Route path="/login" element={<LoginScreen />} />
        <Route
          element={
            <RequireAuth>
              <AppShell />
            </RequireAuth>
          }
        >
          <Route index element={<OverviewScreen />} />
          <Route path="processing" element={<ProcessingQueueScreen />} />
          <Route path="processing/review" element={<ReviewQueueScreen />} />
          <Route path="processing/review/:workItemId" element={<ReleaseReviewScreen />} />
          <Route path="processing/quarantine" element={<QuarantineScreen />} />
          <Route path="processing/history" element={<HistoryScreen />} />
          <Route path="catalog/artists" element={<ArtistsScreen />} />
          <Route path="catalog/artists/:id" element={<ArtistDetailScreen />} />
          <Route path="catalog/releases" element={<ReleasesScreen />} />
          <Route path="catalog/releases/:id" element={<ReleaseDetailScreen />} />
          <Route path="catalog/tracks" element={<TracksScreen />} />
          <Route path="catalog/tracks/:id" element={<TrackDetailScreen />} />
          <Route path="catalog/identity" element={<IdentityReviewScreen />} />
          <Route path="policies/releases" element={<PoliciesScreen />} />
          <Route path="policies/matching" element={<PlaceholderScreen titleKey="policies.matchingRulesTitle" subtitleKey="policies.subtitle" />} />
          <Route path="system/storage" element={<PlaceholderScreen titleKey="system.storageTitle" subtitleKey="system.subtitle" />} />
          <Route path="system/workers" element={<SystemWorkersScreen />} />
          <Route path="system/search" element={<PlaceholderScreen titleKey="system.searchTitle" />} />
          <Route path="system/users" element={<UsersScreen />} />
          <Route path="system/settings" element={<SettingsScreen />} />
          <Route path="system/diagnostics" element={<PlaceholderScreen titleKey="system.diagnosticsTitle" subtitleKey="system.subtitle" />} />
          <Route path="onboarding" element={<OnboardingWizard />} />
          <Route path="*" element={<ErrorState kind="notFound" />} />
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </BrowserRouter>
  );
}
