import type { ReactNode } from "react";

const shapes = {
  download: <><path d="M12 3v12m-5-5 5 5 5-5" /><path d="M4 16v4a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4" /></>,
  youtube: <><rect x="3" y="5" width="18" height="14" rx="4" /><path d="m10 9 5 3-5 3Z" /></>,
  tiktok: <><path d="M14 3v12a5 5 0 1 1-5-5v4a1 1 0 1 0 1 1V3h4Zm0 0c1 4 3 5 6 5v4c-3 0-5-1-6-3" /></>,
  instagram: <><rect x="3" y="3" width="18" height="18" rx="5" /><circle cx="12" cy="12" r="4" /><path d="M17.5 6.5h.01" /></>,
  x: <><path d="m5 4 14 16M19 4 5 20" /></>,
  bilibili: <><rect x="3" y="6" width="18" height="14" rx="3" /><path d="m7 2 3 4m7-4-3 4M8 11v3m8-3v3m-6 3h4" /></>,
  history: <><path d="M3 11a9 9 0 1 1 2.6 7.4M3 4v7h7m2-4v5l3 2" /></>,
  settings: <><path d="m9 3-.7 2.2-2 .9-2.1-.5-1.5 2.6L4.2 10v2l-1.5 1.8 1.5 2.6 2.1-.5 2 .9L9 19h3l.7-2.2 2-.9 2.1.5 1.5-2.6L16.8 12v-2l1.5-1.8-1.5-2.6-2.1.5-2-.9L12 3Z" transform="translate(1.5 1)" /><circle cx="12" cy="12" r="3" /></>,
  clipboard: <><path d="M9 5H6a2 2 0 0 0-2 2v13a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1V7a2 2 0 0 0-2-2h-3" /><rect x="9" y="3" width="6" height="4" rx="1" /></>,
  play: <path d="m9 5 11 7-11 7Z" />,
  image: <><rect x="3" y="3" width="18" height="18" rx="2" /><circle cx="8" cy="8" r="1.5" /><path d="m3 17 5-5 4 4 4-6 5 7" /></>,
  "external-link": <><path d="M14 3h7v7m0-7L11 13" /><path d="M10 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-5" /></>,
  "chevron-down": <path d="m6 9 6 6 6-6" />,
  folder: <path d="M3 7V5a1 1 0 0 1 1-1h5l2 3h9a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1Z" />,
  "check-circle": <><circle cx="12" cy="12" r="9" /><path d="m8 12 3 3 5-6" /></>,
  "file-video": <><path d="M14 3H5v18h14V8Zm0 0v5h5" /><path d="m9 11 5 3-5 3Z" /></>,
  sliders: <><path d="M4 6h4m4 0h8M4 12h9m4 0h3M4 18h2m4 0h10M8 3v6m5 0v6M6 15v6" /></>,
  loader: <path d="M21 12a9 9 0 1 1-9-9" />,
  warning: <><path d="m10.3 4-8 14a2 2 0 0 0 1.7 3h16a2 2 0 0 0 1.7-3l-8-14a2 2 0 0 0-3.4 0Z" /><path d="M12 9v4m0 4h.01" /></>,
  close: <path d="m6 6 12 12M18 6 6 18" />,
} satisfies Record<string, ReactNode>;

export type IconName = keyof typeof shapes;

export function Icon({ name, className, size = 20 }: { name: IconName; className?: string; size?: number }) {
  return <svg className={className ? `ui-icon ${className}` : "ui-icon"} width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">{shapes[name]}</svg>;
}
