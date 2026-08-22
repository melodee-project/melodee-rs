import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ApiError } from "../../api/client";
import { ArtworkManager } from "./ArtworkManager";

vi.mock("../../api/admin", async (importOriginal) => {
  const original = await importOriginal<typeof import("../../api/admin")>();
  return {
    ...original,
    uploadCatalogImage: vi.fn(),
    removeCatalogImage: vi.fn(),
  };
});

const { uploadCatalogImage, removeCatalogImage } =
  await import("../../api/admin");
const mockUpload = vi.mocked(uploadCatalogImage);
const mockRemove = vi.mocked(removeCatalogImage);

const PROPS = {
  entity: "artists" as const,
  entityId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  imageId: "artist_aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  name: "The Nomads",
};

function chooseFile(file: File) {
  const input = screen.getByLabelText("Choose image file");
  fireEvent.change(input, { target: { files: [file] } });
}

function pngFile(name = "cover.png", size?: number) {
  const file = new File(["png-bytes"], name, { type: "image/png" });
  if (size !== undefined) {
    Object.defineProperty(file, "size", { value: size });
  }
  return file;
}

/** The Carbon Modal keeps its footer buttons mounted, so the trigger and
 * the dialog's danger button share the action label; disambiguate by the
 * modal container. */
function removeButtons() {
  const buttons = screen.getAllByRole("button", {
    name: "Remove custom artwork",
  });
  return {
    trigger: buttons.find((button) => !button.closest(".cds--modal")),
    confirm: buttons.find((button) => button.closest(".cds--modal")),
  };
}

describe("ArtworkManager (ADR-0143)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("renders the current artwork and the upload/remove affordances", () => {
    const onChanged = vi.fn();
    render(<ArtworkManager {...PROPS} onChanged={onChanged} />);
    const image = screen.getByRole("img", { name: "The Nomads" });
    expect(image.getAttribute("src")).toContain(
      "/images/artist_aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa/medium",
    );
    expect(screen.getByLabelText("Choose image file")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Upload artwork" }),
    ).toBeDisabled();
    expect(removeButtons().trigger).toBeEnabled();
  });

  it("rejects a non-jpeg/png file client-side without calling the API", () => {
    render(<ArtworkManager {...PROPS} onChanged={vi.fn()} />);
    chooseFile(new File(["x"], "notes.txt", { type: "text/plain" }));
    expect(
      screen.getByText("Only JPEG or PNG images can be uploaded."),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Upload artwork" }),
    ).toBeDisabled();
    expect(mockUpload).not.toHaveBeenCalled();
  });

  it("rejects an oversized file client-side without calling the API", () => {
    render(<ArtworkManager {...PROPS} onChanged={vi.fn()} />);
    chooseFile(pngFile("huge.png", 10 * 1024 * 1024 + 1));
    expect(screen.getByText(/between 1 byte and/)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Upload artwork" }),
    ).toBeDisabled();
    expect(mockUpload).not.toHaveBeenCalled();
  });

  it("uploads the chosen file, refreshes the artwork, and notifies the parent", async () => {
    mockUpload.mockResolvedValue({
      message: "Artwork updated",
      imageId: PROPS.imageId,
    });
    const onChanged = vi.fn();
    render(<ArtworkManager {...PROPS} onChanged={onChanged} />);
    const file = pngFile();
    chooseFile(file);
    expect(screen.getByText(/Selected: cover.png/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Upload artwork" }));
    await waitFor(() =>
      expect(mockUpload).toHaveBeenCalledWith("artists", PROPS.entityId, file),
    );
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
    expect(screen.queryByText(/Selected:/)).not.toBeInTheDocument();
    // The refresh key busts the cached image URL.
    expect(
      screen.getByRole("img", { name: "The Nomads" }).getAttribute("src"),
    ).toContain("?v=1");
  });

  it("surfaces a localized error when the server rejects the upload (400)", async () => {
    mockUpload.mockRejectedValue(
      new ApiError(400, "invalid_image", "nope", null),
    );
    render(<ArtworkManager {...PROPS} onChanged={vi.fn()} />);
    chooseFile(pngFile());
    fireEvent.click(screen.getByRole("button", { name: "Upload artwork" }));
    await screen.findByText(/The server rejected the image/);
  });

  it("removes the override through the destructive-confirm dialog", async () => {
    mockRemove.mockResolvedValue(undefined);
    const onChanged = vi.fn();
    render(<ArtworkManager {...PROPS} onChanged={onChanged} />);
    fireEvent.click(removeButtons().trigger!);
    const dialogButton = removeButtons().confirm;
    expect(dialogButton).toBeDefined();
    expect(dialogButton).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Reason"), {
      target: { value: "wrong photo" },
    });
    expect(dialogButton).toBeEnabled();
    fireEvent.click(dialogButton!);
    await waitFor(() =>
      expect(mockRemove).toHaveBeenCalledWith(
        "artists",
        PROPS.entityId,
        "wrong photo",
      ),
    );
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("surfaces a localized error when the entity vanished (404 on remove)", async () => {
    mockRemove.mockRejectedValue(new ApiError(404, "not_found", "gone", null));
    render(<ArtworkManager {...PROPS} onChanged={vi.fn()} />);
    fireEvent.click(removeButtons().trigger!);
    fireEvent.change(await screen.findByLabelText("Reason"), {
      target: { value: "cleanup" },
    });
    fireEvent.click(removeButtons().confirm!);
    await screen.findByText(/no longer exists/);
  });
});
