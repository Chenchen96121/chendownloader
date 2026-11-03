import { renderToStaticMarkup } from "react-dom/server";
import { expect, it, vi } from "vitest";
import { ClipTimeline } from "./ClipTimeline";

it("renders two labeled keyboard-operable controls with precise values and bounds", () => {
  const html = renderToStaticMarkup(<ClipTimeline durationMs={10000} range={{startMs: 1234, endMs: 9876}} onChange={() => {}} locale="zh-CN" />);
  expect(html.match(/type="range"/g)).toHaveLength(2);
  expect(html).toContain('aria-label="片段开始位置"');
  expect(html).toContain('aria-label="片段结束位置"');
  expect(html).toContain('value="1234"');
  expect(html).toContain('aria-valuemax="9875"');
  expect(html).toContain('aria-valuemin="1235"');
  expect(html).toContain('aria-valuetext="0:00:01.234"');
});

it("emits one valid range on either boundary and respects disabled state", () => {
  const onChange = vi.fn();
  const component = ClipTimeline({durationMs: 10000, range: {startMs: 1000, endMs: 5000}, locale: "en-US", onChange});
  const children = component?.props.children;
  const start = children?.[0];
  const end = children?.[1];
  start.props.onChange({currentTarget: {valueAsNumber: 6000}});
  end.props.onChange({currentTarget: {valueAsNumber: 0}});
  expect(onChange.mock.calls).toEqual([[{startMs: 4999, endMs: 5000}], [{startMs: 1000, endMs: 1001}]]);
  const disabled = ClipTimeline({durationMs: 10000, range: {startMs: 1000, endMs: 5000}, locale: "en-US", onChange, disabled: true});
  disabled?.props.children[0].props.onChange({currentTarget: {valueAsNumber: 0}});
  expect(onChange).toHaveBeenCalledTimes(2);
  expect(ClipTimeline({durationMs: NaN, range: {startMs: 0, endMs: 1}, locale: "en-US", onChange})).toBeNull();
});

it("moves keyboard arrows by a second or a tenth with Shift, without crossing", () => {
  const onChange = vi.fn(); const preventDefault = vi.fn();
  const control = ClipTimeline({durationMs: 10000, range: {startMs: 1000, endMs: 5000}, locale: "en-US", onChange});
  control?.props.children[0].props.onKeyDown({key: "ArrowRight", shiftKey: false, preventDefault});
  control?.props.children[1].props.onKeyDown({key: "ArrowLeft", shiftKey: true, preventDefault});
  expect(onChange.mock.calls).toEqual([[{startMs: 2000, endMs: 5000}], [{startMs: 1000, endMs: 4900}]]);
  expect(preventDefault).toHaveBeenCalledTimes(2);
});
