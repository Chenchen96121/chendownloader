import type { BilibiliAnalysisPreview } from "../../shared/contracts/bilibili";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { formatDuration, formatResolution } from "../analyze/formatPresentation";
import type { BilibiliCoverState } from "./bilibiliState";
import { Icon } from "../../shared/ui/Icon";

export function BilibiliPreview({ data, locale, cover, active, openFailed, onOpenSource, onRetryCover, onImageError }: {
  data: BilibiliAnalysisPreview; locale: Locale; cover: BilibiliCoverState | null; active: boolean;
  openFailed: boolean; onOpenSource: () => void; onRetryCover: () => void; onImageError: () => void;
}) {
  const copy = catalogs[locale];
  const highest = data.rankedVideoFormatIds.map(id => data.media.formats.find(row => row.id === id))
    .find(row => row?.width !== undefined && row.height !== undefined);
  const current = cover?.analysisId === data.analysisId ? cover : null;
  const payload = current?.status === "ready" ? current.payload : null;
  return <section className="selection-details" aria-label={copy.mediaLabel}>
    <h3 className="media-title">{data.media.title}</h3>
    <div className="media-preview bilibili-preview" aria-busy={!current || current.status === "loading"}>
      {payload ? <button type="button" className="cover-button" aria-label={copy.bilibiliCoverLabel} disabled={!active} onClick={onOpenSource}>
        <img src={`data:${payload.mimeType};base64,${payload.dataBase64}`} alt="" onError={onImageError} />
        <span className="bilibili-cover-open" aria-hidden="true"><Icon name="external-link" /></span>
      </button> : <div className="bilibili-cover-placeholder" role="status">
        <Icon name="image" size={32} />
        <p>{current?.status === "error" ? copy.bilibiliCoverUnavailable : copy.bilibiliCoverLoading}</p>
        {current?.status === "error" ? <button type="button" disabled={!active} onClick={onRetryCover}>{copy.bilibiliCoverRetry}</button> : null}
      </div>}
      <div className="preview-actions"><button type="button" disabled={!active} onClick={onOpenSource}><Icon name="external-link" size={16} />{copy.previewOriginal}</button></div>
      {openFailed ? <p className="field-hint error-message" role="alert">{copy.previewOpenFailed}</p> : null}
    </div>
    <p className="media-summary">{copy.highestResolutionLabel} {highest ? formatResolution(highest, locale) : copy.resolutionUnknown} · {copy.durationLabel} {formatDuration(data.media.durationSeconds, locale)} · {copy.bilibiliPartLabel.replace("{part}", String(data.part))}</p>
  </section>;
}
