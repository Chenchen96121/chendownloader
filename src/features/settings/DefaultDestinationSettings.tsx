import { useEffect, useRef, useState } from "react";
import { chooseDefaultDestination, loadDefaultDestination, type DefaultDestinationStatus } from "../../shared/ipc/destinationSettings";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
export function DefaultDestinationSettings({ locale, onChanged }: {locale: Locale; onChanged: () => void}) {
  const [status, setStatus] = useState<DefaultDestinationStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const choosing = useRef(false);
  useEffect(() => {
    let current = true;
    void loadDefaultDestination().then((value) => { if (current) setStatus(value); }).catch(() => { if (current) setStatus({ state: "unavailable" }); });
    return () => { current = false; };
  }, []);
  async function choose() {
    if (choosing.current) return;
    choosing.current = true; setBusy(true); setFailed(false);
    try { const selected = await chooseDefaultDestination(); if (selected) { setStatus(selected); onChanged(); } }
    catch { setFailed(true); }
    finally { choosing.current = false; setBusy(false); }
  }
  return <DefaultDestinationSettingsView locale={locale} status={status} busy={busy} failed={failed} onChoose={() => void choose()} />;
}
export function DefaultDestinationSettingsView({locale, status, busy, failed, onChoose}: {locale: Locale; status: DefaultDestinationStatus | null; busy: boolean; failed: boolean; onChoose: () => void}) {
  const copy = catalogs[locale];
  return <section className="default-destination-setting" aria-labelledby="default-destination-title">
    <h3 id="default-destination-title">{copy.defaultDestinationTitle}</h3>
    <div className="settings-folder-row"><span>{status === null ? copy.defaultDestinationLoading : status.state === "ready" ? status.displayName : status.state === "missing" ? copy.downloadNoDestination : copy.defaultDestinationUnavailable}</span>
      <button type="button" disabled={busy} onClick={onChoose}>{busy ? copy.downloadChoosingFolder : copy.downloadChooseFolder}</button></div>
    <p className="field-hint">{copy.defaultDestinationHint}</p>
    {failed ? <p role="alert" className="error-message">{copy.defaultDestinationUnavailable}</p> : null}
  </section>;
}
