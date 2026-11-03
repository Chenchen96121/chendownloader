import type {
  InstagramAnalysisPreview,
  InstagramMediaItem,
} from "../../shared/contracts/instagram";
import { catalogs, type Catalog } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import type { InstagramPreviewState } from "./instagramState";
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

function previewData(preview: InstagramPreviewState | undefined): string | null {
  return preview?.status === "ready" && preview.preview
    ? `data:${preview.preview.mimeType};base64,${preview.preview.dataBase64}`
    : null;
}

function MediaFacts({ item, locale }: { item: InstagramMediaItem; locale: Locale }) {
  const copy = catalogs[locale];
  const facts = [
    item.width !== undefined && item.height !== undefined
      ? `${item.width} × ${item.height}`
      : copy.resolutionUnknown,
    item.container.toUpperCase(),
    `${item.fileSize?.isApproximate ? `${copy.sizeApproximate} ` : ""}${formatBytes(item.fileSize?.bytes, locale)}`,
  ];
  const videoFacts = item.kind === "video"
    ? [
        item.fps === undefined ? null : `${item.fps} FPS`,
        formatCodec(item.videoCodec, copy),
        item.audioCodec === undefined ? copy.noAudio : item.audioCodec.toUpperCase(),
        item.dynamicRange ?? null,
        item.videoBitrateKbps === undefined
          ? null
          : `${item.videoBitrateKbps >= 1000
            ? `${(item.videoBitrateKbps / 1000).toFixed(item.videoBitrateKbps >= 10_000 ? 0 : 1)} Mbps`
            : `${Math.round(item.videoBitrateKbps)} kbps`}`,
      ]
    : [];
  return <><ul className="instagram-media-facts" aria-label={copy.mediaLabel}>
    {facts.map((fact) => (
      <li key={fact}>{fact}</li>
    ))}
  </ul>
    {videoFacts.length > 0 ? <details className="media-extra-details">
      <summary>{copy.sourceDetailsLabel}<Icon name="chevron-down" size={16} /></summary>
      <ul className="media-extra-facts">{videoFacts.filter((fact): fact is string => fact !== null).map((fact) => <li key={fact}>{fact}</li>)}</ul>
    </details> : null}
  </>;
}

export function InstagramMediaGallery({
  locale,
  analysis,
  selectedItemIds,
  previews,
  locked,
  reelEmbedded,
  onToggle,
  onRequestPreview,
  onOpenReel,
  onOpenSource,
}: {
  locale: Locale;
  analysis: InstagramAnalysisPreview;
  selectedItemIds: readonly string[];
  previews: Readonly<Record<string, InstagramPreviewState>>;
  locked: boolean;
  reelEmbedded: boolean;
  onToggle: (itemId: string) => void;
  onRequestPreview: (itemId: string) => void;
  onOpenReel: () => void;
  onOpenSource: () => void;
}) {
  const copy = catalogs[locale];
  const selected = new Set(selectedItemIds);
  const reelEmbedUrl = `https://www.instagram.com/reel/${analysis.shortcode}/embed/`;

  return <section className="instagram-media-gallery" aria-labelledby="instagram-media-heading">
    <header className="instagram-analysis-heading">
      <div>
        <p className="instagram-source-kind">
          {analysis.sourceKind === "reel" ? copy.instagramReelLabel : copy.instagramPostLabel}
        </p>
        <h3 id="instagram-media-heading">{analysis.title}</h3>
      </div>
      {analysis.creator ? <p className="instagram-creator">
        <span>{copy.instagramCreatorLabel}</span>
        <strong>{analysis.creator}</strong>
      </p> : null}
    </header>

    <div className="instagram-media-list">
      {analysis.items.map((item) => {
        const available = item.availability === "ready";
        const preview = previews[item.itemId];
        const imageSource = previewData(preview);
        const itemLabel = interpolate(copy.instagramItemLabel, { ordinal: item.ordinal });
        const kindLabel = item.kind === "image" ? copy.instagramImageLabel : copy.instagramVideoLabel;
        return <article
          key={item.itemId}
          className={`instagram-media-card${selected.has(item.itemId) ? " is-selected" : ""}${available ? "" : " is-unavailable"}`}
          aria-labelledby={`instagram-item-${item.itemId}`}
        >
          <div className="instagram-card-preview">
            {analysis.sourceKind === "reel" && reelEmbedded
              ? <iframe
                  src={reelEmbedUrl}
                  title={`${copy.previewPlayer}: ${analysis.title}`}
                  sandbox="allow-scripts allow-same-origin allow-popups"
                  referrerPolicy="strict-origin-when-cross-origin"
                  loading="lazy"
                />
              : analysis.sourceKind === "reel"
                ? <button type="button" className="instagram-reel-cover" onClick={onOpenReel}>
                    {imageSource
                      ? <img src={imageSource} alt="" />
                      : <span className="instagram-preview-mark" aria-hidden="true"><Icon name="play" size={28} /></span>}
                    <span>{copy.instagramPlayReel}</span>
                  </button>
                : imageSource
                  ? <img src={imageSource} alt={`${itemLabel} ${kindLabel}`} />
                  : item.previewAvailable && preview?.status !== "loading"
                    ? <button type="button" className="instagram-preview-request" onClick={() => onRequestPreview(item.itemId)}>
                        <span className="instagram-preview-mark" aria-hidden="true"><Icon name={item.kind === "image" ? "image" : "play"} size={28} /></span>
                        <span>{copy.instagramLoadPreview}</span>
                      </button>
                    : <div className="instagram-preview-placeholder">
                        <span className="instagram-preview-mark" aria-hidden="true"><Icon name={item.kind === "image" ? "image" : "play"} size={28} /></span>
                        <span>{preview?.status === "loading" ? copy.instagramPreviewLoading : copy.instagramPreviewUnavailable}</span>
                      </div>}
          </div>
          <div className="instagram-card-copy">
            <div className="instagram-item-title">
              <div>
                <p>{itemLabel}</p>
                <h4 id={`instagram-item-${item.itemId}`}>{kindLabel}</h4>
              </div>
              <label className="instagram-item-select">
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
              ? <p className="instagram-item-error" role="status">{copy[`analysisError.${item.unavailableReason}`]}</p>
              : null}
          </div>
        </article>;
      })}
    </div>
    {analysis.sourceKind === "reel" ? <button type="button" className="instagram-open-source" onClick={onOpenSource}>
      <Icon name="external-link" size={16} />{copy.instagramOpenOriginal}
    </button> : null}
  </section>;
}
