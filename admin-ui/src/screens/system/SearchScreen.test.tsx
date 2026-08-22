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
import type { SearchStatus } from "../../api/types";
import { SearchScreen } from "./SearchScreen";

vi.mock("../../api/admin", () => ({
  getSearchStatus: vi.fn(),
  requestSearchRebuild: vi.fn(),
}));

const admin = await import("../../api/admin");
const mockGetSearchStatus = vi.mocked(admin.getSearchStatus);
const mockRequestSearchRebuild = vi.mocked(admin.requestSearchRebuild);

const SEARCH_ENABLED: SearchStatus = {
  enabled: true,
  projection: {
    pending: 3,
    failed: 0,
    leased: 1,
    oldestAvailableAt: null,
    lastProcessedOutboxId: 42,
    activeGenerationId: "55555555-5555-4555-8555-555555555555",
    latestRebuildStatus: null,
    latestRebuildError: null,
  },
  generation: {
    artists: 100,
    releases: 200,
    tracks: 300,
    total: 600,
    schemaVersion: 1,
  },
  generationErrorCode: null,
  generationError: null,
};

function renderScreen() {
  return render(
    <MemoryRouter initialEntries={["/system/search"]}>
      <SearchScreen />
    </MemoryRouter>,
  );
}

/** Carbon keeps closed modals mounted; pick the dialog containing a heading. */
async function findDialogWithText(text: string): Promise<HTMLElement> {
  const dialogs = await screen.findAllByRole("dialog");
  const match = dialogs.find(
    (dialog) => within(dialog).queryByText(text) !== null,
  );
  if (!match) {
    throw new Error(`no dialog contains "${text}"`);
  }
  return match;
}

/** The panel trigger, not the (mounted) modal's primary button. */
function panelButton(name: string): HTMLElement {
  const matches = screen
    .getAllByRole("button", { name })
    .filter((element) => element.closest('[role="dialog"]') === null);
  if (matches.length !== 1) {
    throw new Error(
      `expected exactly one panel button "${name}", found ${matches.length}`,
    );
  }
  const [button] = matches;
  return button as HTMLElement;
}

describe("SearchScreen", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetSearchStatus.mockResolvedValue(SEARCH_ENABLED);
  });

  it("renders the real search status and the shared rebuild panel", async () => {
    renderScreen();
    expect(
      await screen.findByRole("heading", { name: "Search", level: 1 }),
    ).toBeInTheDocument();
    expect(await screen.findByText("Indexed documents")).toBeInTheDocument();
    expect(screen.getByText("600")).toBeInTheDocument();
    expect(screen.getByText("Pending documents")).toBeInTheDocument();
  });

  it("states honestly that MQL query tooling is a CLI/native-API surface", async () => {
    renderScreen();
    expect(
      await screen.findByText("Query tooling is a CLI and API surface"),
    ).toBeInTheDocument();
    expect(screen.getByText(/melodee search/)).toBeInTheDocument();
    // No query affordance exists: the admin API has no search-query endpoint.
    expect(screen.queryByRole("searchbox")).not.toBeInTheDocument();
  });

  it("requests a rebuild through the confirmation modal", async () => {
    mockRequestSearchRebuild.mockResolvedValue({
      jobId: "88888888-8888-4888-8888-888888888888",
      status: "pending",
      requestedBy: "admin-1",
    });
    renderScreen();
    await screen.findByText("Indexed documents");
    fireEvent.click(panelButton("Rebuild search index"));
    const dialog = await findDialogWithText("Rebuild the search index");
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Rebuild search index" }),
    );
    await waitFor(() => expect(mockRequestSearchRebuild).toHaveBeenCalled());
    expect(
      await screen.findByText(
        /Rebuild requested \(job 88888888-8888-4888-8888-888888888888\)/,
      ),
    ).toBeInTheDocument();
  });

  it("renders the disabled state honestly when the search worker is off", async () => {
    mockGetSearchStatus.mockResolvedValue({
      enabled: false,
      projection: null,
      generation: null,
      generationErrorCode: null,
      generationError: null,
    });
    renderScreen();
    expect(
      await screen.findByText("Search worker disabled"),
    ).toBeInTheDocument();
    expect(panelButton("Rebuild search index")).toBeDisabled();
  });

  it("renders a retryable error state when the status fetch fails", async () => {
    mockGetSearchStatus.mockRejectedValue(
      new ApiError(500, "server_error", "boom", null),
    );
    renderScreen();
    expect(
      await screen.findByText("System information could not be loaded"),
    ).toBeInTheDocument();
    expect(screen.getByText("boom")).toBeInTheDocument();
  });
});
