import { useId, useState } from "react";
import type { SelectionMode, SelectionOutcome, SelectionPreview } from "../../shared/contracts/selection";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { formatDuration, formatFileSize, formatResolution, formatFps, formatBitrate, formatCodec } from "./formatPresentation";
import { MediaPreview } from "./MediaPreview";
import { Icon } from "../../shared/ui/Icon";

export function SelectionDetails({ data, locale, preview }: {
  data: SelectionPreview; outcome: SelectionOutcome | null; mode: SelectionMode; locale: Locale;
  preview?: { analysisId: string; active: boolean };
}) {
  const copy = catalogs[locale];
  const highestSource = data.rankedVideoFormatIds.map(id=>data.media.formats.find(format=>format.id===id)).find(format=>format?.width !== undefined && format.height !== undefined);
  const highest = highestSource ? formatResolution(highestSource, locale) : copy.resolutionUnknown;
  return <section className="selection-details" aria-label={copy.mediaLabel}>
    <h3 className="media-title">{data.media.title}</h3>
    {preview ? <MediaPreview videoId={data.media.mediaId} analysisId={preview.analysisId} active={preview.active} locale={locale} /> : null}
    <p className="media-summary">{copy.highestResolutionLabel} {highest} · {copy.durationLabel} {formatDuration(data.media.durationSeconds, locale)}</p>
  </section>;
}

export function SelectionSummary({data, outcome, locale}: {data: SelectionPreview; outcome: SelectionOutcome | null; locale: Locale}) {
  const copy = catalogs[locale];
  const [sourceExpanded, setSourceExpanded] = useState(false);
  const sourceFieldsId = useId();
  if (outcome?.status === "unavailable") return <p className="unavailable-message" role="status">{copy[`unavailable.${outcome.code}`]}</p>;
  if (outcome?.status !== "selected") return null;
  const plan = outcome.plan;
  const video = data.media.formats.find(format=>format.id===plan.videoFormatId);
  const audio = data.media.formats.find(format=>format.id===plan.audioFormatId);
  const hasVideo = plan.videoFormatId !== undefined;
  const hasAudio = plan.audioFormatId !== undefined;
  const imageValue = (value: string) => hasVideo ? value : copy.notApplicable;
  const audioValue = (value: string) => hasAudio ? value : copy.noAudio;
  const containers = [...new Set([video?.container, audio?.container].filter((value): value is string=>value !== undefined))].map(value=>value.toUpperCase()).join(" + ") || copy.formatUnknown;
  const sourceFields = [
    [copy.resolutionLabel, imageValue(video ? formatResolution(video, locale) : copy.resolutionUnknown)],
    [copy.fpsLabel, imageValue(formatFps(video?.fps, locale))],
    [copy.videoCodecLabel, imageValue(formatCodec(video?.videoCodec, locale))],
    [copy.audioCodecLabel, audioValue(formatCodec(audio?.audioCodec, locale))],
    ["HDR", imageValue(video?.dynamicRange ?? copy.codecUnknown)],
    [copy.videoBitrateLabel, imageValue(formatBitrate(video?.videoBitrateKbps, locale))],
    [copy.audioBitrateLabel, audioValue(formatBitrate(audio?.audioBitrateKbps, locale))],
    [copy.sourceContainerLabel, containers],
  ];
  const size = plan.videoProcessing === "transcode" || plan.audioProcessing === "transcode" || plan.clip || plan.operation === "extract_video" || plan.operation === "extract_audio" ? undefined
    : plan.estimatedSizeBytes !== undefined ? { bytes: plan.estimatedSizeBytes, isApproximate: true }
      : undefined;
  return <section className="selected-stream-details" aria-label={copy.selectionResultLabel}>
    <div className="source-details-heading"><h4>{copy.sourceDetailsLabel}</h4>
      <button type="button" aria-expanded={sourceExpanded} aria-controls={sourceFieldsId} onClick={() => setSourceExpanded(value => !value)}>{sourceExpanded ? copy.queueCollapse : copy.queueExpand}<Icon name="chevron-down" size={16} /></button>
    </div>
    <dl id={sourceFieldsId} className="source-fields">{(sourceExpanded ? sourceFields : sourceFields.slice(0, 2)).map(([label,value])=><div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl>
    {sourceExpanded && plan.mode === "best_available" ? <p className="field-hint">{copy.recommendationNote}</p> : null}
    <h4 className="target-heading">{copy.saveDetailsLabel}</h4>
    <dl className="result-fields">
      <div><dt>{copy.saveContainerLabel}</dt><dd>{plan.outputContainer?.toUpperCase() ?? copy.formatUnknown}</dd></div>
      <div><dt>{copy.estimatedSizeLabel}</dt><dd>{formatFileSize(size, locale)}</dd></div>
      {plan.compatibilityMode ? <>
        <div><dt>{copy.videoCodecLabel}</dt><dd>{imageValue(formatCodec(plan.outputVideoCodec,locale))}</dd></div>
        <div><dt>{copy.audioCodecLabel}</dt><dd>{audioValue(formatCodec(plan.outputAudioCodec,locale))}</dd></div>
      </> : null}
    </dl>
    {plan.compatibilityMode ? <p className="field-hint">{copy.compatibilityTarget}</p> : null}
    {plan.warnings.includes("hdr_to_sdr") ? <p className="field-hint">{copy["warning.hdr_to_sdr"]}</p> : null}
  </section>;
}
