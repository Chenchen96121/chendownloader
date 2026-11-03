import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { youtubePreviewUrls } from "./youtubePreview";
import { MediaPreview } from "./MediaPreview";

describe("isolated YouTube preview", () => {
  it.each(["", "short", "../../evil1", "demoYT00001?token=x", "https://evil.invalid", "demoYT0000\n"])("rejects invalid ID %j", (id) => {
    expect(youtubePreviewUrls(id)).toBeUndefined();
  });
  it("builds only fixed public thumbnail and embed URLs", () => {
    expect(youtubePreviewUrls("demoYT00001")).toEqual({
      thumbnail: "https://i.ytimg.com/vi/demoYT00001/hqdefault.jpg",
      embed: "https://www.youtube-nocookie.com/embed/demoYT00001?autoplay=1",
    });
  });
  it("shows the cover and original-page action without loading a player", () => {
    const html = renderToStaticMarkup(<MediaPreview videoId="demoYT00001" analysisId="1" locale="zh-CN" active />);
    expect(html).toContain("i.ytimg.com/vi/demoYT00001/hqdefault.jpg");
    expect(html).toContain("打开原网页");
    expect(html).not.toContain("<iframe");
  });
  it("does not load remote content when hidden or the ID is invalid", () => {
    for (const props of [{ videoId: "invalid", active: true }, { videoId: "demoYT00001", active: false }]) {
      const html = renderToStaticMarkup(<MediaPreview {...props} analysisId="1" locale="en-US" />);
      expect(html).not.toContain("<img");
      expect(html).not.toContain("<iframe");
    }
  });
});
