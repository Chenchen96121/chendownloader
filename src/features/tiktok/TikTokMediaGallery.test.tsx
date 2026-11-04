import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { TikTokMediaGallery } from "./TikTokMediaGallery";
import { mixedTikTokPost } from "./tiktokTestFixtures";
it.each(["zh-CN","en-US"] as const)("shows ordered local covers, source facts and independent availability in %s", locale => {
  const html = renderToStaticMarkup(<TikTokMediaGallery locale={locale} analysis={mixedTikTokPost} selectedItemIds={["11","12","13"]}
    previews={{"14":{status:"ready",requestId:1,preview:{mimeType:"image/jpeg",dataBase64:"AQID"}}}} locked={false}
    onToggle={() => {}} onRequestPreview={() => {}} onOpenSource={() => {}} />);
  for (const fact of ["1600 × 900","1920 × 1080","1280 × 720","MP4","H.264","60 FPS","1.91 MB"]) expect(html).toContain(fact);
  const primaryFacts = [...html.matchAll(/<ul class="x-media-facts"[^>]*>(.*?)<\/ul>/g)].map(match => {
    if (match[1] === undefined) throw new Error("Primary media facts capture is missing");
    return match[1];
  });
  expect(primaryFacts).toHaveLength(4);
  expect(primaryFacts.some(facts => facts.includes("MP4"))).toBe(false);
  expect(primaryFacts.some(facts => /H\.264/.test(facts))).toBe(false);
  expect(primaryFacts.some(facts=>facts.includes("60 FPS"))).toBe(true);
  expect(html).toContain('<details class="media-extra-details"><summary>');
  expect(html).toMatch(/<ul class="media-extra-facts">.*?H\.264/);
  expect(html).toContain(locale === "zh-CN" ? "视频" : "Video");
  expect(html).toContain(locale === "zh-CN" ? "无音频" : "No audio");
  expect(html).toContain("data:image/jpeg;base64,AQID");
  expect(html.match(/type="checkbox"/g)).toHaveLength(4);
  expect(html.match(/checked=""/g)).toHaveLength(3);
  expect(html.match(/disabled=""/g)).toHaveLength(1);
  expect(html).not.toMatch(/<iframe|src="https:|\/embed\/|shortcode|video\.twimg/);
});
