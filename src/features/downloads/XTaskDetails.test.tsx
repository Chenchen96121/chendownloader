import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { XTaskResultView } from "./XTaskDetails";
import { selectedXResult } from "../x/xTestFixtures";
import { xResultSummary } from "../x/xResultSummary";
import { catalogs } from "../../shared/i18n/catalog";
it("only calls the unchanged, fully saved original post complete",()=>{
  const result={...selectedXResult,postItemCount:1,unavailableCount:0};
  expect(xResultSummary(result,catalogs["en-US"])).toBe("Whole post completed: 1 items");
  expect(xResultSummary({...result,contentChanged:true},catalogs["en-US"])).toBe("1 selected item(s) saved");
});
it.each(["zh-CN","en-US"] as const)("renders honest retained X results independently of new analyses in %s",locale=>{
  const html=renderToStaticMarkup(<XTaskResultView locale={locale} result={{...selectedXResult,contentChanged:true}} />);
  for (const text of ["03.mp4",locale==="zh-CN"?"动图视频":"Animated video",locale==="zh-CN"?"已保存所选 1 项":"1 selected item(s) saved",locale==="zh-CN"?"原帖 4 项，其中 1 项不可用":"Original post: 4 items; 1 unavailable",locale==="zh-CN"?"帖子内容在下载前已发生变化":"The post changed before download"]) expect(html).toContain(text);
  expect(html).not.toMatch(/jobId|itemId|C:\\|Whole post completed|整帖下载完成/);
});
