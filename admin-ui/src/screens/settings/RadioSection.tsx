import { Add } from "@carbon/icons-react";
import {
  ActionableNotification,
  Button,
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
  createRadioStation,
  deleteRadioStation,
  listRadioStations,
  updateRadioStation,
} from "../../api/admin";
import { ApiError } from "../../api/client";
import type { RadioStation } from "../../api/types";
import { EmptyState } from "../../components/EmptyState";
import { formatDateTime, formatRelativeTime } from "../../i18n/format";

/**
 * Internet radio stations (PRD-RADIO-001/002): CRUD over the single
 * authoritative radio store — stations created here appear on the
 * OpenSubsonic `getInternetRadioStations` surface and vice versa. Station
 * public ids are immutable once issued and are never regenerated.
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

function radioErrorText(
  error: ApiError,
  t: ReturnType<typeof useTranslation>["t"],
): string {
  if (error.status === 400 && error.code === "invalid_stream_url") {
    return t("settings.radio.error.invalidStreamUrl");
  }
  if (error.status === 400 && error.code === "invalid_home_page_url") {
    return t("settings.radio.error.invalidHomePageUrl");
  }
  if (error.status === 400 && error.code === "invalid_name") {
    return t("settings.radio.error.invalidName");
  }
  return t("settings.radio.error.mutation", { message: error.message });
}

export function RadioSection() {
  const { t } = useTranslation();
  const [stations, setStations] = useState<RadioStation[] | null>(null);
  const [listError, setListError] = useState<ApiError | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);

  const [createOpen, setCreateOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<RadioStation | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<RadioStation | null>(null);

  const fetchStations = useCallback((signal: { cancelled: boolean }) => {
    return listRadioStations({ limit: 500 })
      .then((result) => {
        if (!signal.cancelled) {
          setStations(result);
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
    void fetchStations(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchStations]);

  const mutationError = (cause: unknown) =>
    setNotice({ kind: "error", text: radioErrorText(toApiError(cause), t) });

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
          title={t("settings.radio.loadErrorTitle")}
          subtitle={listError.message}
          hideCloseButton
          actionButtonLabel={t("actions.retry")}
          onActionButtonClick={() => void fetchStations({ cancelled: false })}
        />
      ) : null}
      {stations === null && !listError ? (
        <Loading description={t("common.loading")} withOverlay={false} />
      ) : null}
      {stations !== null ? (
        <TableContainer
          title={t("settings.radio.tableCaption")}
          description={t("settings.radio.description")}
        >
          <Table aria-label={t("settings.radio.tableCaption")}>
            <TableHead>
              <TableRow>
                <TableHeader>{t("common.name")}</TableHeader>
                <TableHeader>
                  {t("settings.radio.column.streamUrl")}
                </TableHeader>
                <TableHeader>{t("settings.radio.column.homePage")}</TableHeader>
                <TableHeader>{t("common.created")}</TableHeader>
                <TableHeader>{t("users.column.actions")}</TableHeader>
              </TableRow>
            </TableHead>
            <TableBody>
              {stations.map((station) => (
                <TableRow key={station.id}>
                  <TableCell>{station.name}</TableCell>
                  <TableCell>
                    <code>{station.streamUrl}</code>
                  </TableCell>
                  <TableCell>
                    {station.homePageUrl ?? t("common.none")}
                  </TableCell>
                  <TableCell>
                    <span title={formatDateTime(station.createdAt)}>
                      {formatRelativeTime(station.createdAt)}
                    </span>
                  </TableCell>
                  <TableCell>
                    <Button
                      kind="ghost"
                      size="sm"
                      onClick={() => setEditTarget(station)}
                    >
                      {t("actions.edit")}
                    </Button>
                    <Button
                      kind="danger--ghost"
                      size="sm"
                      onClick={() => setDeleteTarget(station)}
                    >
                      {t("actions.delete")}
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          {stations.length === 0 ? (
            <EmptyState
              compact
              title={t("settings.radio.emptyTitle")}
              description={t("settings.radio.emptyDescription")}
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
          {t("settings.radio.create")}
        </Button>
      </div>

      <StationModal
        open={createOpen}
        station={null}
        onClose={() => setCreateOpen(false)}
        onSaved={() => {
          setCreateOpen(false);
          setNotice({
            kind: "success",
            text: t("settings.radio.createSuccess"),
          });
          void fetchStations({ cancelled: false });
        }}
        onError={mutationError}
      />
      <StationModal
        key={editTarget?.id ?? "closed"}
        open={editTarget !== null}
        station={editTarget}
        onClose={() => setEditTarget(null)}
        onSaved={() => {
          setEditTarget(null);
          setNotice({
            kind: "success",
            text: t("settings.radio.updateSuccess"),
          });
          void fetchStations({ cancelled: false });
        }}
        onError={mutationError}
      />
      {deleteTarget !== null ? (
        <Modal
          danger
          open
          modalHeading={t("settings.radio.deleteTitle", {
            name: deleteTarget.name,
          })}
          primaryButtonText={t("actions.delete")}
          secondaryButtonText={t("actions.cancel")}
          onRequestClose={() => setDeleteTarget(null)}
          onRequestSubmit={() => {
            const target = deleteTarget;
            setDeleteTarget(null);
            deleteRadioStation(target.id)
              .then(() => {
                setNotice({
                  kind: "success",
                  text: t("settings.radio.deleteSuccess"),
                });
                void fetchStations({ cancelled: false });
              })
              .catch(mutationError);
          }}
        >
          <p>{t("settings.radio.deleteBody")}</p>
        </Modal>
      ) : null}
    </div>
  );
}

/** Create/edit modal; an empty home page URL clears the stored value. */
function StationModal({
  open,
  station,
  onClose,
  onSaved,
  onError,
}: {
  open: boolean;
  station: RadioStation | null;
  onClose: () => void;
  onSaved: () => void;
  onError: (cause: unknown) => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(station?.name ?? "");
  const [streamUrl, setStreamUrl] = useState(station?.streamUrl ?? "");
  const [homePageUrl, setHomePageUrl] = useState(station?.homePageUrl ?? "");
  const [busy, setBusy] = useState(false);

  const valid = name.trim().length > 0 && streamUrl.trim().length > 0 && !busy;

  // Closed modals are not rendered (Carbon keeps them mounted otherwise).
  if (!open) {
    return null;
  }

  const submit = () => {
    if (!valid) {
      return;
    }
    setBusy(true);
    const request = station
      ? updateRadioStation(station.id, {
          name: name.trim(),
          streamUrl: streamUrl.trim(),
          homePageUrl: homePageUrl.trim() === "" ? null : homePageUrl.trim(),
        })
      : createRadioStation({
          name: name.trim(),
          streamUrl: streamUrl.trim(),
          ...(homePageUrl.trim() === ""
            ? {}
            : { homePageUrl: homePageUrl.trim() }),
        });
    request
      .then(onSaved)
      .catch(onError)
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={open}
      modalHeading={
        station
          ? t("settings.radio.editTitle", { name: station.name })
          : t("settings.radio.createTitle")
      }
      primaryButtonText={
        station ? t("actions.saveChanges") : t("actions.create")
      }
      secondaryButtonText={t("actions.cancel")}
      primaryButtonDisabled={!valid}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      <TextInput
        id="station-name"
        labelText={t("common.name")}
        value={name}
        onChange={(event) => setName(event.target.value)}
      />
      <TextInput
        id="station-stream-url"
        labelText={t("settings.radio.column.streamUrl")}
        value={streamUrl}
        onChange={(event) => setStreamUrl(event.target.value)}
      />
      <TextInput
        id="station-home-page"
        labelText={t("settings.radio.column.homePage")}
        helperText={t("settings.radio.homePageHelp")}
        value={homePageUrl}
        onChange={(event) => setHomePageUrl(event.target.value)}
      />
    </Modal>
  );
}
