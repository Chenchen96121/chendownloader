import { useEffect, useRef, useState } from "react";
import type { InstagramDownloadResult } from "../../shared/contracts/instagram";
import { getInstagramDownloadResult } from "../../shared/ipc/instagram";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";

function format(template: string, values: Record<string, number>) {
  return template.replace(/\{(\w+)\}/gu, (_, key: string) => String(values[key] ?? ""));
}
export function InstagramTaskDetails({ jobId, title, locale }: { jobId: string; title: string; locale: Locale }) {
  const [expanded, setExpanded] = useState(false);
  const [result, setResult] = useState<InstagramDownloadResult | null>(null);
  const [failed, setFailed] = useState(false);
  const [loading, setLoading] = useState(false);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  async function toggle() {
    if (expanded) { setExpanded(false); return; }
    setExpanded(true);
    if (result || inFlight.current) return;
    inFlight.current = true; setLoading(true); setFailed(false);
    try { const next = await getInstagramDownloadResult(jobId); if (mounted.current) setResult(next); }
    catch { if (mounted.current) setFailed(true); }
    finally { inFlight.current = false; if (mounted.current) setLoading(false); }
  }
  const copy = catalogs[locale];
  return <div className="queue-task-details">
    <button type="button" aria-label={`${copy.queueDetails}: ${title}`} aria-expanded={expanded} onClick={() => void toggle()}>{copy.queueDetails}</button>
    {expanded ? <div>{loading ? <p>{copy.queueResultLoading}</p> : result ? <InstagramTaskResultView result={result} locale={locale} /> : failed ? <p role="alert" className="error-message">{copy.queueResultFailed}</p> : null}</div> : null}
  </div>;
}
export function InstagramTaskResultView({ result, locale }: { result: InstagramDownloadResult; locale: Locale }) {
  const copy = catalogs[locale];
  return <div className="queue-item-results">
    <p>{format(copy.instagramPartialSummary, { completed: result.completedCount, failed: result.failedCount })}</p>
    <ul>{result.items.map(item => <li key={item.itemId}>
      <span>{format(copy.instagramItemLabel, { ordinal: item.ordinal })} · {item.kind === "image" ? copy.instagramImageLabel : copy.instagramVideoLabel}</span>
      <span>{copy[`queueState.${item.state}`]}{item.displayName ? ` · ${item.displayName}` : ""}</span>
      {item.errorCode ? <span className="error-message">{copy[`downloadError.${item.errorCode}`]}</span> : null}
    </li>)}</ul>
  </div>;
}
