import type { ReactNode } from "react";
import { catalogs } from "../shared/i18n/catalog";
import type { Locale } from "../shared/i18n/locale";
import { Icon } from "../shared/ui/Icon";
export type DesktopRoute = "youtube" | "instagram" | "x" | "bilibili" | "history" | "settings";
const platformRoutes = ["youtube", "instagram", "x", "bilibili"] as const;
export function DesktopShell({ locale, route, onNavigate, youtubeWorkspace, instagramWorkspace, xWorkspace, bilibiliWorkspace, settings, downloads }: {
  locale: Locale;
  route: DesktopRoute;
  onNavigate: (route: DesktopRoute) => void;
  youtubeWorkspace: ReactNode;
  instagramWorkspace: ReactNode;
  xWorkspace: ReactNode;
  bilibiliWorkspace: ReactNode;
  settings: ReactNode;
  downloads?: ReactNode;
}) {
  const copy = catalogs[locale];
  return <main className="desktop-shell workbench">
    <aside className="sidebar">
      <div className="product-heading"><span className="product-mark" aria-hidden="true"><img src="/chen-icon.png" alt="" width={32} height={32} /></span><h1>{copy.productTitle}</h1></div>
      <nav aria-label={copy.navigationLabel}>
        {platformRoutes.map((platform) => <button key={platform} type="button" className="nav-button" aria-current={route === platform ? "page" : undefined} onClick={() => onNavigate(platform)}>
          <span className="nav-icon" aria-hidden="true"><Icon name={platform} /></span>
          <span>{copy[`route.${platform}`]}</span>
        </button>)}
        <button type="button" className="nav-button nav-history" aria-current={route === "history" ? "page" : undefined} onClick={() => onNavigate("history")}>
          <span className="nav-icon" aria-hidden="true"><Icon name="history" /></span><span>{copy.queueTitle}</span>
        </button>
      </nav>
      <button type="button" className="nav-button nav-settings" aria-current={route === "settings" ? "page" : undefined} onClick={() => onNavigate("settings")}><span className="nav-icon" aria-hidden="true"><Icon name="settings" /></span>{copy.settingsTitle}</button>
    </aside>
    <div className="desktop-content">
      <div className="workspace-route youtube-route" hidden={route !== "youtube"}>{youtubeWorkspace}</div>
      <div className="workspace-route instagram-route" hidden={route !== "instagram"}>{instagramWorkspace}</div>
      <div className="workspace-route x-route" hidden={route !== "x"}>{xWorkspace}</div>
      <div className="workspace-route bilibili-route" hidden={route !== "bilibili"}>{bilibiliWorkspace}</div>
      <div className="settings-route" hidden={route !== "settings"}><h2 className="page-title">{copy.settingsTitle}</h2>{settings}</div>
      <div className="history-route" hidden={route !== "history"}>{downloads}</div>
    </div>
  </main>;
}
