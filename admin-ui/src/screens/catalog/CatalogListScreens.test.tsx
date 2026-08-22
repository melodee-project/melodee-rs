import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  CatalogArtistListItem,
  CatalogReleaseListItem,
  CatalogTrackListItem,
  Page,
} from "../../api/types";
import { ArtistsScreen } from "./ArtistsScreen";
import { ReleasesScreen } from "./ReleasesScreen";
import { TracksScreen } from "./TracksScreen";

vi.mock("../../api/admin", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../api/admin")>();
  return {
    ...original,
    listCatalogArtists: vi.fn(),
    listCatalogReleases: vi.fn(),
    listCatalogTracks: vi.fn(),
  };
});

const { listCatalogArtists, listCatalogReleases, listCatalogTracks } =
  await import("../../api/admin");
const mockListArtists = vi.mocked(listCatalogArtists);
const mockListReleases = vi.mocked(listCatalogReleases);
const mockListTracks = vi.mocked(listCatalogTracks);

const ARTIST: CatalogArtistListItem = {
  id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  name: "The Nomads",
  sortName: "Nomads, The",
  artistType: "group",
  disambiguation: "Australian rock band",
  status: "verified",
  releaseCount: 12,
  trackCount: 128,
  imageId: "artist_aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  createdAt: "2026-08-01T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const RELEASE: CatalogReleaseListItem = {
  id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
  title: "Into the Night",
  displayArtist: "The Nomads",
  releaseDate: "1986-05-12",
  releaseDateText: null,
  status: "active",
  trackCount: 10,
  durationMs: 2_280_000,
  imageId: "album_bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
  createdAt: "2026-08-01T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const TRACK: CatalogTrackListItem = {
  id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
  title: "Down Under",
  displayArtist: "The Nomads",
  ordinal: 3,
  discNumber: 1,
  trackNumber: 3,
  durationMs: 222_000,
  releaseId: RELEASE.id,
  releaseTitle: RELEASE.title,
  imageId: "song_cccccccc-cccc-4ccc-8ccc-cccccccccccc",
  createdAt: "2026-08-01T10:00:00Z",
};

function page<T>(items: T[], total = items.length): Page<T> {
  return { items, total, limit: 50, offset: 0 };
}

beforeEach(() => {
  vi.clearAllMocks();
  mockListArtists.mockResolvedValue(page([ARTIST], 137));
  mockListReleases.mockResolvedValue(page([RELEASE], 137));
  mockListTracks.mockResolvedValue(page([TRACK], 137));
});

describe("ArtistsScreen", () => {
  it("renders the server envelope and maps query/sort/page URL state to request params", async () => {
    render(
      <MemoryRouter
        initialEntries={["/catalog/artists?sort=name:desc&page=2&pageSize=25"]}
      >
        <ArtistsScreen />
      </MemoryRouter>,
    );
    expect(
      await screen.findByRole("link", { name: "The Nomads" }),
    ).toHaveAttribute("href", `/catalog/artists/${ARTIST.id}`);
    expect(mockListArtists).toHaveBeenCalledWith({
      query: undefined,
      limit: 25,
      offset: 25,
      sort: "name:desc",
    });
    // Total-count pagination: 137 server-side rows over one rendered row.
    expect(screen.getByText(/137/)).toBeInTheDocument();
    expect(screen.getByText("Group")).toBeInTheDocument();
    expect(
      screen.getByRole("link", { name: "The Nomads" }),
    ).toBeInTheDocument();
  });

  it("binds the search box to the query filter and header clicks to whitelisted sort", async () => {
    render(
      <MemoryRouter initialEntries={["/catalog/artists"]}>
        <ArtistsScreen />
      </MemoryRouter>,
    );
    await screen.findByRole("link", { name: "The Nomads" });
    expect(mockListArtists).toHaveBeenLastCalledWith({
      query: undefined,
      limit: 50,
      offset: 0,
      sort: undefined,
    });

    fireEvent.change(
      screen.getByRole("searchbox", { name: "Search artists" }),
      {
        target: { value: "nomads" },
      },
    );
    await waitFor(
      () => {
        expect(mockListArtists).toHaveBeenLastCalledWith(
          expect.objectContaining({ query: "nomads", offset: 0 }),
        );
      },
      { timeout: 2000 },
    );

    fireEvent.click(screen.getByRole("button", { name: "Sort by Name" }));
    await waitFor(() => {
      expect(mockListArtists).toHaveBeenLastCalledWith(
        expect.objectContaining({ sort: "name:asc" }),
      );
    });
  });
});

describe("ReleasesScreen", () => {
  it("renders the envelope and maps URL state to request params", async () => {
    render(
      <MemoryRouter
        initialEntries={[
          "/catalog/releases?query=night&sort=releaseDate:asc&page=3&pageSize=25",
        ]}
      >
        <ReleasesScreen />
      </MemoryRouter>,
    );
    expect(
      await screen.findByRole("link", { name: "Into the Night" }),
    ).toHaveAttribute("href", `/catalog/releases/${RELEASE.id}`);
    expect(mockListReleases).toHaveBeenCalledWith({
      query: "night",
      limit: 25,
      offset: 50,
      sort: "releaseDate:asc",
    });
    expect(screen.getByText("The Nomads")).toBeInTheDocument();
  });

  it("honors the sort whitelist by ignoring unknown sort keys", async () => {
    render(
      <MemoryRouter initialEntries={["/catalog/releases?sort=haxx:asc"]}>
        <ReleasesScreen />
      </MemoryRouter>,
    );
    await screen.findByRole("link", { name: "Into the Night" });
    expect(mockListReleases).toHaveBeenCalledWith(
      expect.objectContaining({ sort: undefined }),
    );
  });
});

describe("TracksScreen", () => {
  it("renders the envelope with release linkage and maps URL state to request params", async () => {
    render(
      <MemoryRouter
        initialEntries={[
          "/catalog/tracks?sort=durationMs:desc&page=2&pageSize=25",
        ]}
      >
        <TracksScreen />
      </MemoryRouter>,
    );
    expect(
      await screen.findByRole("link", { name: "Down Under" }),
    ).toHaveAttribute("href", `/catalog/tracks/${TRACK.id}`);
    expect(
      screen.getByRole("link", { name: "Into the Night" }),
    ).toHaveAttribute("href", `/catalog/releases/${RELEASE.id}`);
    expect(mockListTracks).toHaveBeenCalledWith({
      query: undefined,
      limit: 25,
      offset: 25,
      sort: "durationMs:desc",
    });
  });

  it("shows the localized empty state when the server page is empty", async () => {
    mockListTracks.mockResolvedValue(page([], 0));
    render(
      <MemoryRouter initialEntries={["/catalog/tracks?query=zzz"]}>
        <TracksScreen />
      </MemoryRouter>,
    );
    expect(await screen.findByText("No tracks")).toBeInTheDocument();
  });
});
