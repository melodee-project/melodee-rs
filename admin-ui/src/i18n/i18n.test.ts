import i18next from "i18next";
import { describe, expect, it } from "vitest";

import i18n, { CULTURES, dirFor, FALLBACK_LOCALE } from "./index";
import { formatDateTime, formatDuration, formatNumber } from "./format";

describe("i18n foundation (ADR-0137)", () => {
  it("preserves the 20-culture inventory", () => {
    expect(CULTURES).toHaveLength(20);
    expect(CULTURES.map((culture) => culture.code)).toContain("fa-IR");
  });

  it("derives RTL only for Arabic and Persian", () => {
    expect(dirFor("ar-SA")).toBe("rtl");
    expect(dirFor("fa-IR")).toBe("rtl");
    expect(dirFor("ar")).toBe("rtl");
    expect(dirFor("en-US")).toBe("ltr");
    expect(dirFor("he-IL")).toBe("ltr");
  });

  it("uses en-US as the fallback locale", () => {
    expect(FALLBACK_LOCALE).toBe("en-US");
    expect(i18n.options.fallbackLng).toContain("en-US");
  });

  it("falls back to English for keys missing from a harvested locale", async () => {
    const deDE = (await import("./locales/de-DE.json")).default;
    i18n.addResourceBundle("de-DE", "translation", deDE, true, true);
    // Admin-console vocabulary has no legacy analog; German falls back.
    const translated = i18n.getFixedT("de-DE");
    expect(translated("nav.artists")).toBe("Künstler");
    expect(translated("queue.quarantineTitle")).toBe("Quarantine");
  });
});

describe("Intl formatting (PRD-I18N-004)", () => {
  it("formats numbers per the active locale", () => {
    expect(formatNumber(1234567)).toBe(
      new Intl.NumberFormat("en-US").format(1234567),
    );
  });

  it("formats datetimes and returns empty for invalid input", () => {
    expect(formatDateTime("2026-08-14T12:00:00Z")).toBeTruthy();
    expect(formatDateTime("not-a-date")).toBe("");
  });

  it("formats durations compactly", () => {
    expect(formatDuration(3_661_000)).toBe("1 h 1 min 1 s");
    expect(formatDuration(-1)).toBe("");
  });
});

describe("plural forms and Intl under non-English locales (PRD-I18N-004, ADR-0137)", () => {
  it("resolves plural categories per locale via Intl.PluralRules", () => {
    // French: singular/plural.
    expect(new Intl.PluralRules("fr-FR").select(1)).toBe("one");
    expect(new Intl.PluralRules("fr-FR").select(2)).toBe("other");
    // Japanese: no grammatical number — every count is 'other'.
    expect(new Intl.PluralRules("ja-JP").select(1)).toBe("other");
    expect(new Intl.PluralRules("ja-JP").select(5)).toBe("other");
    // Russian: the one/few/many split that naive pluralization gets wrong.
    expect(new Intl.PluralRules("ru-RU").select(1)).toBe("one");
    expect(new Intl.PluralRules("ru-RU").select(3)).toBe("few");
    expect(new Intl.PluralRules("ru-RU").select(5)).toBe("many");
    // Arabic: a zero category on top of one/two/few/many.
    expect(new Intl.PluralRules("ar-SA").select(0)).toBe("zero");
    expect(new Intl.PluralRules("ar-SA").select(2)).toBe("two");
  });

  it("resolves i18next plural forms under ru-RU (one/few/many)", async () => {
    // The en-US catalog has no plural keys yet; this synthetic bundle proves
    // the i18next plural machinery against the hardest inventory locale so
    // future plural keys are safe to add in any culture.
    const instance = i18next.createInstance();
    await instance.init({
      lng: "ru-RU",
      resources: {
        "ru-RU": {
          translation: {
            tracks_one: "{{count}} трек",
            tracks_few: "{{count}} трека",
            tracks_many: "{{count}} треков",
            tracks_other: "{{count}} трека",
          },
        },
      },
      keySeparator: false,
      interpolation: { escapeValue: false },
    });
    const t = instance.getFixedT("ru-RU");
    expect(t("tracks", { count: 1 })).toBe("1 трек");
    expect(t("tracks", { count: 3 })).toBe("3 трека");
    expect(t("tracks", { count: 12 })).toBe("12 треков");
  });

  it("formats numbers with per-locale grouping and digits", () => {
    expect(new Intl.NumberFormat("de-DE").format(1234567)).toBe("1.234.567");
    // ar-SA renders Arabic-Indic digits.
    expect(new Intl.NumberFormat("ar-SA").format(1234567)).toBe("١٬٢٣٤٬٥٦٧");
    // fr-FR groups with no-break/narrow-no-break spaces (U+00A0/U+202F).
    const fr = new Intl.NumberFormat("fr-FR").format(1234567);
    expect(fr.replace(/[\u00a0\u202f]/g, " ")).toBe("1 234 567");
    expect(fr).not.toBe("1,234,567");
  });

  it("formats datetimes per non-English locales", () => {
    const instant = new Date("2026-08-14T12:00:00Z");
    const ja = new Intl.DateTimeFormat("ja-JP", { dateStyle: "medium" }).format(
      instant,
    );
    const de = new Intl.DateTimeFormat("de-DE", { dateStyle: "medium" }).format(
      instant,
    );
    expect(ja).toContain("2026");
    expect(de).toContain("2026");
    // Distinct locale structures, neither in the en-US shape.
    const en = new Intl.DateTimeFormat("en-US", { dateStyle: "medium" }).format(
      instant,
    );
    expect(ja).not.toBe(en);
    expect(de).not.toBe(en);
  });
});
