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
import { XDownloadPanel } from "./XDownloadPanel";
import { XMediaGallery } from "./XMediaGallery";
import {
  isXWorkspaceLocked,
  type XWorkspaceState,
} from "./xState";
import { useXWorkspace } from "./useXWorkspace";

export function pendingXPreviewIds(state: XWorkspaceState, active: boolean): string[] {
  if (!active || state.analysis === null) return [];
  const slots = Math.max(0, 2 - Object.values(state.previews).filter(preview => preview.status === "loading").length);
  return state.analysis.items.filter(item => item.previewAvailable && state.previews[item.itemId] === undefined).slice(0, slots).map(item => item.itemId);
}

export function XWorkspace({
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
  const workspace = useXWorkspace();
  const [destination, setDestination] = useState<DestinationApproval | null>(null);
  const [pastePending, setPastePending] = useState(false);
  const [pasteError, setPasteError] = useState<ClipboardErrorCode | null>(null);
  const pasteGeneration = useRef(0);
  const mounted = useRef(true);
  const activeRef = useRef(active);
  const analysisId = workspace.state.analysis?.analysisId;
  const locked = isXWorkspaceLocked(workspace.state);

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
    for (const itemId of pendingXPreviewIds(workspace.state, active)) void workspace.loadPreview(itemId);
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

  return <XWorkspaceView
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

export function XWorkspaceView({
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
  locale: Locale;
  state: XWorkspaceState;
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
  const locked = isXWorkspaceLocked(state);
  const analyzing = state.analysisStatus === "analyzing";
  const inputLocked = locked || analyzing;
  const analysisError = state.error?.scope === "analysis"
    ? copy[`analysisError.${state.error.code}`]
    : null;

  return <section className="x-workspace" aria-labelledby="x-workspace-heading">
    <header className="x-workspace-heading">
      <h2 id="x-workspace-heading" className="page-title">{copy["route.x"]}</h2>
    </header>
    <form className="x-url-form" onSubmit={onAnalyze}>
      <div className="field">
        <label htmlFor="x-url">{copy.xUrlLabel}</label>
        <div className="url-input-wrap">
          <input
            id="x-url"
            type="url"
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            maxLength={2048}
            value={state.url}
            placeholder={copy.xUrlPlaceholder}
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
        {analysisError ? <p className="field-hint error-message" role="alert">{analysisError}</p> : null}
      </div>
      <button type="submit" className="analyze-button" disabled={inputLocked || state.url.trim() === ""} aria-busy={analyzing}>
        {analyzing ? copy.analysisLoading : copy.xAnalyze}
      </button>
    </form>

    <div className="x-layout">
      <div className="x-gallery-pane" aria-busy={analyzing}>
        {state.analysis
          ? <XMediaGallery
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
              <Icon name="x" size={32} />
              <p>{analyzing ? copy.analysisLoading : copy.xEmpty}</p>
            </div>}
      </div>
      <XDownloadPanel
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
