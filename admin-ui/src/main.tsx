import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import App from './App';
import { AuthProvider } from './auth/AuthContext';
import { ThemeProvider } from './theme/ThemeProvider';

// i18n must initialize before first render; side-effect import is the
// sanctioned pattern for i18next (ADR-0137).
import './i18n';
import './theme/index.scss';
// Melodee layer order matters: tokens define values, surfaces/shell/screens
// consume them (ADR-0184).
import './theme/tokens.css';
import './theme/surfaces.css';
import './theme/shell.css';
import './theme/screens.css';

const rootElement = document.getElementById('root');
if (!rootElement) {
  throw new Error('missing #root element');
}

createRoot(rootElement).render(
  <StrictMode>
    <ThemeProvider>
      <AuthProvider>
        <App />
      </AuthProvider>
    </ThemeProvider>
  </StrictMode>,
);
