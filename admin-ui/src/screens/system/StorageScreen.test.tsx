import { render, screen, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "../../api/client";
import type { OnboardingStatus, SystemHealth } from "../../api/types";
import { StorageScreen } from "./StorageScreen";

vi.mock("../../api/admin", () => ({
  getOnboardingStatus: vi.fn(),
  getSystemHealth: vi.fn(),
}));

const admin = await import("../../api/admin");
const mockGetOnboardingStatus = vi.mocked(admin.getOnboardingStatus);
const mockGetSystemHealth = vi.mocked(admin.getSystemHealth);

const HEALTH: SystemHealth = {
  status: "ok",
  postgresql: { status: "ok", detail: "postgresql answered a bounded probe" },
  tantivy: { status: "ok", detail: "active search generation validated" },
  storage: { status: "ok", detail: "storage paths writable" },
  ffmpeg: { status: "ok", detail: "ffmpeg 8.1" },
  apiSurfaces: [{ surface: "native", status: "serving" }],
};

function makeOnboarding(checks: OnboardingStatus["checks"]): OnboardingStatus {
  return {
    completed: true,
    bypassed: false,
    canComplete: true,
    completedBy: "admin",
    checks,
  };
}

const PATHS_PASSED = {
  name: "storage_paths",
  required: false,
  passed: true,
  detail:
    "inbound: 2 source(s) [/srv/incoming, /srv/drop]; staging: /srv/staging; work: /var/cache/melodee/work; canonical: /srv/melodee",
  remediation: null,
};

const PATHS_FAILED = {
  name: "storage_paths",
  required: false,
  passed: false,
  detail: "inbound source /library/incoming does not exist or is not readable",
  remediation:
    "create the directories with read (sources) / write (storage) permissions for the server user",
};

function renderScreen() {
  return render(
    <MemoryRouter initialEntries={["/system/storage"]}>
      <StorageScreen />
    </MemoryRouter>,
  );
}

describe("StorageScreen", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetSystemHealth.mockResolvedValue(HEALTH);
    mockGetOnboardingStatus.mockResolvedValue(makeOnboarding([PATHS_PASSED]));
  });

  it("renders the storage subsystem verdict from /system/health", async () => {
    renderScreen();
    const panel = (
      await screen.findByRole("heading", { name: "Storage Subsystem" })
    ).closest("section");
    expect(panel).not.toBeNull();
    expect(
      within(panel as HTMLElement).getByText("Healthy"),
    ).toBeInTheDocument();
    expect(
      within(panel as HTMLElement).getByText("storage paths writable"),
    ).toBeInTheDocument();
  });

  it("renders the storage-paths preflight detail from /onboarding/status", async () => {
    renderScreen();
    const panel = (
      await screen.findByRole("heading", { name: "Storage Paths Preflight" })
    ).closest("section");
    expect(panel).not.toBeNull();
    expect(
      within(panel as HTMLElement).getByText("Passed"),
    ).toBeInTheDocument();
    // ADR-0145: each storage role is reported separately in the detail.
    const detail = within(panel as HTMLElement).getByText(
      /inbound: 2 source\(s\)/,
    );
    expect(detail).toBeInTheDocument();
    for (const role of [
      /inbound: 2 source\(s\) \[\/srv\/incoming, \/srv\/drop\]/,
      /staging: \/srv\/staging/,
      /work: \/var\/cache\/melodee\/work/,
      /canonical: \/srv\/melodee/,
    ]) {
      expect(within(panel as HTMLElement).getByText(role)).toBeInTheDocument();
    }
  });

  it("renders the remediation guidance when the preflight fails", async () => {
    mockGetOnboardingStatus.mockResolvedValue(makeOnboarding([PATHS_FAILED]));
    renderScreen();
    expect(await screen.findByText("Failed")).toBeInTheDocument();
    expect(
      screen.getByText(/inbound source \/library\/incoming does not exist/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/create the directories with read/),
    ).toBeInTheDocument();
  });

  it("states honestly when the preflight check is absent", async () => {
    mockGetOnboardingStatus.mockResolvedValue(makeOnboarding([]));
    renderScreen();
    expect(
      await screen.findByText(
        "The storage-paths preflight result is not available.",
      ),
    ).toBeInTheDocument();
  });

  it("states that per-path usage remains a CLI surface", async () => {
    renderScreen();
    expect(
      await screen.findByText("Per-path usage stays a CLI surface"),
    ).toBeInTheDocument();
    expect(screen.getByText(/melodee-cli doctor/)).toBeInTheDocument();
  });

  it("renders retryable error states when either fetch fails", async () => {
    mockGetSystemHealth.mockRejectedValue(
      new ApiError(500, "server_error", "health boom", null),
    );
    mockGetOnboardingStatus.mockRejectedValue(
      new ApiError(500, "server_error", "onboarding boom", null),
    );
    renderScreen();
    const errors = await screen.findAllByText(
      "System information could not be loaded",
    );
    expect(errors.length).toBe(2);
    expect(screen.getByText("health boom")).toBeInTheDocument();
    expect(screen.getByText("onboarding boom")).toBeInTheDocument();
  });
});
