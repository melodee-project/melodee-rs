#!/usr/bin/env python3
"""Harvest legacy Blazor resx translations as seeds for the admin SPA catalog.

Phase 10 W0-3 (PRD-I18N-001, ADR-0137): the admin SPA preserves the legacy
20-culture inventory. Legacy consumer-surface strings whose semantics match
the new admin catalog are harvested as translation seeds; admin-console
vocabulary (Review/Quarantine/Disposition/...) has no legacy analog and falls
back to English until translated.

Inputs : ../melodee/src/Melodee.Blazor/Resources/SharedResources*.resx
         (overridable via argv[1])
Outputs: admin-ui/src/i18n/locales/<culture>.json  (19 non-English cultures,
         harvested keys only)
         target/verification/i18n/i18n-harvest.md  (coverage + mapping table)

Python stdlib only.
"""

from __future__ import annotations

import json
import re
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_RESX_DIR = REPO_ROOT.parent / "melodee" / "src" / "Melodee.Blazor" / "Resources"
LOCALES_DIR = REPO_ROOT / "admin-ui" / "src" / "i18n" / "locales"
REPORT_PATH = REPO_ROOT / "target" / "verification" / "i18n" / "i18n-harvest.md"

SOURCE_CULTURE = "en-US"

# Every non-English culture in the preserved inventory (ADR-0137), mapped to
# the legacy resx suffix. All 19 legacy resource files exist.
CULTURE_FILES = {
    "de-DE": "SharedResources.de-DE.resx",
    "es-ES": "SharedResources.es-ES.resx",
    "fr-FR": "SharedResources.fr-FR.resx",
    "it-IT": "SharedResources.it-IT.resx",
    "ja-JP": "SharedResources.ja-JP.resx",
    "pt-BR": "SharedResources.pt-BR.resx",
    "ru-RU": "SharedResources.ru-RU.resx",
    "zh-CN": "SharedResources.zh-CN.resx",
    "ar-SA": "SharedResources.ar-SA.resx",
    "nl-NL": "SharedResources.nl-NL.resx",
    "pl-PL": "SharedResources.pl-PL.resx",
    "tr-TR": "SharedResources.tr-TR.resx",
    "id-ID": "SharedResources.id-ID.resx",
    "ko-KR": "SharedResources.ko-KR.resx",
    "vi-VN": "SharedResources.vi-VN.resx",
    "fa-IR": "SharedResources.fa-IR.resx",
    "uk-UA": "SharedResources.uk-UA.resx",
    "cs-CZ": "SharedResources.cs-CZ.resx",
    "sv-SE": "SharedResources.sv-SE.resx",
}

# Curated mapping: legacy resx key -> new admin catalog key. Only keys whose
# semantics match exactly are mapped; legacy consumer-player vocabulary
# (Albums/Songs/Playlists/Jukebox/PartyMode) and mockup-era concepts with no
# admin-screen analog are deliberately excluded and fall back to English.
KEY_MAP = {
    # Navigation (semantics preserved in the admin IA)
    "Navigation.Artists": "nav.artists",
    "Navigation.Settings": "nav.settings",
    "Navigation.Search": "shell.search",
    "Navigation.Users": "users.title",
    "Navigation.SystemSettings": "settings.title",
    # Actions
    "Actions.Save": "actions.save",
    "Actions.SaveChanges": "actions.saveChanges",
    "Actions.Cancel": "actions.cancel",
    "Actions.Delete": "actions.delete",
    "Actions.Edit": "actions.edit",
    "Actions.Create": "actions.create",
    "Actions.Add": "actions.add",
    "Actions.Remove": "actions.remove",
    "Actions.Update": "actions.update",
    "Actions.Submit": "actions.submit",
    "Actions.Search": "actions.search",
    "Actions.Clear": "actions.clear",
    "Actions.Reset": "actions.reset",
    "Actions.Apply": "actions.apply",
    "Actions.Back": "actions.back",
    "Actions.Next": "actions.next",
    "Actions.Previous": "actions.previous",
    "Actions.Close": "actions.close",
    "Actions.Refresh": "actions.refresh",
    "Actions.Upload": "actions.upload",
    "Actions.Download": "actions.download",
    "Actions.Export": "actions.export",
    "Actions.Import": "actions.import",
    "Actions.ViewDetails": "actions.viewDetails",
    # Common field labels
    "Common.Name": "common.name",
    "Common.Description": "common.description",
    "Common.Artist": "common.artist",
    "Common.Status": "common.status",
    "Common.Type": "common.type",
    "Common.CreatedAt": "common.created",
    "Common.LastUpdatedAt": "common.updated",
    "Common.Never": "common.never",
    "Common.None": "common.none",
    "Common.Yes": "common.yes",
    "Common.No": "common.no",
    # Authentication
    "Auth.Login": "auth.signIn",
    "Auth.Logout": "auth.signOut",
    "Auth.Username": "auth.username",
    "Auth.Email": "auth.email",
    "Auth.Password": "auth.password",
    "Auth.ConfirmPassword": "auth.confirmPassword",
    "Auth.InvalidCredentials": "auth.invalidCredentials",
    "Auth.SignInToContinue": "auth.signInToContinue",
    "Auth.UsernameRequired": "auth.usernameRequired",
    "Auth.PasswordRequired": "auth.passwordRequired",
    # Validation
    "Validation.Required": "validation.required",
    "Validation.EmailInvalid": "validation.emailInvalid",
    "Validation.PasswordTooShort": "validation.passwordTooShort",
    "Validation.PasswordsDoNotMatch": "validation.passwordsDoNotMatch",
    # Status vocabulary shared with the admin status semantics
    "Status.Online": "status.online",
    "Status.Error": "status.error",
    "Status.Ok": "status.ok",
    "Status.Enabled": "status.enabled",
    "Status.Disabled": "status.disabled",
    "Status.Locked": "status.locked",
    "Status.Pending": "status.pending",
    "Status.InProgress": "status.inProgress",
    "Status.Completed": "status.completed",
    "Status.Rejected": "status.rejected",
    "Status.Running": "status.running",
    "Status.Paused": "status.paused",
    "Status.Ignored": "status.ignored",
    "Status.New": "status.new",
    # Grid / table chrome
    "Grid.NoRecordsToDisplay": "empty.noRecords",
    "Grid.ItemsPerPage": "common.itemsPerPage",
    "Grid.TotalRecords": "common.totalRecords",
    # Dialogs
    "Dialog.AreYouSure": "destructive.areYouSure",
    "Dialog.Confirm": "actions.confirm",
    "Dialog.DeleteConfirmation": "destructive.deleteConfirmation",
    # Messages
    "Message.Error": "common.error",
    "Message.Success": "common.success",
    "Message.Warning": "common.warning",
    "Message.NoDataAvailable": "empty.noData",
    # Roles
    "UserRole.Administrator": "users.roleAdministrator",
    "UserRole.Editor": "users.roleEditor",
    # Onboarding
    "Onboarding.WizardTitle": "onboarding.title",
    "Onboarding.CompleteSetup": "onboarding.completeSetup",
    "Onboarding.RetryButton": "actions.retry",
    # Settings
    "AdminDashboard.ExportConfiguration": "settings.exportConfiguration",
}

# Legacy resx placeholders are positional ({0}); the i18next catalog uses
# named interpolation ({{name}}). Rename per key where semantics are known.
PLACEHOLDER_RENAMES = {
    "Validation.PasswordTooShort": {"0": "min"},
    "Grid.TotalRecords": {"0": "total"},
}

_POSITIONAL = re.compile(r"\{(\d+)\}")


def load_resx(path: Path) -> dict[str, str]:
    """Parse a resx file into {key: value}, skipping empty values."""
    tree = ET.parse(path)
    values: dict[str, str] = {}
    for data in tree.getroot().iter("data"):
        name = data.get("name")
        value = data.find("value")
        if name and value is not None and value.text and value.text.strip():
            values[name] = value.text.strip()
    return values


def convert_placeholders(legacy_key: str, value: str) -> str:
    renames = PLACEHOLDER_RENAMES.get(legacy_key, {})

    def replace(match: re.Match[str]) -> str:
        index = match.group(1)
        # Unmapped positional placeholders keep a stable i18next-shaped name
        # so the string remains structurally valid.
        return "{{" + renames.get(index, f"p{index}") + "}}"

    return _POSITIONAL.sub(replace, value)


def main() -> int:
    resx_dir = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else DEFAULT_RESX_DIR
    if not resx_dir.is_dir():
        print(f"error: legacy resources directory not found: {resx_dir}", file=sys.stderr)
        return 1

    source_path = resx_dir / "SharedResources.resx"
    if not source_path.is_file():
        print(f"error: source resx not found: {source_path}", file=sys.stderr)
        return 1
    source_values = load_resx(source_path)

    en_catalog_path = LOCALES_DIR / f"{SOURCE_CULTURE}.json"
    en_catalog = json.loads(en_catalog_path.read_text(encoding="utf-8"))
    en_keys = set(en_catalog)

    # Guard against mapping onto keys that do not exist in the authored
    # en-US catalog; a drift here would silently drop harvested strings.
    missing_targets = sorted({v for v in KEY_MAP.values()} - en_keys)
    if missing_targets:
        print("error: KEY_MAP targets missing from en-US.json:", file=sys.stderr)
        for key in missing_targets:
            print(f"  - {key}", file=sys.stderr)
        return 1

    coverage: dict[str, int] = {}
    total_mapped = len(KEY_MAP)
    for culture, filename in CULTURE_FILES.items():
        path = resx_dir / filename
        if not path.is_file():
            print(f"warning: missing {filename}; skipping {culture}", file=sys.stderr)
            coverage[culture] = 0
            continue
        legacy = load_resx(path)
        harvested: dict[str, str] = {}
        for legacy_key, new_key in KEY_MAP.items():
            value = legacy.get(legacy_key)
            if value is None:
                continue
            # A culture file that simply repeats the English source is not a
            # translation; keep the honest fallback instead.
            if value == source_values.get(legacy_key) and culture != SOURCE_CULTURE:
                continue
            harvested[new_key] = convert_placeholders(legacy_key, value)
        out_path = LOCALES_DIR / f"{culture}.json"
        out_path.write_text(
            json.dumps(harvested, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        coverage[culture] = len(harvested)

    write_report(coverage, total_mapped, len(en_keys), resx_dir)
    print(f"harvested {total_mapped} mapped keys across {len(CULTURE_FILES)} cultures")
    for culture in CULTURE_FILES:
        print(f"  {culture}: {coverage.get(culture, 0)}/{total_mapped}")
    print(f"report: {REPORT_PATH.relative_to(REPO_ROOT)}")
    return 0


def write_report(coverage: dict[str, int], total_mapped: int, en_key_count: int, resx_dir: Path) -> None:
    lines = [
        "# Legacy i18n harvest report (Phase 10, W0-3)",
        "",
        "Generated by `scripts/harvest_legacy_i18n.py` from the legacy Blazor",
        "`SharedResources*.resx` files. Do not edit by hand; re-run the script.",
        "",
        "## Summary",
        "",
        f"- Source: `{resx_dir.relative_to(REPO_ROOT.parent) if resx_dir.is_relative_to(REPO_ROOT.parent) else resx_dir}` (override: `argv[1]`)",
        f"- Authored en-US catalog keys: {en_key_count}",
        f"- Legacy keys mapped onto the new catalog (exact semantic match): {total_mapped}",
        f"- Cultures written: {len(CULTURE_FILES)} (all non-English inventory entries)",
        "",
        "## Per-culture coverage",
        "",
        "Coverage counts keys actually harvested per culture out of the mapped set.",
        "Untranslated or English-echoed legacy values are excluded so the English",
        "fallback (ADR-0137) stays honest.",
        "",
        "| Culture | Harvested keys | Coverage of mapped set |",
        "|---|---|---|",
    ]
    for culture in CULTURE_FILES:
        count = coverage.get(culture, 0)
        pct = (100.0 * count / total_mapped) if total_mapped else 0.0
        lines.append(f"| {culture} | {count} | {pct:.1f}% |")
    lines += [
        "",
        "## Mapping table",
        "",
        "| Legacy resx key | Admin catalog key |",
        "|---|---|",
    ]
    for legacy_key in sorted(KEY_MAP):
        lines.append(f"| `{legacy_key}` | `{KEY_MAP[legacy_key]}` |")
    lines += [
        "",
        "## Honesty note",
        "",
        "Admin-console vocabulary — Review Queue, Quarantine, Disposition, Identity",
        "Review, policies, workers, diagnostics — has no legacy analog (the legacy",
        "admin surface was English-only in practice for these concepts). Those keys",
        "ship English-complete in en-US and fall back to English in every other",
        "locale until translated (ADR-0137 fallback semantics). Consumer-player",
        "legacy strings (Albums/Songs/Playlists/Jukebox/Party Mode) are not",
        "harvested: they do not exist in the admin IA and admin product language is",
        "Release/Track.",
    ]
    REPORT_PATH.parent.mkdir(parents=True, exist_ok=True)
    REPORT_PATH.write_text("\n".join(lines) + "\n", encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
