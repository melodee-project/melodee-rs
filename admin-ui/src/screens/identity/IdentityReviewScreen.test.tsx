import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "../../api/client";
import type {
  IdentityDecisionEntry,
  ObservationDetail,
  Page,
  UnresolvedObservation,
} from "../../api/types";
import { IdentityReviewScreen } from "./IdentityReviewScreen";
import { takeFlashNotice } from "./identity";

vi.mock("../../api/admin", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../api/admin")>();
  return {
    ...original,
    listUnresolvedObservations: vi.fn(),
    getObservationDetail: vi.fn(),
    resolveObservation: vi.fn(),
    neverMatchCandidate: vi.fn(),
    listIdentityDecisions: vi.fn(),
    mergeArtists: vi.fn(),
    splitArtist: vi.fn(),
  };
});

const {
  listUnresolvedObservations,
  getObservationDetail,
  resolveObservation,
  neverMatchCandidate,
  listIdentityDecisions,
  mergeArtists,
  splitArtist,
} = await import("../../api/admin");
const mockListUnresolved = vi.mocked(listUnresolvedObservations);
const mockGetDetail = vi.mocked(getObservationDetail);
const mockResolve = vi.mocked(resolveObservation);
const mockNeverMatch = vi.mocked(neverMatchCandidate);
const mockListDecisions = vi.mocked(listIdentityDecisions);
const mockMerge = vi.mocked(mergeArtists);
const mockSplit = vi.mocked(splitArtist);

const OBSERVATION_ID = "11111111-1111-4111-8111-111111111111";
const OBSERVATION_ID_2 = "22222222-2222-4222-8222-222222222222";
const ARTIST_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const ARTIST_ID_2 = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

function makeObservation(
  overrides: Partial<UnresolvedObservation> = {},
): UnresolvedObservation {
  return {
    id: OBSERVATION_ID,
    displayValue: "The Nomads",
    rawValue: "The Nomads",
    normalizedValue: "the nomads",
    scope: "release",
    sourceOrdinal: 0,
    sourcePointer: "/library/incoming/Into the Night",
    releaseId: "55555555-5555-4555-8555-555555555555",
    releaseTitle: "Into the Night",
    trackId: null,
    trackTitle: null,
    pendingMatchCount: 2,
    evidenceCount: 5,
    createdAt: "2026-08-14T10:00:00Z",
    ...overrides,
  };
}

function makeQueue(): Page<UnresolvedObservation> {
  return {
    items: [
      makeObservation(),
      makeObservation({
        id: OBSERVATION_ID_2,
        displayValue: "Silver Skies",
        normalizedValue: "silver skies",
        releaseTitle: "Fragments",
        pendingMatchCount: 1,
      }),
    ],
    total: 2,
    limit: 25,
    offset: 0,
  };
}

function makeDetail(
  overrides: Partial<ObservationDetail> = {},
): ObservationDetail {
  return {
    observation: makeObservation(),
    current: null,
    candidates: [
      {
        artistId: ARTIST_ID,
        artistName: "The Nomads",
        artistStatus: "active",
        latestOutcome: "provisional",
        latestConfidence: 9200,
        latestReason: "Strong alias and release overlap.",
        latestLocked: false,
        latestDecidedAt: "2026-08-14T10:05:00Z",
        decisionCount: 3,
        evidence: [
          {
            kind: "alias",
            polarity: "positive",
            confidence: 9500,
            source: "tags",
            payload: { alias: "Nomads" },
            createdAt: "2026-08-14T10:04:00Z",
          },
          {
            kind: "year",
            polarity: "positive",
            confidence: 8800,
            source: "tags",
            payload: { year: 1986 },
            createdAt: "2026-08-14T10:04:00Z",
          },
          {
            kind: "release",
            polarity: "positive",
            confidence: 9100,
            source: "catalog",
            payload: { release: "Into the Night" },
            createdAt: "2026-08-14T10:04:00Z",
          },
          {
            kind: "external",
            polarity: "positive",
            confidence: 9000,
            source: "musicbrainz",
            payload: { mbid: "external-1" },
            createdAt: "2026-08-14T10:04:00Z",
          },
          {
            kind: "release",
            polarity: "negative",
            confidence: 3000,
            source: "catalog",
            payload: { release: "Conflicting release" },
            createdAt: "2026-08-14T10:04:00Z",
          },
        ],
      },
      {
        artistId: ARTIST_ID_2,
        artistName: "The Nomads (US)",
        artistStatus: "provisional",
        latestOutcome: "provisional",
        latestConfidence: 4100,
        latestReason: "",
        latestLocked: false,
        latestDecidedAt: "2026-08-14T10:05:00Z",
        decisionCount: 1,
        evidence: [],
      },
    ],
    negativeAssertions: [],
    ...overrides,
  };
}

function makeFeed(): Page<IdentityDecisionEntry> {
  return {
    items: [
      {
        eventId: "99999999-9999-4999-8999-999999999999",
        entityType: "observedArtist",
        entityId: OBSERVATION_ID,
        eventType: "match_confirmed",
        actor: "operator-1",
        reason: "Verified against the catalog.",
        payload: {},
        createdAt: "2026-08-14T10:06:00Z",
      },
    ],
    total: 1,
    limit: 20,
    offset: 0,
  };
}

function renderScreen(entry = "/catalog/identity") {
  return render(
    <MemoryRouter initialEntries={[entry]}>
      <Routes>
        <Route path="/catalog/identity" element={<IdentityReviewScreen />} />
      </Routes>
    </MemoryRouter>,
  );
}

describe("IdentityReviewScreen", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Reset the module-scope flash slot so notices never leak between tests.
    takeFlashNotice();
    mockListUnresolved.mockResolvedValue(makeQueue());
    mockGetDetail.mockResolvedValue(makeDetail());
    mockListDecisions.mockResolvedValue(makeFeed());
    mockResolve.mockResolvedValue({
      observationId: OBSERVATION_ID,
      resolution: "confirm",
      artistId: ARTIST_ID,
      changed: true,
    });
    mockNeverMatch.mockResolvedValue({
      observationId: OBSERVATION_ID,
      artistId: ARTIST_ID,
      recorded: true,
    });
    mockMerge.mockResolvedValue({
      mergeId: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
      sourceArtistId: ARTIST_ID,
      targetArtistId: ARTIST_ID_2,
    });
    mockSplit.mockResolvedValue({
      splitId: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
      sourceArtistId: ARTIST_ID,
      newArtistId: ARTIST_ID_2,
      movedAssignments: 1,
    });
  });

  it("renders the queue with pending counts and loads detail on selection", async () => {
    renderScreen();
    const table = await screen.findByRole("table", {
      name: "Unresolved artist observations",
    });
    expect(within(table).getByText("The Nomads")).toBeInTheDocument();
    expect(within(table).getByText("Into the Night")).toBeInTheDocument();
    expect(within(table).getByText("2 pending")).toBeInTheDocument();
    expect(within(table).getByText("Silver Skies")).toBeInTheDocument();

    fireEvent.click(within(table).getByText("The Nomads"));
    await waitFor(() =>
      expect(mockGetDetail).toHaveBeenCalledWith(OBSERVATION_ID),
    );

    // Incoming observation card: observed name plus source context.
    expect(await screen.findByText("Incoming observation")).toBeInTheDocument();
    expect(screen.getByText("Identity review")).toBeInTheDocument();
    expect(
      screen.getByText("/library/incoming/Into the Night"),
    ).toBeInTheDocument();
  });

  it("reflects the selection in the URL when deep-linked", async () => {
    renderScreen(`/catalog/identity?observation=${OBSERVATION_ID}`);
    await waitFor(() =>
      expect(mockGetDetail).toHaveBeenCalledWith(OBSERVATION_ID),
    );
    expect(await screen.findByText("Incoming observation")).toBeInTheDocument();
  });

  it("renders candidate confidence and the persisted evidence breakdown", async () => {
    renderScreen(`/catalog/identity?observation=${OBSERVATION_ID}`);
    expect(await screen.findByText("Candidate Matches")).toBeInTheDocument();
    // Confidence percentages from basis points.
    expect(screen.getByText("92%")).toBeInTheDocument();
    expect(screen.getByText("41%")).toBeInTheDocument();
    expect(
      screen.getByText("Strong alias and release overlap."),
    ).toBeInTheDocument();
    // Evidence summary strip: counts by kind with the negative count split out.
    expect(screen.getByText("Evidence Summary")).toBeInTheDocument();
    expect(screen.getByText("Alias match")).toBeInTheDocument();
    expect(screen.getByText("Year match")).toBeInTheDocument();
    expect(screen.getByText("Release match")).toBeInTheDocument();
    // "External identity" also appears as a split-assignment kind option.
    expect(screen.getAllByText("External identity").length).toBeGreaterThan(0);
    expect(screen.getByText("2 (1 negative)")).toBeInTheDocument();
  });

  it("confirm posts the resolution with the selected candidate and reason", async () => {
    renderScreen(`/catalog/identity?observation=${OBSERVATION_ID}`);
    fireEvent.click(
      await screen.findByRole("radio", { name: /^The NomadsActive/ }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Confirm Match" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Reason"), {
      target: { value: "Alias and release evidence line up." },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Apply decision" }),
    );
    await waitFor(() =>
      expect(mockResolve).toHaveBeenCalledWith(OBSERVATION_ID, {
        resolution: "confirm",
        artistId: ARTIST_ID,
        reason: "Alias and release evidence line up.",
      }),
    );
    expect(await screen.findByText("Match confirmed.")).toBeInTheDocument();
    // Queue refetches and selection advances to the next unresolved row.
    await waitFor(() => expect(mockListUnresolved).toHaveBeenCalledTimes(2));
  });

  it("create-new posts without an artistId", async () => {
    mockResolve.mockResolvedValue({
      observationId: OBSERVATION_ID,
      resolution: "create-new",
      artistId: ARTIST_ID_2,
      changed: true,
    });
    renderScreen(`/catalog/identity?observation=${OBSERVATION_ID}`);
    fireEvent.click(
      await screen.findByRole("button", { name: "Create New Artist" }),
    );
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Reason"), {
      target: { value: "No existing Artist is correct." },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Apply decision" }),
    );
    await waitFor(() =>
      expect(mockResolve).toHaveBeenCalledWith(OBSERVATION_ID, {
        resolution: "create-new",
        reason: "No existing Artist is correct.",
      }),
    );
    expect(
      await screen.findByText("New provisional Artist created."),
    ).toBeInTheDocument();
  });

  it("confirm requires a selected candidate", async () => {
    renderScreen(`/catalog/identity?observation=${OBSERVATION_ID}`);
    fireEvent.click(
      await screen.findByRole("button", { name: "Confirm Match" }),
    );
    expect(
      await screen.findByText(
        "Select a candidate Artist before confirming or rejecting a match.",
      ),
    ).toBeInTheDocument();
    expect(mockResolve).not.toHaveBeenCalled();
  });

  it("never-match is destructive-styled and requires a reason", async () => {
    renderScreen(`/catalog/identity?observation=${OBSERVATION_ID}`);
    fireEvent.click(
      await screen.findByRole("radio", {
        name: /^The Nomads \(US\)Provisional/,
      }),
    );
    const neverMatch = screen.getByRole("button", {
      name: "Never Match Candidate",
    });
    fireEvent.click(neverMatch);
    const dialog = await screen.findByRole("dialog");
    const submit = within(dialog).getByRole("button", {
      name: "Never Match Candidate",
    });
    expect(submit).toBeDisabled();
    expect(mockNeverMatch).not.toHaveBeenCalled();
    fireEvent.change(within(dialog).getByLabelText("Reason"), {
      target: { value: "Different band, same name." },
    });
    expect(submit).toBeEnabled();
    fireEvent.click(submit);
    await waitFor(() =>
      expect(mockNeverMatch).toHaveBeenCalledWith(OBSERVATION_ID, {
        artistId: ARTIST_ID_2,
        reason: "Different band, same name.",
      }),
    );
    expect(
      await screen.findByText("Do-not-match assertion recorded."),
    ).toBeInTheDocument();
  });

  it("renders the recent identity decisions feed", async () => {
    renderScreen();
    expect(
      await screen.findByText("Recent Identity Decisions"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("match_confirmed (observedArtist)"),
    ).toBeInTheDocument();
    expect(screen.getByText("operator-1")).toBeInTheDocument();
    expect(
      screen.getByText("Verified against the catalog."),
    ).toBeInTheDocument();
  });

  it("surfaces a 409 conflict as an inline notification", async () => {
    mockResolve.mockRejectedValue(
      new ApiError(
        409,
        "locked_decision",
        "a locked manual identity decision prevents this change",
        "corr-1",
      ),
    );
    renderScreen(`/catalog/identity?observation=${OBSERVATION_ID}`);
    fireEvent.click(
      await screen.findByRole("radio", { name: /^The NomadsActive/ }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Confirm Match" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Reason"), {
      target: { value: "Trying." },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Apply decision" }),
    );
    expect(
      await screen.findByText(
        /could not be applied: a locked manual identity decision/,
      ),
    ).toBeInTheDocument();
  });

  it("merge requires the typed confirmation and posts both public ids with a reason", async () => {
    renderScreen();
    // Open the Artist operations accordion.
    fireEvent.click(
      await screen.findByRole("button", { name: "Artist operations" }),
    );
    const mergeRegion = screen.getByRole("region", { name: "Merge Artists" });
    fireEvent.change(
      within(mergeRegion).getByLabelText("Source Artist public id"),
      {
        target: { value: ARTIST_ID },
      },
    );
    fireEvent.change(
      within(mergeRegion).getByLabelText("Target Artist public id"),
      {
        target: { value: ARTIST_ID_2 },
      },
    );
    fireEvent.change(within(mergeRegion).getByLabelText("Reason"), {
      target: { value: "Duplicate identities." },
    });
    const mergeButton = within(mergeRegion).getByRole("button", {
      name: "Merge Artists",
    });
    expect(mergeButton).toBeDisabled();
    fireEvent.change(
      within(mergeRegion).getByLabelText(/Type MERGE to confirm/),
      {
        target: { value: "MERGE" },
      },
    );
    expect(mergeButton).toBeEnabled();
    fireEvent.click(mergeButton);
    await waitFor(() =>
      expect(mockMerge).toHaveBeenCalledWith({
        sourceArtistId: ARTIST_ID,
        targetArtistId: ARTIST_ID_2,
        reason: "Duplicate identities.",
      }),
    );
    expect(
      await screen.findByText("The Artists were merged."),
    ).toBeInTheDocument();
  });

  it("split posts the new Artist and explicit assignments with a reason", async () => {
    renderScreen();
    fireEvent.click(
      await screen.findByRole("button", { name: "Artist operations" }),
    );
    const splitRegion = screen.getByRole("region", { name: "Split Artist" });
    fireEvent.change(
      within(splitRegion).getByLabelText("Source Artist public id"),
      {
        target: { value: ARTIST_ID },
      },
    );
    fireEvent.change(
      within(splitRegion).getByLabelText("New Artist canonical name"),
      {
        target: { value: "The Nomads (US)" },
      },
    );
    fireEvent.change(within(splitRegion).getByLabelText("Assignment id"), {
      target: { value: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee" },
    });
    fireEvent.change(within(splitRegion).getByLabelText("Reason"), {
      target: { value: "Two bands share the name." },
    });
    const splitButton = within(splitRegion).getByRole("button", {
      name: "Split Artist",
    });
    expect(splitButton).toBeEnabled();
    fireEvent.click(splitButton);
    await waitFor(() =>
      expect(mockSplit).toHaveBeenCalledWith(ARTIST_ID, {
        reason: "Two bands share the name.",
        newArtist: { canonicalName: "The Nomads (US)" },
        assignments: [
          { kind: "releaseArtist", id: "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee" },
        ],
      }),
    );
    expect(
      await screen.findByText("The Artist was split; 1 assignments moved."),
    ).toBeInTheDocument();
  });

  it("renders a queue error notification with retry", async () => {
    mockListUnresolved.mockRejectedValueOnce(
      new ApiError(500, "store_error", "boom", null),
    );
    renderScreen();
    expect(
      await screen.findByText(
        "The unresolved-identities queue could not be loaded",
      ),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole("table", {
        name: "Unresolved artist observations",
      }),
    ).toBeInTheDocument();
  });
});
