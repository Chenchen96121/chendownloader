import { useEffect, useRef, useState } from "react";
import { getDownloadQueue, setDownloadConcurrency } from "../../shared/ipc/downloadQueue";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
export function DownloadConcurrencySettings({ locale }: { locale: Locale }) {
  const [limit, setLimit] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const saving = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { let disposed = false; mounted.current = true; void getDownloadQueue().then(snapshot => { if (!disposed) setLimit(snapshot.limit); }).catch(() => { if (!disposed) setFailed(true); }); return () => { disposed = true; mounted.current = false; }; }, []);
  async function save(value: number) {
    if (saving.current) return;
    saving.current = true; setBusy(true); setFailed(false);
    try { const snapshot = await setDownloadConcurrency(value); if (mounted.current) setLimit(snapshot.limit); }
    catch { if (mounted.current) setFailed(true); }
    finally { saving.current = false; if (mounted.current) setBusy(false); }
  }
  const copy = catalogs[locale];
  return <section className="download-concurrency-setting" aria-labelledby="download-concurrency-title"><h3 id="download-concurrency-title">{copy.concurrencyTitle}</h3>
    <label htmlFor="download-concurrency">{copy.concurrencyLabel}</label>
    <select id="download-concurrency" value={limit ?? ""} disabled={busy || limit === null} onChange={event => void save(Number(event.target.value))}>
      {limit === null ? <option value="">{copy.concurrencyLoading}</option> : null}
      {[1, 2, 3, 4].map(value => <option key={value} value={value}>{value}</option>)}
    </select><p className="field-hint">{copy.concurrencyHint}</p>
    {failed ? <p role="alert" className="error-message">{copy.concurrencyFailed}</p> : null}
  </section>;
}
