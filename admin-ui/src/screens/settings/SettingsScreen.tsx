import { Tab, TabList, TabPanel, TabPanels, Tabs } from "@carbon/react";
import { useTranslation } from "react-i18next";
import { useSearchParams } from "react-router-dom";

import { PageHeader } from "../../components/PageHeader";
import { ConfigSection } from "./ConfigSection";
import { DeviceProfilesSection } from "./DeviceProfilesSection";
import { NotificationsSection } from "./NotificationsSection";
import { RadioSection } from "./RadioSection";
import { ScriptsSection } from "./ScriptsSection";
import { ThemeSection } from "./ThemeSection";

/**
 * Settings (PRD-CONFIG-001/002, PRD-WEB-002): tabbed sub-pages over
 * the admin configuration surfaces — configuration browser with
 * export/import, notification templates, scripts, internet radio, and
 * device profiles. The active tab is URL state so views are shareable.
 */

const TABS = [
  "configuration",
  "notifications",
  "scripts",
  "radio",
  "deviceProfiles",
  "theme",
] as const;
type SettingsTab = (typeof TABS)[number];

function parseTab(raw: string | null): SettingsTab {
  return TABS.find((tab) => tab === raw) ?? "configuration";
}

export function SettingsScreen() {
  const { t } = useTranslation();
  const [searchParams, setSearchParams] = useSearchParams();
  const tab = parseTab(searchParams.get("tab"));

  return (
    <>
      <PageHeader
        title={t("settings.title")}
        subtitle={t("settings.subtitle")}
      />
      <Tabs
        selectedIndex={TABS.indexOf(tab)}
        onChange={({ selectedIndex }) => {
          const next = new URLSearchParams(searchParams);
          const nextTab = TABS[selectedIndex] ?? "configuration";
          if (nextTab === "configuration") {
            next.delete("tab");
          } else {
            next.set("tab", nextTab);
          }
          setSearchParams(next, { replace: true });
        }}
      >
        <TabList aria-label={t("settings.tabsLabel")}>
          <Tab>{t("settings.tabs.configuration")}</Tab>
          <Tab>{t("settings.tabs.notifications")}</Tab>
          <Tab>{t("settings.tabs.scripts")}</Tab>
          <Tab>{t("settings.tabs.radio")}</Tab>
          <Tab>{t("settings.tabs.deviceProfiles")}</Tab>
          <Tab>{t("settings.tabs.theme")}</Tab>
        </TabList>
        <TabPanels>
          <TabPanel>
            <ConfigSection />
          </TabPanel>
          <TabPanel>
            <NotificationsSection />
          </TabPanel>
          <TabPanel>
            <ScriptsSection />
          </TabPanel>
          <TabPanel>
            <RadioSection />
          </TabPanel>
          <TabPanel>
            <DeviceProfilesSection />
          </TabPanel>
          <TabPanel>
            <ThemeSection />
          </TabPanel>
        </TabPanels>
      </Tabs>
    </>
  );
}
