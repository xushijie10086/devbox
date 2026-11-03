// 项目组 tab 栏的滚动逻辑（纯函数，不碰 DOM，方便测试）。
//
// 背景：tab 多了会超出宽度，横向滚动条被隐藏了。macOS 触控板能横向滑动，但 Windows / Linux 上
// 只有普通鼠标滚轮（只会纵向滚），既没有滚动条也没有别的办法，被挤出去的项目组就点不到了。

/** 滚轮的一次滚动换算成横向滚动的像素数；滚轮模式 1 是「行」、2 是「页」 */
const LINE_PX = 40;

/**
 * 在 tab 栏上滚滚轮时，应当横向滚动多少像素；返回 null 表示不接管（交还给浏览器）。
 * - 栏没有溢出：没东西可滚，不接管，让页面正常纵向滚动
 * - 已经带横向分量（触控板横滑、Shift+滚轮）：浏览器自己会处理
 * - 已经滚到尽头还往那个方向滚：不接管，让页面纵向滚动，不能把页面卡死
 */
export function wheelToScroll({ deltaX, deltaY, deltaMode = 0 }, { scrollLeft, scrollWidth, clientWidth }) {
  if (scrollWidth <= clientWidth + 1) return null;
  if (deltaX !== 0 || deltaY === 0) return null;
  const px = deltaMode === 1 ? deltaY * LINE_PX : deltaMode === 2 ? deltaY * clientWidth : deltaY;
  const max = scrollWidth - clientWidth;
  if (px < 0 && scrollLeft <= 0) return null;
  if (px > 0 && scrollLeft >= max - 1) return null;
  return px;
}

/**
 * 要让 [left, right] 这一段（相对滚动内容的坐标）完整露出来，scrollLeft 应该是多少。
 * 已经完整可见就原样返回；两侧各留 pad 像素，免得贴着边。
 */
export function scrollLeftToReveal({ left, right, scrollLeft, clientWidth, scrollWidth, pad = 24 }) {
  const max = Math.max(scrollWidth - clientWidth, 0);
  let next = scrollLeft;
  if (left - pad < scrollLeft) next = left - pad;
  else if (right + pad > scrollLeft + clientWidth) next = right + pad - clientWidth;
  return Math.min(Math.max(next, 0), max);
}
