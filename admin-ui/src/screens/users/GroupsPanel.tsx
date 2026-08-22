import { Add, TrashCan } from "@carbon/icons-react";
import {
  ActionableNotification,
  Button,
  ComboBox,
  Heading,
  InlineNotification,
  Loading,
  Modal,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
  TextInput,
} from "@carbon/react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  addGroupMember,
  bindGroupLibrary,
  createGroup,
  deleteGroup,
  getGroup,
  listGroups,
  listUsers,
  removeGroupMember,
  unbindGroupLibrary,
  updateGroup,
} from "../../api/admin";
import { ApiError } from "../../api/client";
import type {
  AccessGroup,
  AccessGroupDetail,
  UserSummary,
} from "../../api/types";
import { EmptyState } from "../../components/EmptyState";
import {
  formatDateTime,
  formatNumber,
  formatRelativeTime,
} from "../../i18n/format";

/**
 * Library access groups tab (PRD-WEB-002): group list with create /
 * rename / delete plus a detail panel for member management and library
 * bindings. The endpoints return bounded arrays (no paging envelope), so
 * plain Carbon tables are used rather than the server-paged EntityTable.
 * Deletes are destructive-class: a danger modal with explicit impact text
 * (the API fails closed with 409 group_not_empty when members or bindings
 * remain). No fake affordances: there is no library-picker endpoint, so
 * bindings take a library public id with helper text saying so.
 */

interface Notice {
  kind: "success" | "error" | "info" | "warning";
  text: string;
}

function toApiError(cause: unknown): ApiError {
  return cause instanceof ApiError
    ? cause
    : new ApiError(0, "unknown", String(cause), null);
}

/** Localized text for the stable group-mutation error codes. */
function groupErrorText(
  error: ApiError,
  t: ReturnType<typeof useTranslation>["t"],
): string {
  if (error.status === 409 && error.code === "group_exists") {
    return t("users.groups.error.exists");
  }
  if (error.status === 409 && error.code === "group_name_ambiguous") {
    return t("users.groups.error.ambiguous");
  }
  if (error.status === 409 && error.code === "group_not_empty") {
    return t("users.groups.error.notEmpty");
  }
  if (error.status === 400 && error.code === "invalid_group_name") {
    return t("users.groups.error.invalidName");
  }
  if (error.status === 404) {
    return t("users.groups.error.notFound");
  }
  return t("users.groups.error.mutation", { message: error.message });
}

export function GroupsPanel() {
  const { t } = useTranslation();
  const [groups, setGroups] = useState<AccessGroup[] | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [selected, setSelected] = useState<AccessGroupDetail | null>(null);
  const [selectedError, setSelectedError] = useState<ApiError | null>(null);
  const [allUsers, setAllUsers] = useState<UserSummary[]>([]);

  const [createOpen, setCreateOpen] = useState(false);
  const [renameTarget, setRenameTarget] = useState<AccessGroup | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<AccessGroup | null>(null);

  const fetchGroups = useCallback((signal: { cancelled: boolean }) => {
    return listGroups()
      .then((result) => {
        if (!signal.cancelled) {
          setGroups(result);
          setListError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setListError(toApiError(cause));
        }
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchGroups(signal);
    // The add-member picker needs the user directory; bounded at 500 rows.
    listUsers({ limit: 500 })
      .then((page) => {
        if (!signal.cancelled) {
          setAllUsers(page.items);
        }
      })
      .catch(() => {
        // The picker degrades to empty; membership display is unaffected.
      });
    return () => {
      signal.cancelled = true;
    };
  }, [fetchGroups]);

  const openGroup = useCallback((id: string) => {
    setSelectedError(null);
    getGroup(id)
      .then(setSelected)
      .catch((cause: unknown) => {
        setSelected(null);
        setSelectedError(toApiError(cause));
      });
  }, []);

  const runMutation = (action: () => Promise<unknown>, successKey?: string) => {
    action()
      .then(() => {
        if (successKey) {
          setNotice({ kind: "success", text: t(successKey) });
        }
        void fetchGroups({ cancelled: false });
        if (selected) {
          openGroup(selected.id);
        }
      })
      .catch((cause: unknown) => {
        setNotice({
          kind: "error",
          text: groupErrorText(toApiError(cause), t),
        });
      });
  };

  const nonMembers = selected
    ? allUsers.filter(
        (user) => !selected.members.some((member) => member.userId === user.id),
      )
    : [];

  return (
    <div>
      {notice ? (
        <InlineNotification
          kind={notice.kind}
          lowContrast
          title={notice.text}
          onCloseButtonClick={() => setNotice(null)}
        />
      ) : null}
      {listError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t("users.groups.loadErrorTitle")}
          subtitle={listError.message}
          hideCloseButton
          actionButtonLabel={t("actions.retry")}
          onActionButtonClick={() => void fetchGroups({ cancelled: false })}
        />
      ) : null}
      {groups === null && !listError ? (
        <Loading description={t("common.loading")} withOverlay={false} />
      ) : null}
      {groups !== null ? (
        // The groups table is wider than the sm viewport; it scrolls inside
        // its own container rather than expanding the page (UI-SPEC §8).
        <div className="melodee-table-scroll">
          <TableContainer
            title={t("users.groups.tableCaption")}
            description={t("users.groups.description")}
          >
            <Table aria-label={t("users.groups.tableCaption")}>
              <TableHead>
                <TableRow>
                  <TableHeader>{t("common.name")}</TableHeader>
                  <TableHeader>{t("users.groups.column.members")}</TableHeader>
                  <TableHeader>
                    {t("users.groups.column.libraries")}
                  </TableHeader>
                  <TableHeader>{t("common.created")}</TableHeader>
                  <TableHeader>{t("users.column.actions")}</TableHeader>
                </TableRow>
              </TableHead>
              <TableBody>
                {groups.map((group) => (
                  <TableRow key={group.id}>
                    <TableCell>
                      <Button
                        kind="ghost"
                        size="sm"
                        onClick={() => openGroup(group.id)}
                      >
                        {group.name}
                      </Button>
                    </TableCell>
                    <TableCell>{formatNumber(group.memberCount)}</TableCell>
                    <TableCell>{formatNumber(group.bindingCount)}</TableCell>
                    <TableCell>
                      <span title={formatDateTime(group.createdAt)}>
                        {formatRelativeTime(group.createdAt)}
                      </span>
                    </TableCell>
                    <TableCell>
                      <Button
                        kind="ghost"
                        size="sm"
                        onClick={() => setRenameTarget(group)}
                      >
                        {t("users.groups.rename")}
                      </Button>
                      <Button
                        kind="danger--ghost"
                        size="sm"
                        renderIcon={TrashCan}
                        onClick={() => setDeleteTarget(group)}
                      >
                        {t("actions.delete")}
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            {groups.length === 0 ? (
              <EmptyState
                compact
                title={t("users.groups.emptyTitle")}
                description={t("users.groups.emptyDescription")}
              />
            ) : null}
          </TableContainer>
        </div>
      ) : null}
      <div style={{ marginBlockStart: "1rem" }}>
        <Button
          kind="primary"
          size="sm"
          renderIcon={Add}
          onClick={() => setCreateOpen(true)}
        >
          {t("users.groups.create")}
        </Button>
      </div>

      {selected ? (
        <section className="melodee-panel" aria-labelledby="group-detail-title">
          <Heading id="group-detail-title" className="melodee-panel__title">
            {t("users.groups.detailTitle", { name: selected.name })}
          </Heading>
          <Heading className="melodee-panel__subtitle">
            {t("users.groups.membersTitle")}
          </Heading>
          {selected.members.length === 0 ? (
            <p className="melodee-panel__subtitle">
              {t("users.groups.membersEmpty")}
            </p>
          ) : (
            <ul>
              {selected.members.map((member) => (
                <li key={member.userId}>
                  {member.username}{" "}
                  <Button
                    kind="danger--ghost"
                    size="sm"
                    onClick={() =>
                      runMutation(
                        () => removeGroupMember(selected.id, member.userId),
                        "users.groups.memberRemoved",
                      )
                    }
                  >
                    {t("actions.remove")}
                  </Button>
                </li>
              ))}
            </ul>
          )}
          <ComboBox
            id="group-add-member"
            titleText={t("users.groups.addMember")}
            helperText={t("users.groups.addMemberHelp")}
            items={nonMembers}
            itemToString={(user: UserSummary | null) =>
              user ? user.username : ""
            }
            onChange={({
              selectedItem,
            }: {
              selectedItem?: UserSummary | null;
            }) => {
              if (selectedItem) {
                runMutation(
                  () => addGroupMember(selected.id, selectedItem.id),
                  "users.groups.memberAdded",
                );
              }
            }}
          />
          <Heading className="melodee-panel__subtitle">
            {t("users.groups.bindingsTitle")}
          </Heading>
          {selected.libraryIds.length === 0 ? (
            <p className="melodee-panel__subtitle">
              {t("users.groups.bindingsEmpty")}
            </p>
          ) : (
            <ul>
              {selected.libraryIds.map((libraryId) => (
                <li key={libraryId}>
                  <code>{libraryId}</code>{" "}
                  <Button
                    kind="danger--ghost"
                    size="sm"
                    onClick={() =>
                      runMutation(
                        () => unbindGroupLibrary(selected.id, libraryId),
                        "users.groups.libraryUnbound",
                      )
                    }
                  >
                    {t("users.groups.unbind")}
                  </Button>
                </li>
              ))}
            </ul>
          )}
          <LibraryBindForm
            onBind={(libraryId) =>
              runMutation(
                () => bindGroupLibrary(selected.id, libraryId),
                "users.groups.libraryBound",
              )
            }
          />
        </section>
      ) : null}
      {selectedError ? (
        <InlineNotification
          kind="error"
          lowContrast
          title={t("users.groups.detailError")}
          subtitle={selectedError.message}
          hideCloseButton
        />
      ) : null}

      <GroupNameModal
        // Remount on each open so the name field resets.
        key={String(createOpen)}
        open={createOpen}
        title={t("users.groups.createTitle")}
        initialName=""
        onClose={() => setCreateOpen(false)}
        onSubmit={(name) => {
          setCreateOpen(false);
          runMutation(() => createGroup(name), "users.groups.createSuccess");
        }}
      />
      <GroupNameModal
        key={renameTarget?.id ?? "closed"}
        open={renameTarget !== null}
        title={t("users.groups.renameTitle", {
          name: renameTarget?.name ?? "",
        })}
        initialName={renameTarget?.name ?? ""}
        onClose={() => setRenameTarget(null)}
        onSubmit={(name) => {
          const target = renameTarget;
          setRenameTarget(null);
          if (target) {
            runMutation(
              () => updateGroup(target.id, name),
              "users.groups.renameSuccess",
            );
          }
        }}
      />
      {deleteTarget !== null ? (
        <Modal
          danger
          open
          modalHeading={t("users.groups.deleteTitle", {
            name: deleteTarget.name,
          })}
          primaryButtonText={t("actions.delete")}
          secondaryButtonText={t("actions.cancel")}
          onRequestClose={() => setDeleteTarget(null)}
          onRequestSubmit={() => {
            const target = deleteTarget;
            setDeleteTarget(null);
            if (target) {
              if (selected?.id === target.id) {
                setSelected(null);
              }
              runMutation(
                () => deleteGroup(target.id),
                "users.groups.deleteSuccess",
              );
            }
          }}
        >
          <p>
            {t("users.groups.deleteBody", {
              members: formatNumber(deleteTarget.memberCount),
              bindings: formatNumber(deleteTarget.bindingCount),
            })}
          </p>
        </Modal>
      ) : null}
    </div>
  );
}

/** Shared create/rename modal: the only field a group carries is its name. */
function GroupNameModal({
  open,
  title,
  initialName,
  onClose,
  onSubmit,
}: {
  open: boolean;
  title: string;
  initialName: string;
  onClose: () => void;
  onSubmit: (name: string) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(initialName);
  const valid = name.trim().length > 0;
  // Closed modals are not rendered (Carbon keeps them mounted otherwise).
  if (!open) {
    return null;
  }
  return (
    <Modal
      open={open}
      modalHeading={title}
      primaryButtonText={t("actions.save")}
      secondaryButtonText={t("actions.cancel")}
      primaryButtonDisabled={!valid}
      onRequestClose={onClose}
      onRequestSubmit={() => onSubmit(name.trim())}
    >
      <TextInput
        id="group-name"
        labelText={t("common.name")}
        value={name}
        onChange={(event) => setName(event.target.value)}
      />
    </Modal>
  );
}

/** Bind-by-id form; no library listing endpoint exists, hence helper text. */
function LibraryBindForm({ onBind }: { onBind: (libraryId: string) => void }) {
  const { t } = useTranslation();
  const [libraryId, setLibraryId] = useState("");
  const valid = libraryId.trim().length > 0;
  return (
    <div>
      <TextInput
        id="group-bind-library"
        labelText={t("users.groups.bindLabel")}
        helperText={t("users.groups.bindHelp")}
        value={libraryId}
        onChange={(event) => setLibraryId(event.target.value)}
      />
      <Button
        kind="secondary"
        size="sm"
        disabled={!valid}
        onClick={() => {
          onBind(libraryId.trim());
          setLibraryId("");
        }}
      >
        {t("users.groups.bind")}
      </Button>
    </div>
  );
}
