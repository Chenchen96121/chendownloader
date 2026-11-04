import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";
import type { DestinationApproval } from "../../shared/contracts/download";
import { Icon } from "../../shared/ui/Icon";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import {
  ClipboardIpcError,
  readClipboardText,
  type ClipboardErrorCode,
} from "../../shared/ipc/clipboard";
import { approveDefaultDestination } from "../../shared/ipc/destinationSettings";
import { TikTokDownloadPanel } from "./TikTokDownloadPanel";
import { TikTokMediaGallery } from "./TikTokMediaGallery";
import {
  isTikTokWorkspaceLocked,
  type TikTokWorkspaceState,
} from "./tiktokState";
import { useTikTokWorkspace } from "./useTikTokWorkspace";
import {useTikTokComponent} from "../settings/TikTokComponentStatus";

export function pendingTikTokPreviewIds(state: TikTokWorkspaceState, active: boolean): string[] {
  if (!active || state.analysis === null) return [];
  if(Object.values(state.previews).some(preview=>preview.status==="loading"))return [];
  const first=state.analysis.items[0];
  return first?.previewAvailable&&state.previews[first.itemId]===undefined?[first.itemId]:[];
}

export function TikTokWorkspace({
  locale,
  destinationRevision = 0,
  active = true,
  onOpenSettings,
}: {
  locale: Locale;
  destinationRevision?: number;
  active?: boolean;
  onOpenSettings: () => void;
}) {
  const workspace = useTikTokWorkspace();
  const component=useTikTokComponent(active);
  const [destination, setDestination] = useState<DestinationApproval | null>(null);
  const [pastePending, setPastePending] = useState(false);
  const [pasteError, setPasteError] = useState<ClipboardErrorCode | null>(null);
  const pasteGeneration = useRef(0);
  const mounted = useRef(true);
  const activeRef = useRef(active);
  const analysisId = workspace.state.analysis?.analysisId;
  const locked = isTikTokWorkspaceLocked(workspace.state);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      pasteGeneration.current += 1;
    };
  }, []);

  useEffect(() => {
    activeRef.current = active;
    if (!active) { pasteGeneration.current += 1; setPastePending(false); }
  }, [active]);


  useEffect(() => {
    if (!active || analysisId === undefined || locked) return;
    setDestination(null);
    let current = true;
    void approveDefaultDestination()
      .then((approved) => {
        if (current && workspace.state.analysis?.analysisId === analysisId) {
          setDestination(approved);
        }
      })
      .catch(() => {
        if (current) setDestination(null);
      });
    return () => { current = false; };
  }, [active, analysisId, destinationRevision, locked]);

  useEffect(() => {
    for (const itemId of pendingTikTokPreviewIds(workspace.state, active)) void workspace.loadPreview(itemId);
  }, [active, analysisId, workspace.loadPreview, workspace.state.analysis, workspace.state.previews]);

  function changeUrl(url: string) {
    pasteGeneration.current += 1;
    setPastePending(false);
    setPasteError(null);
    workspace.changeUrl(url);
  }

  async function pasteLink() {
    if (!active || locked || workspace.state.analysisStatus === "analyzing" || pastePending) return;
    const generation = ++pasteGeneration.current;
    setPastePending(true);
    setPasteError(null);
    try {
      const text = await readClipboardText();
      if (mounted.current && generation === pasteGeneration.current && activeRef.current && !locked) {
        workspace.changeUrl(text);
      }
    } catch (error) {
      if (mounted.current && generation === pasteGeneration.current) {
        setPasteError(error instanceof ClipboardIpcError ? error.code : "unavailable");
      }
    } finally {
      if (mounted.current && generation === pasteGeneration.current) {
        setPastePending(false);
      }
    }
  }

  function analyze(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    pasteGeneration.current += 1;
    setPastePending(false);
    setPasteError(null);
    void workspace.analyze();
  }

  return <TikTokWorkspaceView
    runtimeReady={component?.state==="ready"}
    runtimeError={component!==null&&component.state!=="ready"?(component.state==="missing"?catalogs[locale]["analysisError.tool_missing"]:catalogs[locale]["analysisError.tool_integrity_failed"]):undefined}
    locale={locale}
    state={workspace.state}
    destination={destination}
    eventsReady={workspace.eventsReady}
    active={active}
    pastePending={pastePending}
    pasteError={pasteError}
    onUrlChange={changeUrl}
    onPaste={() => { void pasteLink(); }}
    onAnalyze={analyze}
    onToggleSelection={workspace.toggleSelection}
    onSelectAll={workspace.selectAll}
    onClearSelection={workspace.clearSelection}
    onCompatibilityChange={workspace.setCompatibilityMode}
    onRequestPreview={(itemId) => { void workspace.loadPreview(itemId); }}
    onOpenSource={() => { void workspace.openSource(); }}
    onOpenSettings={onOpenSettings}
    onStart={() => {
      if (destination) void workspace.startDownload(destination.destinationId);
    }}
    onCancel={() => { void workspace.cancelDownload(); }}
    onOpenLocation={() => { void workspace.openCompletedLocation(); }}
  />;
}

export function TikTokWorkspaceView({
  runtimeReady=true,
  runtimeError,
  locale,
  state,
  destination,
  eventsReady,
  active,
  pastePending,
  pasteError,
  onUrlChange,
  onPaste,
  onAnalyze,
  onToggleSelection,
  onSelectAll,
  onClearSelection,
  onCompatibilityChange,
  onRequestPreview,
  onOpenSource,
  onOpenSettings,
  onStart,
  onCancel,
  onOpenLocation,
}: {
  runtimeReady?:boolean;
  runtimeError?:string|undefined;
  locale: Locale;
  state: TikTokWorkspaceState;
  destination: DestinationApproval | null;
  eventsReady: boolean;
  active: boolean;
  pastePending: boolean;
  pasteError: ClipboardErrorCode | null;
  onUrlChange: (url: string) => void;
  onPaste: () => void;
  onAnalyze: (event: FormEvent<HTMLFormElement>) => void;
  onToggleSelection: (itemId: string) => void;
  onSelectAll: () => void;
  onClearSelection: () => void;
  onCompatibilityChange: (enabled: boolean) => void;
  onRequestPreview: (itemId: string) => void;
  onOpenSource: () => void;
  onOpenSettings: () => void;
  onStart: () => void;
  onCancel: () => void;
  onOpenLocation: () => void;
}) {
  const copy = catalogs[locale];
  const locked = isTikTokWorkspaceLocked(state);
  const analyzing = state.analysisStatus === "analyzing";
  const inputLocked = locked || analyzing;
  const analysisError = state.error?.scope === "analysis"
    ? copy[`analysisError.${state.error.code}`]
    : null;

  return <section className="x-workspace" aria-labelledby="tiktok-workspace-heading">
    <header className="x-workspace-heading">
      <h2 id="tiktok-workspace-heading" className="page-title">{copy["route.tiktok"]}</h2>
    </header>
    <form className="x-url-form" onSubmit={onAnalyze}>
      <div className="field">
        <label htmlFor="tiktok-url">{copy.tiktokUrlLabel}</label>
        <div className="url-input-wrap">
          <input
            id="tiktok-url"
            type="text"
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            maxLength={8192}
            value={state.url}
            placeholder={copy.tiktokUrlPlaceholder}
            disabled={inputLocked}
            aria-invalid={state.error?.scope === "analysis"}
            onChange={(event) => onUrlChange(event.target.value)}
          />
          <button
            type="button"
            className="paste-link-button"
            disabled={!active || inputLocked || pastePending}
            aria-busy={pastePending}
            onClick={onPaste}
          ><Icon name="clipboard" size={16} />{copy.pasteLink}</button>
        </div>
        {pasteError ? <p className="field-hint error-message" role="status">{copy[`pasteError.${pasteError}`]}</p> : null}
        {runtimeError?<p className="field-hint error-message" role="alert">{runtimeError} <button type="button" onClick={onOpenSettings}>{copy.settingsTitle}</button></p>:null}
        {analysisError ? <p className="field-hint error-message" role="alert">{analysisError}</p> : null}
      </div>
      <button type="submit" className="analyze-button" disabled={!runtimeReady || inputLocked || state.url.trim() === ""} aria-busy={analyzing}>
        {analyzing ? copy.analysisLoading : copy.tiktokAnalyze}
      </button>
    </form>

    <div className="x-layout">
      <div className="x-gallery-pane" aria-busy={analyzing}>
        {state.analysis
          ? <TikTokMediaGallery
              locale={locale}
              analysis={state.analysis}
              selectedItemIds={state.selectedItemIds}
              previews={state.previews}
              locked={locked}
              onToggle={onToggleSelection}
              onRequestPreview={onRequestPreview}
              onOpenSource={onOpenSource}
            />
          : <div className="x-empty">
              <Icon name="tiktok" size={32} />
              <p>{analyzing ? copy.analysisLoading : copy.tiktokEmpty}</p>
            </div>}
      </div>
      <TikTokDownloadPanel
        locale={locale}
        state={state}
        destination={destination}
        eventsReady={eventsReady}
        onSelectAll={onSelectAll}
        onClearSelection={onClearSelection}
        onCompatibilityChange={onCompatibilityChange}
        onOpenSettings={onOpenSettings}
        onStart={onStart}
        onCancel={onCancel}
        onOpenLocation={onOpenLocation}
      />
    </div>
  </section>;
}
