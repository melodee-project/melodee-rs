import {
  ActionableNotification,
  Button,
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
  TextArea,
  TextInput,
} from "@carbon/react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  deleteNotificationTemplate,
  listNotificationDeliveries,
  listNotificationTemplates,
  putNotificationTemplate,
  testSendNotification,
} from "../../api/admin";
import { ApiError } from "../../api/client";
import type {
  NotificationDelivery,
  NotificationTemplate,
} from "../../api/types";
import { EmptyState } from "../../components/EmptyState";
import { StatusTag } from "../../components/StatusTag";
import {
  formatDateTime,
  formatNumber,
  formatRelativeTime,
} from "../../i18n/format";

/**
 * Notification templates + test-send (PRD-NOTIF-001..003): template
 * list by templateKey + locale, full-replacement body editing (null clears
 * a field), test-send with the honestly recorded delivery result, and a
 * delivery audit view. The audit stores no message bodies (PRD-NOTIF-002),
 * so deliveries show status/error/recipient only. Deleting a seeded
 * template disables rendering for that event (deliveries record `skipped`)
 * — the delete modal states this instead of pretending data is erased.
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

function deliveryKind(status: string): "success" | "error" | "neutral" {
  if (status === "sent") {
    return "success";
  }
  if (status === "failed") {
    return "error";
  }
  return "neutral";
}

export function NotificationsSection() {
  const { t } = useTranslation();
  const [templates, setTemplates] = useState<NotificationTemplate[] | null>(
    null,
  );
  const [templatesError, setTemplatesError] = useState<ApiError | null>(null);
  const [deliveries, setDeliveries] = useState<NotificationDelivery[] | null>(
    null,
  );
  const [notice, setNotice] = useState<Notice | null>(null);

  const [editTarget, setEditTarget] = useState<NotificationTemplate | null>(
    null,
  );
  const [testTarget, setTestTarget] = useState<NotificationTemplate | null>(
    null,
  );
  const [deleteTarget, setDeleteTarget] = useState<NotificationTemplate | null>(
    null,
  );

  const fetchTemplates = useCallback((signal: { cancelled: boolean }) => {
    return listNotificationTemplates({ limit: 500 })
      .then((result) => {
        if (!signal.cancelled) {
          setTemplates(result);
          setTemplatesError(null);
        }
      })
      .catch((cause: unknown) => {
        if (!signal.cancelled) {
          setTemplatesError(toApiError(cause));
        }
      });
  }, []);

  const fetchDeliveries = useCallback((signal: { cancelled: boolean }) => {
    return listNotificationDeliveries({ limit: 100 })
      .then((result) => {
        if (!signal.cancelled) {
          setDeliveries(result);
        }
      })
      .catch(() => {
        // The audit list is supplementary; a failure leaves the empty state.
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchTemplates(signal);
    void fetchDeliveries(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchTemplates, fetchDeliveries]);

  const mutationError = (cause: unknown) =>
    setNotice({
      kind: "error",
      text: t("settings.notifications.error", {
        message: toApiError(cause).message,
      }),
    });

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
      {templatesError ? (
        <ActionableNotification
          inline
          kind="error"
          lowContrast
          title={t("settings.notifications.loadErrorTitle")}
          subtitle={templatesError.message}
          hideCloseButton
          actionButtonLabel={t("actions.retry")}
          onActionButtonClick={() => void fetchTemplates({ cancelled: false })}
        />
      ) : null}
      {templates === null && !templatesError ? (
        <Loading description={t("common.loading")} withOverlay={false} />
      ) : null}
      {templates !== null ? (
        <TableContainer title={t("settings.notifications.templatesCaption")}>
          <Table aria-label={t("settings.notifications.templatesCaption")}>
            <TableHead>
              <TableRow>
                <TableHeader>
                  {t("settings.notifications.column.template")}
                </TableHeader>
                <TableHeader>
                  {t("settings.notifications.column.locale")}
                </TableHeader>
                <TableHeader>
                  {t("settings.notifications.column.subject")}
                </TableHeader>
                <TableHeader>
                  {t("settings.notifications.column.version")}
                </TableHeader>
                <TableHeader>{t("common.updated")}</TableHeader>
                <TableHeader>{t("users.column.actions")}</TableHeader>
              </TableRow>
            </TableHead>
            <TableBody>
              {templates.map((template) => (
                <TableRow key={`${template.templateKey}/${template.locale}`}>
                  <TableCell>
                    <code>{template.templateKey}</code>
                  </TableCell>
                  <TableCell>{template.locale}</TableCell>
                  <TableCell>{template.subject ?? t("common.none")}</TableCell>
                  <TableCell>{formatNumber(template.version)}</TableCell>
                  <TableCell>
                    <span title={formatDateTime(template.updatedAt)}>
                      {formatRelativeTime(template.updatedAt)}
                    </span>
                  </TableCell>
                  <TableCell>
                    <Button
                      kind="ghost"
                      size="sm"
                      onClick={() => setEditTarget(template)}
                    >
                      {t("actions.edit")}
                    </Button>
                    <Button
                      kind="ghost"
                      size="sm"
                      onClick={() => setTestTarget(template)}
                    >
                      {t("settings.notifications.testSend")}
                    </Button>
                    <Button
                      kind="danger--ghost"
                      size="sm"
                      onClick={() => setDeleteTarget(template)}
                    >
                      {t("actions.delete")}
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
          {templates.length === 0 ? (
            <EmptyState
              compact
              title={t("settings.notifications.emptyTitle")}
              description={t("settings.notifications.emptyDescription")}
            />
          ) : null}
        </TableContainer>
      ) : null}

      <section
        className="melodee-panel"
        aria-labelledby="notification-deliveries-title"
      >
        <Heading
          id="notification-deliveries-title"
          className="melodee-panel__title"
        >
          {t("settings.notifications.deliveriesTitle")}
        </Heading>
        <p className="melodee-panel__subtitle">
          {t("settings.notifications.deliveriesNote")}
        </p>
        {deliveries === null || deliveries.length === 0 ? (
          <EmptyState
            compact
            title={t("settings.notifications.deliveriesEmpty")}
          />
        ) : (
          <TableContainer title={t("settings.notifications.deliveriesCaption")}>
            <Table
              size="sm"
              aria-label={t("settings.notifications.deliveriesCaption")}
            >
              <TableHead>
                <TableRow>
                  <TableHeader>
                    {t("settings.notifications.column.template")}
                  </TableHeader>
                  <TableHeader>
                    {t("settings.notifications.column.recipient")}
                  </TableHeader>
                  <TableHeader>{t("common.status")}</TableHeader>
                  <TableHeader>
                    {t("settings.notifications.column.error")}
                  </TableHeader>
                  <TableHeader>{t("common.created")}</TableHeader>
                </TableRow>
              </TableHead>
              <TableBody>
                {deliveries.map((delivery) => (
                  <TableRow key={delivery.id}>
                    <TableCell>
                      <code>{delivery.templateKey}</code>
                    </TableCell>
                    <TableCell>{delivery.recipient}</TableCell>
                    <TableCell>
                      <StatusTag
                        kind={deliveryKind(delivery.status)}
                        label={delivery.status}
                      />
                    </TableCell>
                    <TableCell>{delivery.error ?? t("common.none")}</TableCell>
                    <TableCell>
                      <span title={formatDateTime(delivery.createdAt)}>
                        {formatRelativeTime(delivery.createdAt)}
                      </span>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </TableContainer>
        )}
      </section>

      <TemplateEditModal
        key={
          editTarget
            ? `edit-${editTarget.templateKey}/${editTarget.locale}`
            : "edit-closed"
        }
        template={editTarget}
        onClose={() => setEditTarget(null)}
        onSaved={() => {
          setEditTarget(null);
          setNotice({
            kind: "success",
            text: t("settings.notifications.saveSuccess"),
          });
          void fetchTemplates({ cancelled: false });
        }}
        onError={mutationError}
      />
      <TestSendModal
        key={
          testTarget
            ? `test-${testTarget.templateKey}/${testTarget.locale}`
            : "test-closed"
        }
        template={testTarget}
        onClose={() => setTestTarget(null)}
        onSent={() => {
          void fetchDeliveries({ cancelled: false });
        }}
      />
      {deleteTarget !== null ? (
        <Modal
          danger
          open
          modalHeading={t("settings.notifications.deleteTitle", {
            template: deleteTarget.templateKey,
            locale: deleteTarget.locale,
          })}
          primaryButtonText={t("actions.delete")}
          secondaryButtonText={t("actions.cancel")}
          onRequestClose={() => setDeleteTarget(null)}
          onRequestSubmit={() => {
            const target = deleteTarget;
            setDeleteTarget(null);
            deleteNotificationTemplate(target.templateKey, target.locale)
              .then(() => {
                setNotice({
                  kind: "success",
                  text: t("settings.notifications.deleteSuccess"),
                });
                void fetchTemplates({ cancelled: false });
              })
              .catch(mutationError);
          }}
        >
          <p>{t("settings.notifications.deleteBody")}</p>
        </Modal>
      ) : null}
    </div>
  );
}

/** Full-replacement edit: empty fields are sent as null (server clears). */
function TemplateEditModal({
  template,
  onClose,
  onSaved,
  onError,
}: {
  template: NotificationTemplate | null;
  onClose: () => void;
  onSaved: () => void;
  onError: (cause: unknown) => void;
}) {
  const { t } = useTranslation();
  const [subject, setSubject] = useState(template?.subject ?? "");
  const [bodyText, setBodyText] = useState(template?.bodyText ?? "");
  const [bodyHtml, setBodyHtml] = useState(template?.bodyHtml ?? "");
  const [busy, setBusy] = useState(false);

  // Closed modals are not rendered (Carbon keeps them mounted otherwise).
  if (!template) {
    return null;
  }

  const submit = () => {
    if (busy) {
      return;
    }
    setBusy(true);
    putNotificationTemplate(template.templateKey, template.locale, {
      subject: subject.trim() === "" ? null : subject,
      bodyText: bodyText === "" ? null : bodyText,
      bodyHtml: bodyHtml === "" ? null : bodyHtml,
    })
      .then(onSaved)
      .catch(onError)
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={template !== null}
      size="lg"
      modalHeading={t("settings.notifications.editTitle", {
        template: template?.templateKey ?? "",
        locale: template?.locale ?? "",
      })}
      primaryButtonText={t("actions.saveChanges")}
      secondaryButtonText={t("actions.cancel")}
      primaryButtonDisabled={busy}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      <p>{t("settings.notifications.editNote")}</p>
      <TextInput
        id="template-subject"
        labelText={t("settings.notifications.column.subject")}
        value={subject}
        onChange={(event) => setSubject(event.target.value)}
      />
      <TextArea
        id="template-body-text"
        labelText={t("settings.notifications.bodyText")}
        value={bodyText}
        onChange={(event) => setBodyText(event.target.value)}
      />
      <TextArea
        id="template-body-html"
        labelText={t("settings.notifications.bodyHtml")}
        helperText={t("settings.notifications.bodyHtmlHelp")}
        value={bodyHtml}
        onChange={(event) => setBodyHtml(event.target.value)}
      />
    </Modal>
  );
}

/** Test-send with the honestly recorded delivery result (incl. skipped). */
function TestSendModal({
  template,
  onClose,
  onSent,
}: {
  template: NotificationTemplate | null;
  onClose: () => void;
  onSent: () => void;
}) {
  const { t } = useTranslation();
  const [recipient, setRecipient] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<NotificationDelivery | null>(null);

  const valid = recipient.includes("@");

  if (!template) {
    return null;
  }

  const submit = () => {
    if (!valid || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    setResult(null);
    testSendNotification({
      templateKey: template.templateKey,
      locale: template.locale,
      recipient: recipient.trim(),
    })
      .then((delivery) => {
        setResult(delivery);
        onSent();
      })
      .catch((cause: unknown) => {
        setError(
          t("settings.notifications.testError", {
            message: toApiError(cause).message,
          }),
        );
      })
      .finally(() => setBusy(false));
  };

  return (
    <Modal
      open={template !== null}
      modalHeading={t("settings.notifications.testTitle", {
        template: template?.templateKey ?? "",
        locale: template?.locale ?? "",
      })}
      primaryButtonText={t("settings.notifications.testSend")}
      secondaryButtonText={t("actions.close")}
      primaryButtonDisabled={!valid || busy}
      onRequestClose={onClose}
      onRequestSubmit={submit}
    >
      <p>{t("settings.notifications.testNote")}</p>
      {error ? (
        <InlineNotification
          kind="error"
          lowContrast
          title={error}
          hideCloseButton
        />
      ) : null}
      {result ? (
        <InlineNotification
          kind={
            result.status === "failed"
              ? "error"
              : result.status === "sent"
                ? "success"
                : "info"
          }
          lowContrast
          title={t(`settings.notifications.testResult.${result.status}`, {
            recipient: result.recipient,
          })}
          subtitle={result.error ?? undefined}
          hideCloseButton
        />
      ) : null}
      <TextInput
        id="test-send-recipient"
        type="email"
        labelText={t("settings.notifications.column.recipient")}
        value={recipient}
        invalid={recipient.length > 0 && !valid}
        invalidText={t("validation.emailInvalid")}
        onChange={(event) => setRecipient(event.target.value)}
      />
    </Modal>
  );
}
