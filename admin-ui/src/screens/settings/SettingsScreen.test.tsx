import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "../../api/client";
import type {
  AdminScript,
  ConfigExport,
  ConfigImportResult,
  DeviceProfile,
  NotificationDelivery,
  NotificationTemplate,
  RadioStation,
  ScriptRun,
} from "../../api/types";
import { SettingsScreen } from "./SettingsScreen";
import { ThemeProvider } from "../../theme/ThemeProvider";

vi.mock("../../api/themes", () => ({
  listThemePacks: vi.fn(),
  rescanThemePacks: vi.fn(),
  deleteThemePack: vi.fn(),
  importThemePack: vi.fn(),
  themePackExportUrl: vi.fn((id: string) => `/api/v1/themes/${id}/export`),
  themePackAssetUrl: vi.fn(
    (id: string, path: string) => `/api/v1/themes/${id}/file/${path}`,
  ),
}));

vi.mock("../../api/admin", () => ({
  exportConfig: vi.fn(),
  importConfig: vi.fn(),
  listNotificationTemplates: vi.fn(),
  putNotificationTemplate: vi.fn(),
  deleteNotificationTemplate: vi.fn(),
  listNotificationDeliveries: vi.fn(),
  testSendNotification: vi.fn(),
  listScripts: vi.fn(),
  upsertScript: vi.fn(),
  deleteScript: vi.fn(),
  listScriptRuns: vi.fn(),
  enableScript: vi.fn(),
  disableScript: vi.fn(),
  testRunScript: vi.fn(),
  listRadioStations: vi.fn(),
  createRadioStation: vi.fn(),
  updateRadioStation: vi.fn(),
  deleteRadioStation: vi.fn(),
  listDeviceProfiles: vi.fn(),
  createDeviceProfile: vi.fn(),
  updateDeviceProfile: vi.fn(),
  deleteDeviceProfile: vi.fn(),
  setDefaultDeviceProfile: vi.fn(),
}));

const themes = await import("../../api/themes");
const mockListThemePacks = vi.mocked(themes.listThemePacks);

const admin = await import("../../api/admin");
const mockExportConfig = vi.mocked(admin.exportConfig);
const mockImportConfig = vi.mocked(admin.importConfig);
const mockListTemplates = vi.mocked(admin.listNotificationTemplates);
const mockPutTemplate = vi.mocked(admin.putNotificationTemplate);
const mockListDeliveries = vi.mocked(admin.listNotificationDeliveries);
const mockTestSend = vi.mocked(admin.testSendNotification);
const mockListScripts = vi.mocked(admin.listScripts);
const mockListScriptRuns = vi.mocked(admin.listScriptRuns);
const mockDisableScript = vi.mocked(admin.disableScript);
const mockTestRunScript = vi.mocked(admin.testRunScript);
const mockListStations = vi.mocked(admin.listRadioStations);
const mockCreateStation = vi.mocked(admin.createRadioStation);
const mockListProfiles = vi.mocked(admin.listDeviceProfiles);
const mockSetDefaultProfile = vi.mocked(admin.setDefaultDeviceProfile);

const EXPORT_DOC: ConfigExport = {
  format: "melodee-config-export/v1",
  warning: "Secrets are redacted.",
  exportedAt: "2026-08-14T12:00:00Z",
  settings: {
    "server.port": 8080,
    "search.enabled": true,
    "smtp.password": "__MELODEE_SECRET_REDACTED__",
  },
};

const PREVIEW: ConfigImportResult = {
  applied: false,
  added: ["server.workers"],
  changed: ["server.port"],
  unchanged: ["search.enabled"],
  secretsReentered: [],
  secretsSkipped: ["smtp.password"],
  unknown: [],
  warnings: [],
};

const TEMPLATE: NotificationTemplate = {
  templateKey: "user.password_reset",
  locale: "en",
  subject: "Reset your password",
  bodyText: "Body",
  bodyHtml: null,
  version: 3,
  updatedAt: "2026-08-14T10:00:00Z",
};

const DELIVERY: NotificationDelivery = {
  id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
  templateKey: "user.password_reset",
  recipient: "op@example.com",
  status: "sent",
  error: null,
  createdAt: "2026-08-14T11:00:00Z",
};

const SCRIPT: AdminScript = {
  id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
  event: "directory_processing_start",
  name: "gate",
  source: 'export function main(ctx) { return { outcome: "allow" }; }',
  enabled: true,
  wired: true,
  timeoutMs: 100,
  memoryBytes: 16777216,
  version: 2,
  createdAt: "2026-08-14T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const RUN: ScriptRun = {
  id: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",
  scriptId: SCRIPT.id,
  event: "directory_processing_start",
  outcome: "allow",
  durationMs: 3,
  error: null,
  createdAt: "2026-08-14T11:00:00Z",
};

const STATION: RadioStation = {
  id: "ffffffff-ffff-4fff-8fff-ffffffffffff",
  name: "SomaFM Groove Salad",
  streamUrl: "https://ice1.somafm.com/groovesalad-128-mp3",
  homePageUrl: "https://somafm.com/groovesalad/",
  createdAt: "2026-08-14T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const PROFILE: DeviceProfile = {
  id: "99999999-9999-4999-8999-999999999999",
  name: "Feishin desktop",
  clientMatch: { clientName: "feishin" },
  profile: { directPlay: true },
  isDefault: false,
  createdAt: "2026-08-14T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

function renderScreen(initialEntry = "/system/settings") {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <ThemeProvider>
        <SettingsScreen />
      </ThemeProvider>
    </MemoryRouter>,
  );
}

function stubAllSections() {
  mockListTemplates.mockResolvedValue([TEMPLATE]);
  mockListDeliveries.mockResolvedValue([DELIVERY]);
  mockListScripts.mockResolvedValue({
    scripts: [SCRIPT],
    wiredEvents: ["directory_processing_start"],
  });
  mockListScriptRuns.mockResolvedValue([RUN]);
  mockListStations.mockResolvedValue([STATION]);
  mockListProfiles.mockResolvedValue([PROFILE]);
  // The Theme tab mounts alongside the others (Carbon keeps hidden panels
  // mounted); a valid empty inventory keeps it quiet.
  mockListThemePacks.mockResolvedValue([]);
}

describe("SettingsScreen configuration tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockExportConfig.mockResolvedValue(EXPORT_DOC);
    stubAllSections();
  });

  it("renders the export document grouped with secret values redacted", async () => {
    renderScreen();
    // Redacted marker renders as a tag, never as an editable value.
    expect(await screen.findByText("Redacted")).toBeInTheDocument();
    expect(screen.getByText("smtp.password")).toBeInTheDocument();
    expect(screen.getByText("8080")).toBeInTheDocument();
    // The honesty note: no per-key mutation endpoint exists.
    expect(screen.getByText("Settings are read-only here")).toBeInTheDocument();
    expect(screen.getByText("Secrets are redacted.")).toBeInTheDocument();
  });

  it("filters the browser by key substring", async () => {
    renderScreen();
    expect(await screen.findByText("server.port")).toBeInTheDocument();
    fireEvent.change(
      screen.getByRole("searchbox", { name: "Filter settings by key" }),
      {
        target: { value: "smtp" },
      },
    );
    expect(screen.queryByText("server.port")).not.toBeInTheDocument();
    expect(screen.getByText("smtp.password")).toBeInTheDocument();
  });

  it("runs the import preview then applies with the exact requests", async () => {
    mockImportConfig.mockResolvedValue(PREVIEW);
    renderScreen();
    await screen.findByText("server.port");

    const importDoc = {
      format: "melodee-config-export/v1",
      settings: { "server.port": 9090 },
    };
    const file = new File([JSON.stringify(importDoc)], "config.json", {
      type: "application/json",
    });
    const input = window.document.querySelector(
      'input[type="file"]',
    ) as HTMLInputElement;
    fireEvent.change(input, { target: { files: [file] } });

    // Preview request: preview=true, allowUnknown=false.
    await waitFor(() =>
      expect(mockImportConfig).toHaveBeenCalledWith(importDoc, {
        preview: true,
        allowUnknown: false,
      }),
    );
    expect(
      await screen.findByText("Import preview: config.json"),
    ).toBeInTheDocument();
    expect(screen.getByText("server.workers")).toBeInTheDocument();

    // Apply is explicit and re-posts with preview=false.
    fireEvent.click(screen.getByRole("button", { name: "Apply import" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Apply configuration import",
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Apply import" }),
    );
    await waitFor(() =>
      expect(mockImportConfig).toHaveBeenLastCalledWith(importDoc, {
        preview: false,
        allowUnknown: false,
      }),
    );
    expect(
      await screen.findByText("Configuration import applied."),
    ).toBeInTheDocument();
  });

  it("surfaces the redacted-marker rejection with localized text", async () => {
    mockImportConfig.mockRejectedValue(
      new ApiError(
        400,
        "redacted_marker_rejected",
        "redacted_marker_rejected",
        null,
      ),
    );
    renderScreen();
    await screen.findByText("server.port");

    const file = new File([JSON.stringify(EXPORT_DOC)], "config.json", {
      type: "application/json",
    });
    fireEvent.change(
      window.document.querySelector('input[type="file"]') as HTMLInputElement,
      {
        target: { files: [file] },
      },
    );
    expect(
      await screen.findByText(
        /The redaction marker is never accepted as a value/,
      ),
    ).toBeInTheDocument();
  });

  it("rejects a non-JSON file before any request is made", async () => {
    renderScreen();
    await screen.findByText("server.port");
    const file = new File(["not json"], "config.json", {
      type: "application/json",
    });
    fireEvent.change(
      window.document.querySelector('input[type="file"]') as HTMLInputElement,
      {
        target: { files: [file] },
      },
    );
    expect(
      await screen.findByText("The selected file is not valid JSON."),
    ).toBeInTheDocument();
    expect(mockImportConfig).not.toHaveBeenCalled();
  });
});

describe("SettingsScreen notifications tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockExportConfig.mockResolvedValue(EXPORT_DOC);
    stubAllSections();
  });

  it("lists templates and edits a template with null-cleared fields", async () => {
    mockPutTemplate.mockResolvedValue({
      ...TEMPLATE,
      version: 4,
      bodyHtml: null,
    });
    renderScreen("/system/settings?tab=notifications");
    const table = await screen.findByRole("table", {
      name: "Notification templates",
    });
    expect(within(table).getByText("user.password_reset")).toBeInTheDocument();

    fireEvent.click(within(table).getByRole("button", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Edit template user.password_reset (en)",
    });
    fireEvent.change(within(dialog).getByLabelText("Plain-text body"), {
      target: { value: "" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Save changes" }),
    );
    await waitFor(() =>
      expect(mockPutTemplate).toHaveBeenCalledWith(
        "user.password_reset",
        "en",
        {
          subject: "Reset your password",
          bodyText: null,
          bodyHtml: null,
        },
      ),
    );
  });

  it("test-sends and renders the recorded delivery result", async () => {
    mockTestSend.mockResolvedValue({ ...DELIVERY, status: "skipped" });
    renderScreen("/system/settings?tab=notifications");
    const table = await screen.findByRole("table", {
      name: "Notification templates",
    });
    fireEvent.click(within(table).getByRole("button", { name: "Test send" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Test send user.password_reset (en)",
    });
    fireEvent.change(within(dialog).getByLabelText("Recipient"), {
      target: { value: "op@example.com" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Test send" }));
    await waitFor(() =>
      expect(mockTestSend).toHaveBeenCalledWith({
        templateKey: "user.password_reset",
        locale: "en",
        recipient: "op@example.com",
      }),
    );
    expect(await within(dialog).findByText(/skipped/)).toBeInTheDocument();
  });
});

describe("SettingsScreen scripts tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockExportConfig.mockResolvedValue(EXPORT_DOC);
    stubAllSections();
  });

  it("lists scripts with the wired-event registry and disables via the route", async () => {
    mockDisableScript.mockResolvedValue(undefined);
    renderScreen("/system/settings?tab=scripts");
    const table = await screen.findByRole("table", {
      name: "Registered scripts",
    });
    expect(within(table).getByText("gate")).toBeInTheDocument();
    expect(
      screen.getByText(/directory_processing_start/, {
        selector: ".cds--inline-notification__subtitle",
      }),
    ).toBeInTheDocument();

    fireEvent.click(within(table).getByRole("button", { name: "Disable" }));
    await waitFor(() =>
      expect(mockDisableScript).toHaveBeenCalledWith(SCRIPT.id),
    );
    expect(await screen.findByText("Script disabled.")).toBeInTheDocument();
  });

  it("runs a test with the stored script id and shows the evaluation", async () => {
    mockTestRunScript.mockResolvedValue({
      evaluation: {
        outcome: "allow",
        message: "ok",
        error: null,
        durationMs: 2,
      },
      run: RUN,
      effectiveOnDeny: "skip",
    });
    renderScreen("/system/settings?tab=scripts");
    const table = await screen.findByRole("table", {
      name: "Registered scripts",
    });
    fireEvent.click(within(table).getByRole("button", { name: "Test run" }));
    const dialog = await screen.findByRole("dialog", { name: "Test run gate" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Test run" }));
    await waitFor(() =>
      expect(mockTestRunScript).toHaveBeenCalledWith({
        event: "directory_processing_start",
        scriptId: SCRIPT.id,
        ctx: {},
      }),
    );
    expect(
      await within(dialog).findByText(/Effective on-deny action/),
    ).toBeInTheDocument();
  });
});

describe("SettingsScreen radio tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockExportConfig.mockResolvedValue(EXPORT_DOC);
    stubAllSections();
  });

  it("creates a station with the exact request body", async () => {
    mockCreateStation.mockResolvedValue(STATION);
    renderScreen("/system/settings?tab=radio");
    const table = await screen.findByRole("table", {
      name: "Internet radio stations",
    });
    expect(within(table).getByText("SomaFM Groove Salad")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Add station" }));
    const dialog = await screen.findByRole("dialog", {
      name: "Add radio station",
    });
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "New Station" },
    });
    fireEvent.change(within(dialog).getByLabelText("Stream URL"), {
      target: { value: "https://example.com/stream" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() =>
      expect(mockCreateStation).toHaveBeenCalledWith({
        name: "New Station",
        streamUrl: "https://example.com/stream",
      }),
    );
  });
});

describe("SettingsScreen device profiles tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockExportConfig.mockResolvedValue(EXPORT_DOC);
    stubAllSections();
  });

  it("states the metadata-only contract and sets the default profile", async () => {
    mockSetDefaultProfile.mockResolvedValue({ ...PROFILE, isDefault: true });
    renderScreen("/system/settings?tab=deviceProfiles");
    // Metadata-only honesty text: profiles do not affect streaming.
    expect(await screen.findByText("Metadata only")).toBeInTheDocument();
    expect(
      screen.getAllByText(/do not affect streaming today/).length,
    ).toBeGreaterThan(0);

    const table = screen.getByRole("table", { name: "Device profiles" });
    fireEvent.click(within(table).getByRole("button", { name: "Set default" }));
    await waitFor(() =>
      expect(mockSetDefaultProfile).toHaveBeenCalledWith(PROFILE.id),
    );
    expect(
      await screen.findByText("Default device profile updated."),
    ).toBeInTheDocument();
  });

  it("surfaces the 400 invalid_profile error", async () => {
    mockSetDefaultProfile.mockRejectedValue(
      new ApiError(400, "invalid_profile", "name must be non-empty", null),
    );
    renderScreen("/system/settings?tab=deviceProfiles");
    const table = await screen.findByRole("table", { name: "Device profiles" });
    fireEvent.click(within(table).getByRole("button", { name: "Set default" }));
    expect(
      await screen.findByText(/The profile is invalid/),
    ).toBeInTheDocument();
  });
});
