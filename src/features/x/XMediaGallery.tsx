import type {
  XAnalysisPreview,
  XMediaItem,
} from "../../shared/contracts/x";
import { catalogs, type Catalog } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import type { XPreviewState } from "./xState";
import { Icon } from "../../shared/ui/Icon";

function interpolate(template: string, values: Record<string, string | number>): string {
  return template.replace(/\{(\w+)\}/gu, (_, key: string) => String(values[key] ?? ""));
}

function formatBytes(bytes: number | undefined, locale: Locale): string {
  if (bytes === undefined) return catalogs[locale].sizeUnknown;
  const units = ["B", "KB", "MB", "GB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

function formatCodec(codec: string | undefined, copy: Catalog): string {
  if (codec === undefined) return copy.codecUnknown;
  const normalized = codec.toLowerCase();
  if (normalized === "h264" || normalized === "avc" || normalized === "avc1") return "H.264";
  if (normalized === "h265" || normalized === "hevc" || normalized === "hev1") return "H.265";
  return normalized === "vp9" ? "VP9" : normalized === "av1" ? "AV1" : codec.toUpperCase();
}

function previewData(preview: XPreviewState | undefined): string | null {
  return preview?.status === "ready" && preview.preview
    ? `data:${preview.preview.mimeType};base64,${preview.preview.dataBase64}`
    : null;
}

function MediaFacts({ item, locale }: { item: XMediaItem; locale: Locale }) {
  const copy = catalogs[locale];
  const facts = [
    { key: "resolution", value: item.width !== undefined && item.height !== undefined
      ? `${item.width} × ${item.height}`
      : copy.resolutionUnknown },
    { key: "container", value: item.container === "unknown" ? copy.formatUnknown : item.container.toUpperCase() },
    { key: "size", value: `${item.fileSize?.isApproximate ? `${copy.sizeApproximate} ` : ""}${formatBytes(item.fileSize?.bytes, locale)}` },
  ];
  const videoFacts = item.kind !== "image"
    ? [
        { key: "fps", value: item.fps === undefined ? null : `${item.fps} FPS` },
        { key: "videoCodec", value: formatCodec(item.videoCodec, copy) },
        { key: "audioCodec", value: item.audioCodec === undefined ? (item.availability === "ready" ? copy.noAudio : copy.codecUnknown) : item.audioCodec.toUpperCase() },
        { key: "range", value: item.dynamicRange ?? null },
        { key: "videoBitrate", value: item.videoBitrateKbps === undefined
          ? null
          : `${item.videoBitrateKbps >= 1000
            ? `${(item.videoBitrateKbps / 1000).toFixed(item.videoBitrateKbps >= 10_000 ? 0 : 1)} Mbps`
            : `${Math.round(item.videoBitrateKbps)} kbps`}` },
      ]
    : [];
  return <><ul className="x-media-facts" aria-label={copy.mediaLabel}>
    {facts.map((fact) => (
      <li key={fact.key}>{fact.value}</li>
    ))}
  </ul>
    {videoFacts.length > 0 ? <details className="media-extra-details">
      <summary>{copy.sourceDetailsLabel}<Icon name="chevron-down" size={16} /></summary>
      <ul className="media-extra-facts">{videoFacts.filter(fact => fact.value !== null).map((fact) => <li key={fact.key}>{fact.value}</li>)}</ul>
    </details> : null}
  </>;
}

export function XMediaGallery({
  locale,
  analysis,
  selectedItemIds,
  previews,
  locked,
  onToggle,
  onRequestPreview,
  onOpenSource,
}: {
  locale: Locale;
  analysis: XAnalysisPreview;
  selectedItemIds: readonly string[];
  previews: Readonly<Record<string, XPreviewState>>;
  locked: boolean;
  onToggle: (itemId: string) => void;
  onRequestPreview: (itemId: string) => void;
  onOpenSource: () => void;
}) {
  const copy = catalogs[locale];
  const selected = new Set(selectedItemIds);

  return <section className="x-media-gallery" aria-labelledby="x-media-heading">
    <header className="x-analysis-heading">
      <div>
        <p className="x-source-kind">
          {copy.xPostLabel}
        </p>
        <h3 id="x-media-heading">{analysis.title}</h3>
      </div>
      {analysis.creator ? <p className="x-creator">
        <span>{copy.xCreatorLabel}</span>
        <strong>{analysis.creator}</strong>
      </p> : null}
    </header>

    <div className="x-media-list">
      {analysis.items.map((item) => {
        const available = item.availability === "ready";
        const preview = previews[item.itemId];
        const imageSource = previewData(preview);
        const itemLabel = interpolate(copy.xItemLabel, { ordinal: item.ordinal });
        const kindLabel = item.kind === "image" ? copy.xImageLabel : item.kind === "animated_video" ? copy.xAnimatedVideoLabel : copy.xVideoLabel;
        return <article
          key={item.itemId}
          className={`x-media-card${selected.has(item.itemId) ? " is-selected" : ""}${available ? "" : " is-unavailable"}`}
          aria-labelledby={`x-item-${item.itemId}`}
        >
          <div className="x-card-preview">
            {imageSource
              ? <img src={imageSource} alt={`${itemLabel} ${kindLabel}`} />
              : item.previewAvailable && preview?.status !== "loading"
                ? <button type="button" className="x-preview-request" onClick={() => onRequestPreview(item.itemId)}>
                    <span className="x-preview-mark" aria-hidden="true"><Icon name={item.kind === "image" ? "image" : "play"} size={28} /></span>
                    <span>{copy.xLoadPreview}</span>
                  </button>
                : <div className="x-preview-placeholder">
                    <span className="x-preview-mark" aria-hidden="true"><Icon name={item.kind === "image" ? "image" : "play"} size={28} /></span>
                    <span>{preview?.status === "loading" ? copy.xPreviewLoading : copy.xPreviewUnavailable}</span>
                  </div>}
          </div>
          <div className="x-card-copy">
            <div className="x-item-title">
              <div>
                <p>{itemLabel}</p>
                <h4 id={`x-item-${item.itemId}`}>{kindLabel}</h4>
              </div>
              <label className="x-item-select">
                <span>{`${itemLabel} ${kindLabel}`}</span>
                <input
                  type="checkbox"
                  checked={selected.has(item.itemId)}
                  disabled={locked || !available}
                  onChange={() => onToggle(item.itemId)}
                />
              </label>
            </div>
            <MediaFacts item={item} locale={locale} />
            {!available && item.unavailableReason
              ? <p className="x-item-error" role="status">{copy[`analysisError.${item.unavailableReason}`]}</p>
              : null}
          </div>
        </article>;
      })}
    </div>
    <button type="button" className="x-open-source" onClick={onOpenSource}>
      <Icon name="external-link" size={16} />{copy.xOpenOriginal}
    </button>
  </section>;
}
