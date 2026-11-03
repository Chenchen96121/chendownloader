import { useEffect, useRef, useState } from "react";
import { LiveAnalysisWorkspace } from "../features/analyze/LiveAnalysisWorkspace";
import { InstagramWorkspace } from "../features/instagram/InstagramWorkspace";
import { XWorkspace } from "../features/x/XWorkspace";
import { BilibiliWorkspace } from "../features/bilibili/BilibiliWorkspace";
import { DefaultDestinationSettings } from "../features/settings/DefaultDestinationSettings";
import { DesktopShell, type DesktopRoute } from "./DesktopShell";
import { LocalComponentsSettings } from "../features/settings/LocalComponentsSettings";
import { LocaleSettings } from "../features/settings/LocaleSettings";
import { DownloadConcurrencySettings } from "../features/settings/DownloadConcurrencySettings";
import { DownloadQueuePanel } from "../features/downloads/DownloadQueuePanel";
import {
  beginProvisioningRead,
  beginRepair,
  createProvisioningState,
  failRepair,
  MAX_PROVISIONING_READS,
  receiveProvisioningStatus,
  shouldShowWorkspace,
} from "../features/settings/provisioningState";
import { StartupGate } from "../features/settings/StartupGate";
import type { ToolInventory } from "../shared/contracts/managedTools";
import { isToolProvisioningErrorCode } from "../shared/contracts/toolProvisioning";
import { catalogs } from "../shared/i18n/catalog";
import { readLocale, writeLocale, type Locale } from "../shared/i18n/locale";
import { getToolInventory } from "../shared/ipc/managedTools";
import { getToolProvisioningStatus, repairManagedTools } from "../shared/ipc/toolProvisioning";

const PROVISIONING_POLL_MS = 500;

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

export function App() {
  const [locale, setLocale] = useState<Locale>(() => readLocale());
  const [route, setRoute] = useState<DesktopRoute>("youtube");
  const [destinationRevision, setDestinationRevision] = useState(0);
  const [provisioning, setProvisioning] = useState(createProvisioningState);
  const [inventory, setInventory] = useState<ToolInventory | null | undefined>(null);
  const repairInFlight = useRef(false);
  const copy = catalogs[locale];
  useEffect(() => {
    document.documentElement.lang = locale;
    document.title = catalogs[locale].productTitle;
  }, [locale]);

  useEffect(() => {
    let current = true;
    async function readUntilSettled() {
      for (let attempt = 0; current && attempt < MAX_PROVISIONING_READS; attempt += 1) {
        setProvisioning(beginProvisioningRead);
        let status;
        try {
          status = await getToolProvisioningStatus();
        } catch {
          if (current) {
            setProvisioning((state) => receiveProvisioningStatus(state, {
              state: "failed",
              errorCode: "activation_failed",
            }));
            setInventory(undefined);
          }
          return;
        }
        if (!current) return;
        setProvisioning((state) => receiveProvisioningStatus(state, status));
        if (status.state === "ready") {
          try {
            const nextInventory = await getToolInventory();
            if (current) setInventory(nextInventory);
          } catch {
            if (current) setInventory(undefined);
          }
          return;
        }
        if (status.state === "failed") return;
        await delay(PROVISIONING_POLL_MS);
      }
      if (current) {
        setProvisioning((state) => receiveProvisioningStatus(state, {
          state: "failed",
          errorCode: "activation_failed",
        }));
      }
    }
    void readUntilSettled();
    return () => { current = false; };
  }, []);

  function changeLocale(next: Locale) {
    setLocale(next);
    writeLocale(next);
  }

  async function repairComponents() {
    if (repairInFlight.current) return;
    if (
      provisioning.status === null
      || provisioning.status.state === "preparing"
      || provisioning.status.state === "repairing"
    ) return;
    repairInFlight.current = true;
    setProvisioning(beginRepair);
    try {
      const status = await repairManagedTools();
      setProvisioning((state) => receiveProvisioningStatus(state, status));
      if (status.state === "ready") {
        setInventory(await getToolInventory());
      }
    } catch (error) {
      const code = isToolProvisioningErrorCode(error) ? error : "activation_failed";
      try {
        const status = await getToolProvisioningStatus();
        setProvisioning((state) => failRepair(receiveProvisioningStatus(state, status), code));
        if (status.state === "ready") setInventory(await getToolInventory());
      } catch {
        setProvisioning((state) => failRepair(state, code));
        setInventory(undefined);
      }
    } finally {
      repairInFlight.current = false;
    }
  }

  return (
    <DesktopShell locale={locale} route={route} onNavigate={setRoute}
      downloads={<DownloadQueuePanel locale={locale} />}
      youtubeWorkspace={shouldShowWorkspace(provisioning)
        ? <LiveAnalysisWorkspace locale={locale} active={route === "youtube"} destinationRevision={destinationRevision} onOpenSettings={() => setRoute("settings")} />
        : <StartupGate status={provisioning.status} copy={copy} />}
      instagramWorkspace={shouldShowWorkspace(provisioning)
        ? <InstagramWorkspace locale={locale} active={route === "instagram"} destinationRevision={destinationRevision} onOpenSettings={() => setRoute("settings")} />
        : <StartupGate status={provisioning.status} copy={copy} />}
      xWorkspace={shouldShowWorkspace(provisioning)
        ? <XWorkspace locale={locale} active={route === "x"} destinationRevision={destinationRevision} onOpenSettings={() => setRoute("settings")} />
        : <StartupGate status={provisioning.status} copy={copy} />}
      bilibiliWorkspace={shouldShowWorkspace(provisioning)
        ? <BilibiliWorkspace locale={locale} active={route === "bilibili"} destinationRevision={destinationRevision} onOpenSettings={() => setRoute("settings")} />
        : <StartupGate status={provisioning.status} copy={copy} />}
      settings={<div className="app-settings" aria-label={copy.settingsTitle}>
        <LocaleSettings locale={locale} copy={copy} onLocaleChange={changeLocale} />
        <DefaultDestinationSettings locale={locale} onChanged={() => setDestinationRevision((value) => value + 1)} />
        <DownloadConcurrencySettings locale={locale} />
        <LocalComponentsSettings
          copy={copy}
          status={provisioning.status}
          inventory={inventory}
          repairPending={provisioning.repairPending}
          repairError={provisioning.repairError}
          onRepair={() => { void repairComponents(); }}
        />
      </div>} />
  );
}
