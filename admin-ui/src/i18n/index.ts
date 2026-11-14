import i18n from 'i18next';
import LanguageDetector from 'i18next-browser-languagedetector';
import { initReactI18next } from 'react-i18next';

import enUS from './locales/en-US.json';
import coverage from './coverage.json';

/**
 * The 20-culture inventory preserved from the legacy server (PRD-I18N-001,
 * ADR-0137). en-US is the source and fallback locale; every other locale
 * carries harvested legacy translations for the keys whose semantics match,
 * and falls back to English for admin-console vocabulary with no legacy analog.
 */
export const FALLBACK_LOCALE = 'en-US';

/** Missing source keys are disclosed as preview coverage, never hidden by fallback. */
export function isLocaleComplete(code: string): boolean {
  const entry = (coverage as Record<string, { present: number; total: number }>)[code];
  return entry !== undefined && entry.present === entry.total;
}

export const RTL_LOCALES: ReadonlySet<string> = new Set(['ar-SA', 'fa-IR']);

export interface CultureInfo {
  /** BCP-47 code as used in file names and i18next. */
  code: string;
  /** Endonym shown in the language selector. */
  nativeName: string;
}

export const CULTURES: readonly CultureInfo[] = [
  { code: 'en-US', nativeName: 'English (US)' },
  { code: 'de-DE', nativeName: 'Deutsch' },
  { code: 'es-ES', nativeName: 'Español' },
  { code: 'fr-FR', nativeName: 'Français' },
  { code: 'it-IT', nativeName: 'Italiano' },
  { code: 'ja-JP', nativeName: '日本語' },
  { code: 'pt-BR', nativeName: 'Português (Brasil)' },
  { code: 'ru-RU', nativeName: 'Русский' },
  { code: 'zh-CN', nativeName: '简体中文' },
  { code: 'ar-SA', nativeName: 'العربية' },
  { code: 'nl-NL', nativeName: 'Nederlands' },
  { code: 'pl-PL', nativeName: 'Polski' },
  { code: 'tr-TR', nativeName: 'Türkçe' },
  { code: 'id-ID', nativeName: 'Bahasa Indonesia' },
  { code: 'ko-KR', nativeName: '한국어' },
  { code: 'vi-VN', nativeName: 'Tiếng Việt' },
  { code: 'fa-IR', nativeName: 'فارسی' },
  { code: 'uk-UA', nativeName: 'Українська' },
  { code: 'cs-CZ', nativeName: 'Čeština' },
  { code: 'sv-SE', nativeName: 'Svenska' },
];

const SUPPORTED = CULTURES.map((culture) => culture.code);

/** Text direction for a locale; Arabic and Persian render right-to-left. */
export function dirFor(lng: string): 'rtl' | 'ltr' {
  const normalized = lng.toLowerCase();
  const base = normalized.split('-')[0] ?? normalized;
  return RTL_LOCALES.has(lng) || base === 'ar' || base === 'fa' ? 'rtl' : 'ltr';
}

type LocaleModule = { default: Record<string, string> };

// Lazy static imports keep every locale code-split; Vite resolves the exact
// file set at build time, so a typo'd culture code fails the build, not users.
const LOCALE_LOADERS: Record<string, () => Promise<LocaleModule>> = {
  'de-DE': () => import('./locales/de-DE.json'),
  'es-ES': () => import('./locales/es-ES.json'),
  'fr-FR': () => import('./locales/fr-FR.json'),
  'it-IT': () => import('./locales/it-IT.json'),
  'ja-JP': () => import('./locales/ja-JP.json'),
  'pt-BR': () => import('./locales/pt-BR.json'),
  'ru-RU': () => import('./locales/ru-RU.json'),
  'zh-CN': () => import('./locales/zh-CN.json'),
  'ar-SA': () => import('./locales/ar-SA.json'),
  'nl-NL': () => import('./locales/nl-NL.json'),
  'pl-PL': () => import('./locales/pl-PL.json'),
  'tr-TR': () => import('./locales/tr-TR.json'),
  'id-ID': () => import('./locales/id-ID.json'),
  'ko-KR': () => import('./locales/ko-KR.json'),
  'vi-VN': () => import('./locales/vi-VN.json'),
  'fa-IR': () => import('./locales/fa-IR.json'),
  'uk-UA': () => import('./locales/uk-UA.json'),
  'cs-CZ': () => import('./locales/cs-CZ.json'),
  'sv-SE': () => import('./locales/sv-SE.json'),
};

const loadedLocales = new Set<string>([FALLBACK_LOCALE]);

async function ensureLocale(lng: string): Promise<void> {
  if (loadedLocales.has(lng) || i18n.hasResourceBundle(lng, 'translation')) {
    loadedLocales.add(lng);
    return;
  }
  const loader = LOCALE_LOADERS[lng];
  if (!loader) {
    return;
  }
  const module = await loader();
  // A later language switch may have raced us; adding the bundle is
  // idempotent, so the worst case is a redundant update.
  i18n.addResourceBundle(lng, 'translation', module.default, true, true);
  loadedLocales.add(lng);
}

function applyDocumentDirection(lng: string): void {
  if (typeof document === 'undefined') {
    return;
  }
  document.documentElement.lang = lng;
  document.documentElement.dir = dirFor(lng);
}

/** Resolves once i18next has initialized; tests and boot code can await it. */
export const i18nReady = i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: {
      [FALLBACK_LOCALE]: { translation: enUS },
    },
    fallbackLng: FALLBACK_LOCALE,
    supportedLngs: SUPPORTED,
    load: 'currentOnly',
    // Catalog keys are flat, dot-namespaced strings ('nav.overview'); treat
    // them literally instead of as nested paths.
    keySeparator: false,
    interpolation: {
      // React already escapes rendered output.
      escapeValue: false,
    },
    detection: {
      order: ['localStorage', 'navigator'],
      caches: ['localStorage'],
      lookupLocalStorage: 'melodee.locale',
      // Browser detection reports bare codes like "en" or "de"; map them onto
      // the exact inventory (nonExplicitSupportedLngs is broken with
      // load:'currentOnly' in i18next 26 — it empties the resolve hierarchy).
      convertDetectedLanguage: (detected: string): string => {
        if (SUPPORTED.includes(detected)) {
          return detected;
        }
        const base = detected.split('-')[0] ?? detected;
        return (
          SUPPORTED.find((code) => code.split('-')[0] === base) ?? FALLBACK_LOCALE
        );
      },
    },
  });

i18n.on('languageChanged', (lng) => {
  applyDocumentDirection(lng);
  void ensureLocale(lng).catch(() => {
    // Missing or unloadable locale files fall back to en-US per ADR-0137;
    // there is nothing actionable in the UI beyond the fallback itself.
  });
});

// init() fires the initial languageChanged synchronously — before the
// listener above is registered — so a persisted/detected non-English culture
// would boot on English strings until re-selected. Kick off the detected
// locale's bundle explicitly (ensureLocale is idempotent and no-ops for the
// inline en-US source).
void ensureLocale(i18n.language || FALLBACK_LOCALE).catch(() => {
  // Same fallback semantics as the languageChanged handler.
});

// Boot: `language` is the detected culture (localStorage/navigator);
// `resolvedLanguage` stays at the fallback until the lazy bundle lands, so
// it must not drive the document direction at startup — otherwise a
// pre-seeded RTL culture boots LTR until the language is re-selected.
applyDocumentDirection(i18n.language || i18n.resolvedLanguage || FALLBACK_LOCALE);

export default i18n;
