import {
  Heading,
  Link,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableHeader,
  TableRow,
} from "@carbon/react";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Link as RouterLink } from "react-router-dom";

import { listLeases } from "../../api/admin";
import type { Lease } from "../../api/types";
import { EmptyState } from "../../components/EmptyState";
import { ProcessingStateTag } from "../../components/ProcessingStateTag";
import { formatDateTime, formatRelativeTime } from "../../i18n/format";

/**
 * Active (unexpired) lease evidence table, shared by the System → Workers
 * screen and the System → Diagnostics screen. An active lease proves
 * PostgreSQL holds an unexpired lease; it does not prove the owner process
 * is currently alive. Lease evidence is supplementary: a fetch failure
 * leaves the honest empty state rather than an error banner.
 *
 * `refreshKey` lets a parent screen's refresh action re-run the fetch
 * without owning it.
 */
export function LeasesPanel({
  refreshKey = 0,
  headingId = "system-leases-title",
}: {
  refreshKey?: number;
  headingId?: string;
}) {
  const { t } = useTranslation();
  const [leases, setLeases] = useState<Lease[] | null>(null);

  const fetchLeases = useCallback((signal: { cancelled: boolean }) => {
    return listLeases({ limit: 100 })
      .then((result) => {
        if (!signal.cancelled) {
          setLeases(result);
        }
      })
      .catch(() => {
        // Lease evidence is supplementary; a failure leaves the empty state.
      });
  }, []);

  useEffect(() => {
    const signal = { cancelled: false };
    void fetchLeases(signal);
    return () => {
      signal.cancelled = true;
    };
  }, [fetchLeases, refreshKey]);

  return (
    <section className="melodee-panel" aria-labelledby={headingId}>
      <Heading id={headingId} className="melodee-panel__title">
        {t("system.leases.title")}
      </Heading>
      {leases === null || leases.length === 0 ? (
        <EmptyState compact title={t("system.leases.empty")} />
      ) : (
        <TableContainer title={t("system.leases.caption")}>
          <Table size="sm" aria-label={t("system.leases.caption")}>
            <TableHead>
              <TableRow>
                <TableHeader>{t("system.leases.column.workItem")}</TableHeader>
                <TableHeader>{t("system.leases.column.owner")}</TableHeader>
                <TableHeader>{t("system.leases.column.state")}</TableHeader>
                <TableHeader>{t("system.leases.column.acquired")}</TableHeader>
                <TableHeader>{t("system.leases.column.expires")}</TableHeader>
              </TableRow>
            </TableHead>
            <TableBody>
              {leases.map((lease) => (
                <TableRow key={lease.workItemId}>
                  <TableCell>
                    <Link
                      as={RouterLink}
                      to={`/processing?item=${encodeURIComponent(lease.workItemId)}`}
                    >
                      {lease.workItemId}
                    </Link>
                  </TableCell>
                  <TableCell>{lease.leaseOwner}</TableCell>
                  <TableCell>
                    <ProcessingStateTag state={lease.state} />
                  </TableCell>
                  <TableCell>
                    <span title={formatDateTime(lease.leaseAcquiredAt)}>
                      {formatRelativeTime(lease.leaseAcquiredAt)}
                    </span>
                  </TableCell>
                  <TableCell>
                    <span title={formatDateTime(lease.leaseExpiresAt)}>
                      {formatRelativeTime(lease.leaseExpiresAt)}
                    </span>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      )}
    </section>
  );
}
