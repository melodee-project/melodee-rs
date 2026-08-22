import { expect, test, type Page } from "@playwright/test";

/**
 * Users & Access e2e . The admin API is mocked with page.route —
 * no live server. Covers the users table, create/edit/set-password request
 * bodies, and the ar-SA dir=rtl smoke on the users screen.
 */

const USER = {
  id: "11111111-1111-4111-8111-111111111111",
  username: "operator",
  email: "operator@example.com",
  isAdmin: true,
  isEditor: false,
  enabled: true,
  locked: false,
  roles: ["HasStreamRole"],
  createdAt: "2026-08-14T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

const GROUP = {
  id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  name: "Family",
  memberCount: 1,
  bindingCount: 0,
  createdAt: "2026-08-14T10:00:00Z",
  updatedAt: "2026-08-14T10:00:00Z",
};

interface Captured {
  createdUser: unknown;
  updatedUser: unknown;
  passwordSet: unknown;
}

async function mockAdminApi(page: Page): Promise<Captured> {
  const captured: Captured = {
    createdUser: null,
    updatedUser: null,
    passwordSet: null,
  };
  await page.route("**/api/**", async (route) => {
    const url = new URL(route.request().url());
    const { pathname } = url;
    const method = route.request().method();
    const json = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (pathname === "/api/admin/v1/system/info") {
      await json({ service: "melodee", version: "test", status: "ok" });
      return;
    }
    if (pathname === "/api/admin/v1/users" && method === "GET") {
      await json({ items: [USER], total: 1, limit: 50, offset: 0 });
      return;
    }
    if (pathname === "/api/admin/v1/users" && method === "POST") {
      captured.createdUser = route.request().postDataJSON();
      await json({ ...USER, username: "newuser" }, 201);
      return;
    }
    if (pathname === `/api/admin/v1/users/${USER.id}` && method === "PUT") {
      captured.updatedUser = route.request().postDataJSON();
      await json(USER);
      return;
    }
    if (
      pathname === `/api/admin/v1/users/${USER.id}/password` &&
      method === "POST"
    ) {
      captured.passwordSet = route.request().postDataJSON();
      await route.fulfill({ status: 204 });
      return;
    }
    if (pathname === "/api/admin/v1/groups") {
      await json([GROUP]);
      return;
    }
    if (pathname === `/api/admin/v1/groups/${GROUP.id}`) {
      await json({
        ...GROUP,
        members: [{ userId: USER.id, username: "operator" }],
        libraryIds: [],
      });
      return;
    }
    if (pathname === "/api/admin/v1/onboarding/status") {
      await json({
        completed: true,
        bypassed: false,
        canComplete: true,
        completedBy: "admin",
        checks: [],
      });
      return;
    }
    await json(
      { code: "not_found", message: "not found", correlationId: null },
      404,
    );
  });
  return captured;
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    window.localStorage.setItem("melodee.locale", "en-US");
  });
});

test("users table lists accounts with status and role tags", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto("system/users");

  const table = page.getByRole("table", { name: "User accounts" });
  await expect(table).toBeVisible();
  await expect(table.getByText("operator", { exact: true })).toBeVisible();
  await expect(table.getByText("operator@example.com")).toBeVisible();
  await expect(table.getByText("Administrator")).toBeVisible();
  await expect(table.getByText("Enabled")).toBeVisible();
});

test("create, edit, and set-password send the exact request bodies", async ({
  page,
}) => {
  const captured = await mockAdminApi(page);
  await page.goto("system/users");
  const table = page.getByRole("table", { name: "User accounts" });
  await expect(table.getByText("operator", { exact: true })).toBeVisible();

  // Create.
  await page.getByRole("button", { name: "Create user" }).click();
  const createDialog = page.getByRole("dialog", { name: "Create user" });
  await createDialog.getByLabel("Username").fill("newuser");
  await createDialog.getByLabel("Email").fill("new@example.com");
  await createDialog.getByLabel("Password").fill("correct horse");
  // Carbon renders the input visually hidden; the label toggles it.
  await createDialog.getByText("Editor", { exact: true }).click();
  await createDialog
    .getByRole("button", { name: "Create", exact: true })
    .click();
  await expect
    .poll(() => captured.createdUser)
    .toEqual({
      username: "newuser",
      email: "new@example.com",
      password: "correct horse",
      isAdmin: false,
      isEditor: true,
    });
  await expect(page.getByText("User created.")).toBeVisible();

  // Edit: lock the account.
  await table.getByRole("button", { name: "Edit" }).click();
  const editDialog = page.getByRole("dialog", { name: /Edit operator/ });
  // Carbon renders the input visually hidden; the label toggles it.
  await editDialog.getByText("Locked", { exact: true }).click();
  await editDialog.getByRole("button", { name: "Save changes" }).click();
  await expect
    .poll(() => captured.updatedUser)
    .toEqual({
      isAdmin: true,
      isEditor: false,
      enabled: true,
      locked: true,
      roles: ["HasStreamRole"],
    });

  // Set password (204 surface).
  await table.getByRole("button", { name: "Set password" }).click();
  const passwordDialog = page.getByRole("dialog", {
    name: /Set password for operator/,
  });
  await passwordDialog
    .getByLabel("Password", { exact: true })
    .fill("new password 1");
  await passwordDialog.getByLabel("Confirm password").fill("new password 1");
  await passwordDialog.getByRole("button", { name: "Set password" }).click();
  await expect
    .poll(() => captured.passwordSet)
    .toEqual({ password: "new password 1" });
  await expect(
    page.getByText("Password updated; existing sessions were revoked."),
  ).toBeVisible();
});

test("groups tab lists groups with members in the detail panel", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto("system/users?tab=groups");

  const table = page.getByRole("table", { name: "Library access groups" });
  await expect(table).toBeVisible();
  await table.getByRole("button", { name: "Family" }).click();
  await expect(page.getByText("Group: Family")).toBeVisible();
  await expect(page.getByText("operator")).toBeVisible();
});

test("rtl smoke: ar-SA renders the users screen with dir=rtl", async ({
  page,
}) => {
  await mockAdminApi(page);
  await page.goto("system/users");
  await expect(
    page.getByRole("table", { name: "User accounts" }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Language", exact: true }).click();
  await page.getByText(/العربية/).click();

  await expect(page.locator("html")).toHaveAttribute("dir", "rtl");
  await expect(page.locator("html")).toHaveAttribute("lang", "ar-SA");
  // The users table remains rendered under RTL (localized or source text).
  await expect(page.getByRole("table").first()).toBeVisible();
});
