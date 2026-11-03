import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { catalogs } from "../../shared/i18n/catalog";
import type { ToolProvisioningStatus } from "../../shared/contracts/toolProvisioning";
import { StartupGate } from "./StartupGate";

describe("StartupGate", () => {
  const cases: Array<[ToolProvisioningStatus, string, string]> = [
    [{ state: "preparing" }, "正在准备本地组件…", "Preparing local components…"],
    [{ state: "ready" }, "本地组件已就绪", "Local components are ready"],
    [{ state: "repairing" }, "正在修复本地组件…", "Repairing local components…"],
    [
      { state: "failed", errorCode: "package_invalid" },
      "本地组件不可用，请重新下载并完整解压压缩包。",
      "Local components are unavailable. Download and extract the complete archive again.",
    ],
  ];

  it.each(cases)("renders closed bilingual copy for $0", (status, zh, en) => {
    expect(renderToStaticMarkup(<StartupGate status={status} copy={catalogs["zh-CN"]} />)).toContain(zh);
    expect(renderToStaticMarkup(<StartupGate status={status} copy={catalogs["en-US"]} />)).toContain(en);
  });

  it("keeps failure guidance nontechnical", () => {
    const html = renderToStaticMarkup(
      <StartupGate status={{ state: "failed", errorCode: "tool_probe_failed" }} copy={catalogs["zh-CN"]} />,
    );
    expect(html).not.toMatch(/sha|hash|日志|log|C:\\|manifest/i);
  });
});
