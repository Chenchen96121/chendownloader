import {
  useCallback,
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
import { InstagramDownloadPanel } from "./InstagramDownloadPanel";
import { InstagramMediaGallery } from "./InstagramMediaGallery";
import {
  isInstagramWorkspaceLocked,
  type InstagramWorkspaceState,
} from "./instagramState";
import { useInstagramWorkspace } from "./useInstagramWorkspace";

export function InstagramWorkspace({
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
  const workspace = useInstagramWorkspace();
  const [destination, setDestination] = useState<DestinationApproval | null>(null);
  const [pastePending, setPastePending] = useState(false);
  const [pasteError, setPasteError] = useState<ClipboardErrorCode | null>(null);
  const [reelEmbedded, setReelEmbedded] = useState(false);
  const [previewRevision, setPreviewRevision] = useState(0);
  const previewRequests = useRef(new Set<string>());
  const pasteGeneration = useRef(0);
  const mounted = useRef(true);
  const activeRef = useRef(active);
  const stateRef = useRef(workspace.state);
  activeRef.current = active;
  stateRef.current = workspace.state;
  const analysisId = workspace.state.analysis?.analysisId;
  const locked = isInstagramWorkspaceLocked(workspace.state);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      pasteGeneration.current += 1;
    };
  }, []);

  useEffect(() => {
    setReelEmbedded(false);
  }, [analysisId, active]);

  useEffect(() => {
    if (!active || locked) {
      pasteGeneration.current += 1;
      setPastePending(false);
      setPasteError(null);
    }
  }, [active, locked]);

  const requestPreview = useCallback((itemId: string) => {
    const current = stateRef.current;
    const analysis = current.analysis;
    if (!mounted.current || !activeRef.current || !analysis
      || previewRequests.current.size >= 2
      || !analysis.items.some(item => item.itemId === itemId && item.previewAvailable)
      || current.previews[itemId]?.status === "loading"
      || current.previews[itemId]?.status === "ready") return;
    const key = `${analysis.analysisId}:${itemId}`;
    if (previewRequests.current.has(key)) return;
    // Reserve synchronously so automatic loads and rapid retry clicks share the same limit.
    previewRequests.current.add(key);
    void workspace.loadPreview(itemId).finally(() => {
      previewRequests.current.delete(key);
      // Also wake the scheduler when a previous analysis's pending request settles.
      if (mounted.current) setPreviewRevision(value => value + 1);
    });
  }, [workspace.loadPreview]);

  useEffect(() => {
    if (analysisId === undefined || locked) return;
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
  }, [analysisId, destinationRevision, locked, workspace.state.analysis?.analysisId]);

  useEffect(() => {
    const analysis = workspace.state.analysis;
    if (!active || analysis === null || analysis.sourceKind !== "post") return;
    for (const item of analysis.items) {
      if (
        item.previewAvailable
        && workspace.state.previews[item.itemId] === undefined
      ) {
        requestPreview(item.itemId);
      }
    }
  }, [active, analysisId, requestPreview, workspace.state.analysis, workspace.state.previews, previewRevision]);

  function changeUrl(url: string) {
    pasteGeneration.current += 1;
    setPastePending(false);
    setPasteError(null);
    setReelEmbedded(false);
    workspace.changeUrl(url);
  }

  async function pasteLink() {
    if (!active || locked || workspace.state.analysisStatus === "analyzing" || pastePending) return;
    const generation = ++pasteGeneration.current;
    setPastePending(true);
    setPasteError(null);
    try {
      const text = await readClipboardText();
      if (mounted.current && generation === pasteGeneration.current && activeRef.current && !isInstagramWorkspaceLocked(stateRef.current)) {
        workspace.changeUrl(text);
      }
    } catch (error) {
      if (mounted.current && generation === pasteGeneration.current && activeRef.current) {
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
    setReelEmbedded(false);
    void workspace.analyze();
  }

  return <InstagramWorkspaceView
    locale={locale}
    state={workspace.state}
    destination={destination}
    eventsReady={workspace.eventsReady}
    active={active}
    pastePending={pastePending}
    pasteError={pasteError}
    reelEmbedded={reelEmbedded}
    onUrlChange={changeUrl}
    onPaste={() => { void pasteLink(); }}
    onAnalyze={analyze}
    onToggleSelection={workspace.toggleSelection}
    onSelectAll={workspace.selectAll}
    onClearSelection={workspace.clearSelection}
    onCompatibilityChange={workspace.setCompatibilityMode}
    onRequestPreview={requestPreview}
    onOpenReel={() => setReelEmbedded(true)}
    onOpenSource={() => { void workspace.openSource(); }}
    onOpenSettings={onOpenSettings}
    onStart={() => {
      if (destination) void workspace.startDownload(destination.destinationId);
    }}
    onCancel={() => { void workspace.cancelDownload(); }}
    onOpenLocation={() => { void workspace.openCompletedLocation(); }}
  />;
}

export function InstagramWorkspaceView({
  locale,
  state,
  destination,
  eventsReady,
  active,
  pastePending,
  pasteError,
  reelEmbedded,
  onUrlChange,
  onPaste,
  onAnalyze,
  onToggleSelection,
  onSelectAll,
  onClearSelection,
  onCompatibilityChange,
  onRequestPreview,
  onOpenReel,
  onOpenSource,
  onOpenSettings,
  onStart,
  onCancel,
  onOpenLocation,
}: {
  locale: Locale;
  state: InstagramWorkspaceState;
  destination: DestinationApproval | null;
  eventsReady: boolean;
  active: boolean;
  pastePending: boolean;
  pasteError: ClipboardErrorCode | null;
  reelEmbedded: boolean;
  onUrlChange: (url: string) => void;
  onPaste: () => void;
  onAnalyze: (event: FormEvent<HTMLFormElement>) => void;
  onToggleSelection: (itemId: string) => void;
  onSelectAll: () => void;
  onClearSelection: () => void;
  onCompatibilityChange: (enabled: boolean) => void;
  onRequestPreview: (itemId: string) => void;
  onOpenReel: () => void;
  onOpenSource: () => void;
  onOpenSettings: () => void;
  onStart: () => void;
  onCancel: () => void;
  onOpenLocation: () => void;
}) {
  const copy = catalogs[locale];
  const locked = isInstagramWorkspaceLocked(state);
  const analyzing = state.analysisStatus === "analyzing";
  const inputLocked = locked || analyzing;
  const analysisError = state.error?.scope === "analysis"
    ? copy[`analysisError.${state.error.code}`]
    : null;

  return <section className="instagram-workspace" aria-labelledby="instagram-workspace-heading">
    <header className="instagram-workspace-heading">
      <h2 id="instagram-workspace-heading" className="page-title">{copy["route.instagram"]}</h2>
    </header>
    <form className="instagram-url-form" onSubmit={onAnalyze}>
      <div className="field">
        <label htmlFor="instagram-url">{copy.instagramUrlLabel}</label>
        <div className="url-input-wrap">
          <input
            id="instagram-url"
            type="url"
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            maxLength={2048}
            value={state.url}
            placeholder={copy.instagramUrlPlaceholder}
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
        {analyzing ? copy.analysisLoading : copy.instagramAnalyze}
      </button>
    </form>

    <div className="instagram-layout">
      <div className="instagram-gallery-pane" aria-busy={analyzing}>
        {state.analysis
          ? <InstagramMediaGallery
              locale={locale}
              analysis={state.analysis}
              selectedItemIds={state.selectedItemIds}
              previews={state.previews}
              locked={locked}
              reelEmbedded={active && reelEmbedded}
              onToggle={onToggleSelection}
              onRequestPreview={onRequestPreview}
              onOpenReel={onOpenReel}
              onOpenSource={onOpenSource}
            />
          : <div className="instagram-empty">
              <Icon name="instagram" size={32} />
              <p>{analyzing ? copy.analysisLoading : copy.instagramEmpty}</p>
            </div>}
      </div>
      <InstagramDownloadPanel
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
