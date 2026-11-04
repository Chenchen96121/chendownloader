import {TikTokTaskDetails} from "./TikTokTaskDetails";
import {TikTokDownloadIpcError} from "../../shared/ipc/tiktok";
import { useEffect, useRef, useState } from "react";
import { isQueueTerminal, type DownloadQueueSnapshot, type DownloadTask } from "../../shared/contracts/downloadQueue";
import type { DownloadErrorCode } from "../../shared/contracts/errors";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { getDownloadQueue } from "../../shared/ipc/downloadQueue";
import { DownloadCommandError } from "../../shared/ipc/downloads";
import { InstagramDownloadIpcError } from "../../shared/ipc/instagram";
import { XDownloadIpcError } from "../../shared/ipc/x";
import { BilibiliDownloadIpcError } from "../../shared/ipc/bilibili";
import { XTaskDetails } from "./XTaskDetails";
import { cancelQueueTask, openQueueTask } from "./queueActions";
import { InstagramTaskDetails } from "./InstagramTaskDetails";
import { Icon, type IconName } from "../../shared/ui/Icon";

function taskIcon(state: DownloadTask["state"]): IconName {
  if (state === "completed") return "check-circle";
  if (state === "failed" || state === "partial") return "warning";
  if (state === "canceled") return "close";
  if (state === "queued") return "history";
  return "loader";
}

export function DownloadQueuePanel({ locale }: { locale: Locale }) {
  const [snapshot, setSnapshot] = useState<DownloadQueueSnapshot | null>(null);
  const [failed, setFailed] = useState(false);
  const [error, setError] = useState<DownloadErrorCode | null>(null);
  const [pending, setPending] = useState<ReadonlySet<string>>(new Set());
  const [expanded, setExpanded] = useState(true);
  const pendingRef = useRef(new Set<string>());
  const alive = useRef(true);
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let last = "";
    alive.current = true;
    async function poll() {
      try {
        const next = await getDownloadQueue();
        if (disposed) return;
        const json = JSON.stringify(next);
        if (json !== last) { last = json; setSnapshot(next); }
        setFailed(false);
      } catch { if (!disposed) setFailed(true); }
      finally { if (!disposed) timer = setTimeout(() => void poll(), 750); }
    }
    void poll();
    return () => { disposed = true; alive.current = false; clearTimeout(timer); };
  }, []);
  async function act(task: DownloadTask, operation: typeof cancelQueueTask) {
    if (pendingRef.current.has(task.jobId)) return;
    pendingRef.current.add(task.jobId); setPending(new Set(pendingRef.current)); setError(null);
    try { await operation(task); }
    catch (reason) { if (alive.current) setError(reason instanceof DownloadCommandError || reason instanceof InstagramDownloadIpcError || reason instanceof TikTokDownloadIpcError || reason instanceof XDownloadIpcError || reason instanceof BilibiliDownloadIpcError ? reason.code : "unknown"); }
    finally { pendingRef.current.delete(task.jobId); if (alive.current) setPending(new Set(pendingRef.current)); }
  }
  return <DownloadQueuePanelView locale={locale} snapshot={snapshot} failed={failed} error={error} pending={pending} expanded={expanded} onToggle={() => setExpanded(value => !value)} onCancel={task => void act(task, cancelQueueTask)} onOpen={task => void act(task, openQueueTask)} />;
}

export function DownloadQueuePanelView({ locale, snapshot, failed, error, pending, expanded, onToggle, onCancel, onOpen }: {
  locale: Locale; snapshot: DownloadQueueSnapshot | null; failed: boolean; error: DownloadErrorCode | null;
  pending: ReadonlySet<string>; expanded: boolean; onToggle(): void; onCancel(task: DownloadTask): void; onOpen(task: DownloadTask): void;
}) {
  const copy = catalogs[locale];
  const active = snapshot?.tasks.filter(task => !isQueueTerminal(task.state)).length ?? 0;
  return <section className="download-queue" aria-labelledby="download-queue-title">
    <header className="download-queue-header"><h2 id="download-queue-title">{copy.queueTitle}<span>{copy.queuePending}: {active} · {copy.queueLimit}: {snapshot?.limit ?? "–"}</span></h2>
      <button type="button" aria-expanded={expanded} aria-controls="download-queue-list" onClick={onToggle}>{expanded ? copy.queueCollapse : copy.queueExpand}<Icon name="chevron-down" size={16} /></button></header>
    {failed ? <p role="alert" className="error-message">{copy.queueReadFailed}</p> : null}
    {error ? <p role="alert" className="error-message">{copy[`downloadError.${error}`]}</p> : null}
    <div id="download-queue-list" hidden={!expanded}>
      {!snapshot || snapshot.tasks.length === 0 ? <p className="queue-empty"><Icon name="history" size={32} /><span>{copy.queueEmpty}</span></p> : <ul className="queue-task-list">
        {[...snapshot.tasks].reverse().map(task => {
          const terminal = isQueueTerminal(task.state);
          return <li key={task.jobId} className="queue-task" data-state={task.state}>
            <span className="queue-task-icon"><Icon name={taskIcon(task.state)} /></span>
            <div className="queue-task-copy"><span className="queue-task-platform">{{tiktok:"TikTok",youtube:"YouTube",instagram:"Instagram",x:"X",bilibili:"Bilibili"}[task.platform]}</span><strong title={task.title}>{task.title}</strong>
              <span className="queue-task-state">{copy[`queueState.${task.state}`]}{task.percent === null ? "" : ` · ${Math.round(task.percent)}%`}</span>
              {task.errorCode ? <span className="error-message">{copy[`downloadError.${task.errorCode}`]}</span> : null}
              {!terminal ? <progress max={100} value={task.percent ?? undefined} aria-label={`${task.title}: ${copy[`queueState.${task.state}`]}`} /> : null}
              {terminal && task.platform === "instagram" ? <InstagramTaskDetails jobId={task.jobId} title={task.title} locale={locale} /> : null}
              {terminal && task.platform === "tiktok" ? <TikTokTaskDetails jobId={task.jobId} title={task.title} locale={locale} /> : null}
              {terminal && task.platform === "x" ? <XTaskDetails jobId={task.jobId} title={task.title} locale={locale} /> : null}
            </div>
            <div className="queue-task-actions">{!terminal ? <button type="button" className="queue-task-cancel" disabled={pending.has(task.jobId)} onClick={() => onCancel(task)}><Icon name="close" size={16} />{copy.downloadCancel}</button> : task.displayName ? <button type="button" className="queue-task-open" disabled={pending.has(task.jobId)} onClick={() => onOpen(task)}><Icon name="folder" size={16} />{copy.downloadOpenFolder}</button> : null}</div>
          </li>;
        })}
      </ul>}
    </div>
  </section>;
}
