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
  AccessGroup,
  AccessGroupDetail,
  UserSummary,
} from "../../api/types";
import { UsersScreen } from "./UsersScreen";

vi.mock("../../api/admin", () => ({
  listUsers: vi.fn(),
  createUser: vi.fn(),
  updateUser: vi.fn(),
  setUserPassword: vi.fn(),
  listGroups: vi.fn(),
  createGroup: vi.fn(),
  getGroup: vi.fn(),
  updateGroup: vi.fn(),
  deleteGroup: vi.fn(),
  addGroupMember: vi.fn(),
  removeGroupMember: vi.fn(),
  bindGroupLibrary: vi.fn(),
  unbindGroupLibrary: vi.fn(),
}));

const admin = await import("../../api/admin");
const mockListUsers = vi.mocked(admin.listUsers);
const mockCreateUser = vi.mocked(admin.createUser);
const mockUpdateUser = vi.mocked(admin.updateUser);
const mockSetUserPassword = vi.mocked(admin.setUserPassword);
const mockListGroups = vi.mocked(admin.listGroups);
const mockCreateGroup = vi.mocked(admin.createGroup);
const mockGetGroup = vi.mocked(admin.getGroup);
const mockDeleteGroup = vi.mocked(admin.deleteGroup);
const mockAddGroupMember = vi.mocked(admin.addGroupMember);
const mockRemoveGroupMember = vi.mocked(admin.removeGroupMember);
const mockBindGroupLibrary = vi.mocked(admin.bindGroupLibrary);
const mockUnbindGroupLibrary = vi.mocked(admin.unbindGroupLibrary);

const USER_ID = "11111111-1111-4111-8111-111111111111";
const GROUP_ID = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const LIBRARY_ID = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";

function makeUser(overrides: Partial<UserSummary> = {}): UserSummary {
  return {
    id: USER_ID,
    username: "operator",
    email: "operator@example.com",
    isAdmin: true,
    isEditor: false,
    enabled: true,
    locked: false,
    roles: ["HasStreamRole"],
    createdAt: "2026-08-14T10:00:00Z",
    updatedAt: "2026-08-14T10:00:00Z",
    ...overrides,
  };
}

function makeGroup(overrides: Partial<AccessGroup> = {}): AccessGroup {
  return {
    id: GROUP_ID,
    name: "Family",
    memberCount: 1,
    bindingCount: 0,
    createdAt: "2026-08-14T10:00:00Z",
    updatedAt: "2026-08-14T10:00:00Z",
    ...overrides,
  };
}

function makeGroupDetail(
  overrides: Partial<AccessGroupDetail> = {},
): AccessGroupDetail {
  return {
    ...makeGroup(),
    members: [{ userId: USER_ID, username: "operator" }],
    libraryIds: [],
    ...overrides,
  };
}

function renderScreen(initialEntry = "/system/users") {
  return render(
    <MemoryRouter initialEntries={[initialEntry]}>
      <UsersScreen />
    </MemoryRouter>,
  );
}

describe("UsersScreen users tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListUsers.mockResolvedValue({
      items: [makeUser()],
      total: 1,
      limit: 50,
      offset: 0,
    });
    mockListGroups.mockResolvedValue([makeGroup()]);
  });

  it("lists users with query/sort/paging reflected in the request", async () => {
    renderScreen(
      "/system/users?query=op&sort=createdAt:desc&page=2&pageSize=25",
    );
    expect(await screen.findByText("operator")).toBeInTheDocument();
    await waitFor(() =>
      expect(mockListUsers).toHaveBeenLastCalledWith({
        query: "op",
        limit: 25,
        offset: 25,
        sort: "createdAt:desc",
      }),
    );
  });

  it("creates a user with the exact request body (roles omitted when empty)", async () => {
    mockCreateUser.mockResolvedValue(makeUser({ username: "newuser" }));
    renderScreen();
    fireEvent.click(await screen.findByRole("button", { name: "Create user" }));

    const dialog = await screen.findByRole("dialog", { name: "Create user" });
    fireEvent.change(within(dialog).getByLabelText("Username"), {
      target: { value: "newuser" },
    });
    fireEvent.change(within(dialog).getByLabelText("Email"), {
      target: { value: "new@example.com" },
    });
    fireEvent.change(within(dialog).getByLabelText("Password"), {
      target: { value: "correct horse" },
    });
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Editor" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));

    await waitFor(() =>
      expect(mockCreateUser).toHaveBeenCalledWith({
        username: "newuser",
        email: "new@example.com",
        password: "correct horse",
        isAdmin: false,
        isEditor: true,
      }),
    );
    expect(await screen.findByText("User created.")).toBeInTheDocument();
  });

  it("blocks create while the password is shorter than 8 characters", async () => {
    renderScreen();
    fireEvent.click(await screen.findByRole("button", { name: "Create user" }));
    const dialog = await screen.findByRole("dialog", { name: "Create user" });
    fireEvent.change(within(dialog).getByLabelText("Username"), {
      target: { value: "newuser" },
    });
    fireEvent.change(within(dialog).getByLabelText("Email"), {
      target: { value: "new@example.com" },
    });
    fireEvent.change(within(dialog).getByLabelText("Password"), {
      target: { value: "short" },
    });
    expect(
      within(dialog).getByRole("button", { name: "Create" }),
    ).toBeDisabled();
    expect(mockCreateUser).not.toHaveBeenCalled();
  });

  it("edits flags and roles with the exact update body", async () => {
    mockUpdateUser.mockResolvedValue(makeUser({ locked: true }));
    renderScreen();
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));

    const dialog = await screen.findByRole("dialog", { name: "Edit operator" });
    fireEvent.click(within(dialog).getByRole("checkbox", { name: "Locked" }));
    fireEvent.change(within(dialog).getByLabelText("Roles"), {
      target: { value: "HasStreamRole, Administrator" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Save changes" }),
    );

    await waitFor(() =>
      expect(mockUpdateUser).toHaveBeenCalledWith(USER_ID, {
        isAdmin: true,
        isEditor: false,
        enabled: true,
        locked: true,
        roles: ["HasStreamRole", "Administrator"],
      }),
    );
    expect(await screen.findByText("User updated.")).toBeInTheDocument();
  });

  it("sets a password with the exact request body", async () => {
    mockSetUserPassword.mockResolvedValue(undefined);
    renderScreen();
    fireEvent.click(
      await screen.findByRole("button", { name: "Set password" }),
    );

    const dialog = await screen.findByRole("dialog", {
      name: /Set password for operator/,
    });
    fireEvent.change(within(dialog).getByLabelText("Password"), {
      target: { value: "new password 1" },
    });
    fireEvent.change(within(dialog).getByLabelText("Confirm password"), {
      target: { value: "new password 1" },
    });
    fireEvent.click(
      within(dialog).getByRole("button", { name: "Set password" }),
    );

    await waitFor(() =>
      expect(mockSetUserPassword).toHaveBeenCalledWith(
        USER_ID,
        "new password 1",
      ),
    );
    expect(
      await screen.findByText(
        "Password updated; existing sessions were revoked.",
      ),
    ).toBeInTheDocument();
  });

  it("surfaces a localized 409 user_conflict on create", async () => {
    mockCreateUser.mockRejectedValue(
      new ApiError(409, "user_conflict", "user_conflict", null),
    );
    renderScreen();
    fireEvent.click(await screen.findByRole("button", { name: "Create user" }));
    const dialog = await screen.findByRole("dialog", { name: "Create user" });
    fireEvent.change(within(dialog).getByLabelText("Username"), {
      target: { value: "operator" },
    });
    fireEvent.change(within(dialog).getByLabelText("Email"), {
      target: { value: "operator@example.com" },
    });
    fireEvent.change(within(dialog).getByLabelText("Password"), {
      target: { value: "correct horse" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(
      await within(dialog).findByText(
        "A user with this username or email already exists.",
      ),
    ).toBeInTheDocument();
  });
});

describe("UsersScreen groups tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListUsers.mockResolvedValue({
      items: [makeUser()],
      total: 1,
      limit: 500,
      offset: 0,
    });
    mockListGroups.mockResolvedValue([makeGroup()]);
    mockGetGroup.mockResolvedValue(makeGroupDetail());
  });

  it("lists groups and manages members and library bindings", async () => {
    mockAddGroupMember.mockResolvedValue(undefined);
    mockRemoveGroupMember.mockResolvedValue(undefined);
    mockBindGroupLibrary.mockResolvedValue(undefined);
    mockUnbindGroupLibrary.mockResolvedValue(undefined);
    renderScreen("/system/users?tab=groups");

    const groupsTable = await screen.findByRole("table", {
      name: "Library access groups",
    });
    expect(within(groupsTable).getByText("Family")).toBeInTheDocument();

    // Open the detail panel.
    fireEvent.click(
      within(groupsTable).getByRole("button", { name: "Family" }),
    );
    expect(await screen.findByText("Group: Family")).toBeInTheDocument();
    expect(screen.getByText("operator")).toBeInTheDocument();

    // Remove the existing member.
    mockGetGroup.mockResolvedValue(makeGroupDetail({ members: [] }));
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(mockRemoveGroupMember).toHaveBeenCalledWith(GROUP_ID, USER_ID),
    );

    // Bind a library by public id.
    fireEvent.change(screen.getByLabelText("Library public id"), {
      target: { value: LIBRARY_ID },
    });
    mockGetGroup.mockResolvedValue(
      makeGroupDetail({ members: [], libraryIds: [LIBRARY_ID] }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Bind library" }));
    await waitFor(() =>
      expect(mockBindGroupLibrary).toHaveBeenCalledWith(GROUP_ID, LIBRARY_ID),
    );

    // Unbind it again.
    expect(await screen.findByText(LIBRARY_ID)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Unbind" }));
    await waitFor(() =>
      expect(mockUnbindGroupLibrary).toHaveBeenCalledWith(GROUP_ID, LIBRARY_ID),
    );
  });

  it("creates a group with the exact request body", async () => {
    mockCreateGroup.mockResolvedValue(makeGroup({ name: "Kids" }));
    renderScreen("/system/users?tab=groups");
    fireEvent.click(
      await screen.findByRole("button", { name: "Create group" }),
    );
    const dialog = await screen.findByRole("dialog", { name: "Create group" });
    fireEvent.change(within(dialog).getByLabelText("Name"), {
      target: { value: "Kids" },
    });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(mockCreateGroup).toHaveBeenCalledWith("Kids"));
  });

  it("surfaces the localized 409 group_not_empty on delete", async () => {
    mockDeleteGroup.mockRejectedValue(
      new ApiError(409, "group_not_empty", "group_not_empty", null),
    );
    renderScreen("/system/users?tab=groups");
    const groupsTable = await screen.findByRole("table", {
      name: "Library access groups",
    });
    fireEvent.click(
      within(groupsTable).getByRole("button", { name: "Delete" }),
    );

    const dialog = await screen.findByRole("dialog", {
      name: "Delete group Family",
    });
    // The danger modal states the impact before confirming.
    expect(within(dialog).getByText(/still has members/)).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(
      await screen.findByText(
        "The group still has members or library bindings. Remove them before deleting.",
      ),
    ).toBeInTheDocument();
  });
});
