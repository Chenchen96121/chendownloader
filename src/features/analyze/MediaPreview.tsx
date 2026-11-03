import { useState } from "react";
import { openMediaPage } from "../../shared/ipc/liveAnalysis";
import { catalogs } from "../../shared/i18n/catalog";
import type { Locale } from "../../shared/i18n/locale";
import { youtubePreviewUrls } from "./youtubePreview";
import { Icon } from "../../shared/ui/Icon";

export function MediaPreview(props: { videoId: string; analysisId: string; locale: Locale; active: boolean }) {
  const urls = youtubePreviewUrls(props.videoId);
  // Conditional mounting stops the player and resets its interaction state on route/media changes.
  return props.active && urls ? <Preview key={props.videoId} {...props} urls={urls} /> : null;
}

function Preview({ urls, analysisId, locale }: {
  urls: { thumbnail: string; embed: string }; analysisId: string; locale: Locale;
}) {
  const [playing, setPlaying] = useState(false);
  const [openFailed, setOpenFailed] = useState(false);
  const copy = catalogs[locale];
  async function openOriginal() {
    setOpenFailed(false);
    try { await openMediaPage(analysisId); } catch { setOpenFailed(true); }
  }
  return <div className="media-preview">
    {playing ? <iframe
      src={urls.embed} title={copy.previewPlayer}
      referrerPolicy="strict-origin-when-cross-origin"
      sandbox="allow-scripts allow-same-origin allow-presentation"
      allow="autoplay; encrypted-media; fullscreen; picture-in-picture" allowFullScreen
    /> : <button type="button" className="cover-button" onClick={() => setPlaying(true)} aria-label={copy.previewPlay}>
      <img src={urls.thumbnail} alt="" referrerPolicy="strict-origin-when-cross-origin" />
      <span className="cover-play" aria-hidden="true"><Icon name="play" size={28} /></span>
    </button>}
    <div className="preview-actions">
      {playing ? <button type="button" onClick={() => setPlaying(false)}>{copy.previewStop}</button> : null}
      <button type="button" onClick={() => void openOriginal()}><Icon name="external-link" size={16} />{copy.previewOriginal}</button>
    </div>
    {playing ? <p className="preview-hint">{copy.previewFallback}</p> : null}
    {openFailed ? <p role="alert">{copy.previewOpenFailed}</p> : null}
  </div>;
}
