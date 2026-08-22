import { Loading } from "@carbon/react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Navigate, useLocation } from "react-router-dom";

import { ErrorState } from "../components/ErrorState";
import { useAuth } from "./AuthContext";

/**
 * Route guard (ADR-0139). UX only — the server enforces authorization on
 * every request; this keeps anonymous users on the login screen and shows
 * authenticated non-administrators an explicit access-denied state.
 */
export function RequireAuth({ children }: { children: ReactNode }) {
  const { session } = useAuth();
  const location = useLocation();
  const { t } = useTranslation();

  if (session.kind === "loading") {
    return <Loading description={t("common.loading")} withOverlay={false} />;
  }
  if (session.kind === "anonymous") {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }
  if (session.kind === "forbidden") {
    // Distinct from not-found (PRD-WEB-002): signed in, not an administrator.
    return <ErrorState kind="forbidden" />;
  }
  return <>{children}</>;
}
