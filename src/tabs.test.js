import test from "node:test";
import assert from "node:assert/strict";
import { wheelToScroll, scrollLeftToReveal } from "./tabs.js";

const overflow = { scrollLeft: 100, scrollWidth: 1000, clientWidth: 400 };

test("滚轮纵向滚动换算成横向滚动", () => {
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: 100 }, overflow), 100);
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: -60 }, overflow), -60);
});

test("按行 / 按页滚动的滚轮也换算成像素", () => {
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: 3, deltaMode: 1 }, overflow), 120);
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: 1, deltaMode: 2 }, overflow), 400);
});

test("没有溢出时不接管：页面照常纵向滚动", () => {
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: 100 }, { scrollLeft: 0, scrollWidth: 400, clientWidth: 400 }), null);
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: 100 }, { scrollLeft: 0, scrollWidth: 401, clientWidth: 400 }), null, "差 1 像素的取整误差不算溢出");
});

test("本来就带横向分量（触控板横滑、Shift+滚轮）交给浏览器", () => {
  assert.equal(wheelToScroll({ deltaX: 30, deltaY: 5 }, overflow), null);
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: 0 }, overflow), null);
});

test("滚到尽头后继续往那个方向滚，让出给页面纵向滚动", () => {
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: -50 }, { ...overflow, scrollLeft: 0 }), null);
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: 50 }, { ...overflow, scrollLeft: 600 }), null);
  // 反方向仍然可以滚
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: 50 }, { ...overflow, scrollLeft: 0 }), 50);
  assert.equal(wheelToScroll({ deltaX: 0, deltaY: -50 }, { ...overflow, scrollLeft: 600 }), -50);
});

const view = { clientWidth: 400, scrollWidth: 1000 };

test("已经完整可见的 tab 不用动", () => {
  assert.equal(scrollLeftToReveal({ ...view, left: 150, right: 250, scrollLeft: 100 }), 100);
});

test("右侧被遮住：滚到刚好露出来，并留出边距", () => {
  assert.equal(scrollLeftToReveal({ ...view, left: 700, right: 800, scrollLeft: 0 }), 424);
});

test("左侧被遮住：往回滚，并留出边距", () => {
  assert.equal(scrollLeftToReveal({ ...view, left: 60, right: 160, scrollLeft: 300 }), 36);
});

test("不会滚出内容范围", () => {
  assert.equal(scrollLeftToReveal({ ...view, left: 5, right: 80, scrollLeft: 300 }), 0);
  assert.equal(scrollLeftToReveal({ ...view, left: 940, right: 990, scrollLeft: 0 }), 600);
  assert.equal(scrollLeftToReveal({ left: 0, right: 50, scrollLeft: 0, clientWidth: 400, scrollWidth: 300 }), 0, "内容比容器短");
});
