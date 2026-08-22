import { Add, Checkmark } from "@carbon/icons-react";
import {
  ActionableNotification,
  Button,
  Checkbox,
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
  TextArea,
  TextInput,
} from "@carbon/react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  createDeviceProfile,
  deleteDeviceProfile,
  listDeviceProfiles,
  setDefaultDeviceProfile,
  updateDeviceProfile,
} from "../../api/admin";
import { ApiError } from "../../api/client";
import type { DeviceProfile } from "../../api/types";
import { EmptyState } from "../../components/EmptyState";
import { StatusTag } from "../../components/StatusTag";
import { formatDateTime, formatRelativeTime } from "../../i18n/format";

/**
 * Device profiles (PRD-DEVICE-001/002): CRUD plus set-default over
 * operator-managed metadata. PRD-DEVICE-002 honesty rule: nothing on this
 * surface feeds stream negotiation — profile-driven stream selection is
 * deliberately unwired post-parity work (PRD-DEVICE-003) — so the section
 * states the metadata-only contract and never claims streaming effects.
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

function profileErrorText(
  error: ApiError,
  t: ReturnType<typeof useTranslation>["t"],
): string {
  if (error.status === 400 && error.code === "invalid_profile") {
    return t("settings.deviceProfiles.error.invalidProfile", {
      message: error.message,
    });
  }
  return t("settings.deviceProfiles.error.mutation", {
    message: error.message,
  });
}

export function DeviceProfilesSection() {
  const { t } = useTranslation();
  const [profiles, setProfiles] = useState<DeviceProfile[] | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<DeviceProfile | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<DeviceProfile | null>(null);

  const fetchProfiles = useCallback((signal: { cancelled: boolean }) => {
    return listDeviceProfiles({ limit: 500 })
      .then((result) => {
        if (!signal.cancelled) {
          setProfiles(result);
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
    void fetchProfiles(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchProfiles]);

  const mutationError = (cause: unknown) =>
    setNotice({ kind: "error", text: profileErrorText(toApiError(cause), t) });

  const setDefault = (profile: DeviceProfile) => {
    setBusyId(profile.id);
    setDefaultDeviceProfile(profile.id)
      .then(() => {
        setNotice({
          kind: "success",
          text: t("settings.deviceProfiles.defaultSuccess"),
        });
        void fetchProfiles({ cancelled: false });
      })
      .catch(mutationError)
      .finally(() => setBusyId(null));
  };

  return (
    <div>
      <InlineNotification
        kind="info"
        lowContrast
        title={t("settings.deviceProfiles.metadataTitle")}
        subtitle={t("settings.deviceProfiles.metadataNote")}
        hideCloseButton
      />
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
          title={t("settings.deviceProfiles.loadErrorTitle")}
          subtitle={listError.message}
          hideCloseButton
          actionButtonLabel={t("actions.retry")}
          onActionButtonClick={() => void fetchProfiles({ cancelled: false })}
        />
      ) : null}
      {profiles === null && !listError ? (
        <Loading description={t("common.loading")} withOverlay={false} />
      ) : null}
      {profiles !== null ? (
        <TableContainer title={t("settings.deviceProfiles.tableCaption")}>
          <Table aria-label={t("settings.deviceProfiles.tableCaption")}>
            <TableHead>
              <TableRow>
                <TableHeader>{t("common.name")}</TableHeader>
                <TableHeader>
                  {t("settings.deviceProfiles.column.default")}
                </TableHeader>
                <TableHeader>
                  {t("settings.deviceProfiles.column.clientMatch")}
                </TableHeader>
                <TableHeader>
                  {t("settings.deviceProfiles.column.profile")}
                </TableHeader>
                <TableHeader>{t("common.updated")}</TableHeader>
                <TableHeader>{t("users.column.actions")}</TableHeader>
              </TableRow>
            </TableHead>
            <TableBody>
              {profiles.map((profile) => (
                <TableRow key={profile.id}>
                  <TableCell>{profile.name}</TableCell>
                  <TableCell>
                    {profile.isDefault ? (
                      <StatusTag
                        kind="success"
                        label={t("settings.deviceProfiles.isDefault")}
                      />
                    ) : (
                      t("common.no")
                    )}
                  </TableCell>
                  <TableCell>
                    <code>{JSON.stringify(profile.clientMatch)}</code>
                  </TableCell>
                  <TableCell>
                    <code>{JSON.stringify(profile.profile)}</code>
                  </TableCell>
                  <TableCell>
                    <span title={formatDateTime(profile.updatedAt)}>
                      {formatRelativeTime(profile.updatedAt)}
                    </span>
                  </TableCell>
                  <TableCell>
                    <Button
                      kind="ghost"
                      size="sm"
                      onClick={() => setEditTarget(profile)}
                    >
                      {t("actions.edit")}
                    </Button>
                    <Button
                      kind="ghost"
                      size="sm"
                      renderIcon={Checkmark}
                      disabled={profile.isDefault || busyId === profile.id}
                      onClick={() => setDefault(profile)}
                    >
                      {t("settings.deviceProfiles.setDefault")}
                    </Button>
                    <Button
                      kind="danger--ghost"
                      size="sm"
                      onClick={() => setDeleteTarget(profile)}
                    >
                      {t("actions.delete")}
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          {profiles.length === 0 ? (
            <EmptyState
              compact
              title={t("settings.deviceProfiles.emptyTitle")}
              description={t("settings.deviceProfiles.emptyDescription")}
            />
          ) : null}
        </TableContainer>
      ) : null}
      <div style={{ marginBlockStart: "1rem" }}>
        <Button
          kind="primary"
          size="sm"
          renderIcon={Add}
          onClick={() => setCreateOpen(true)}
        >
          {t("settings.deviceProfiles.create")}
        </Button>
      </div>

      <ProfileModal
        open={createOpen}
        profile={null}
        onClose={() => setCreateOpen(false)}
        onSaved={() => {
          setCreateOpen(false);
          setNotice({
            kind: "success",
            text: t("settings.deviceProfiles.createSuccess"),
          });
          void fetchProfiles({ cancelled: false });
        }}
        onError={mutationError}
      />
      <ProfileModal
        key={editTarget?.id ?? "closed"}
        open={editTarget !== null}
        profile={editTarget}
        onClose={() => setEditTarget(null)}
        onSaved={() => {
          setEditTarget(null);
          setNotice({
            kind: "success",
            text: t("settings.deviceProfiles.updateSuccess"),
          });
          void fetchProfiles({ cancelled: false });
        }}
        onError={mutationError}
      />
      {deleteTarget !== null ? (
        <Modal
          danger
          open
          modalHeading={t("settings.deviceProfiles.deleteTitle", {
            name: deleteTarget.name,
          })}
          primaryButtonText={t("actions.delete")}
          secondaryButtonText={t("actions.cancel")}
          onRequestClose={() => setDeleteTarget(null)}
          onRequestSubmit={() => {
            const target = deleteTarget;
            setDeleteTarget(null);
            deleteDeviceProfile(target.id)
              .then(() => {
                setNotice({
                  kind: "success",
                  text: t("settings.deviceProfiles.deleteSuccess"),
                });
                void fetchProfiles({ cancelled: false });
              })
              .catch(mutationError);
          }}
        >
          <p>{t("settings.deviceProfiles.deleteBody")}</p>
        </Modal>
      ) : null}
    </div>
  );
}

/** Create/edit modal; JSON fields must parse to objects before submit. */
function ProfileModal({
  open,
  profile,
  onClose,
  onSaved,
  onError,
}: {
  open: boolean;
  profile: DeviceProfile | null;
  onClose: () => void;
  onSaved: () => void;
  onError: (cause: unknown) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(profile?.name ?? "");
  const [clientMatchText, setClientMatchText] = useState(
    profile ? JSON.stringify(profile.clientMatch, null, 2) : "{}",
  );
  const [profileText, setProfileText] = useState(
    profile ? JSON.stringify(profile.profile, null, 2) : "{}",
  );
  const [isDefault, setIsDefault] = useState(false);
  const [busy, setBusy] = useState(false);

  const parseObject = (text: string): Record<string, unknown> | null => {
    try {
      const parsed: unknown = JSON.parse(text);
      if (
        typeof parsed === "object" &&
        parsed !== null &&
        !Array.isArray(parsed)
      ) {
        return parsed as Record<string, unknown>;
      }
    } catch {
      // Fall through: invalid JSON or a non-object.
    }
    return null;
  };

  const clientMatch = parseObject(clientMatchText);
  const profileBody = parseObject(profileText);
  const valid =
    name.trim().length > 0 &&
    clientMatch !== null &&
    profileBody !== null &&
    !busy;

  // Closed modals are not rendered (Carbon keeps them mounted otherwise).
  if (!open) {
    return null;
  }

  const submit = () => {
    if (!valid || clientMatch === null || profileBody === null) {
      return;
    }
    setBusy(true);
    const request = profile
      ? updateDeviceProfile(profile.id, {
          name: name.trim(),
          clientMatch: clientMatch as Record<string, string>,
          profile: profileBody,
        })
      : createDeviceProfile({
          name: name.trim(),
          clientMatch: clientMatch as Record<string, string>,
          profile: profileBody,
          isDefault,
        });
    request
      .then(onSaved)
      .catch(onError)
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={open}
      size="lg"
      modalHeading={
        profile
          ? t("settings.deviceProfiles.editTitle", { name: profile.name })
          : t("settings.deviceProfiles.createTitle")
      }
      primaryButtonText={
        profile ? t("actions.saveChanges") : t("actions.create")
      }
      secondaryButtonText={t("actions.cancel")}
      primaryButtonDisabled={!valid}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      <p>{t("settings.deviceProfiles.metadataNote")}</p>
      <TextInput
        id="profile-name"
        labelText={t("common.name")}
        value={name}
        onChange={(event) => setName(event.target.value)}
      />
      <TextArea
        id="profile-client-match"
        labelText={t("settings.deviceProfiles.column.clientMatch")}
        helperText={t("settings.deviceProfiles.clientMatchHelp")}
        value={clientMatchText}
        invalid={clientMatch === null}
        invalidText={t("settings.deviceProfiles.jsonInvalid")}
        onChange={(event) => setClientMatchText(event.target.value)}
      />
      <TextArea
        id="profile-body"
        labelText={t("settings.deviceProfiles.column.profile")}
        helperText={t("settings.deviceProfiles.profileHelp")}
        value={profileText}
        invalid={profileBody === null}
        invalidText={t("settings.deviceProfiles.jsonInvalid")}
        onChange={(event) => setProfileText(event.target.value)}
      />
      {!profile ? (
        <Checkbox
          id="profile-is-default"
          labelText={t("settings.deviceProfiles.isDefault")}
          checked={isDefault}
          onChange={(_event, { checked }) => setIsDefault(checked)}
        />
      ) : null}
    </Modal>
  );
}
