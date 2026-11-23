// 剪贴板写入（消息复制/插件右键复制共用口径）：优先异步 Clipboard API，
// 被拒/无头环境回落 execCommand——不能让「复制」变摆设。返回是否写成功，
// 调用方要给出可见反馈（错误不许静默）。
export async function writeClipboard(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    /* 走回落 */
  }
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.setAttribute('readonly', '');
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  document.body.appendChild(ta);
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand('copy');
  } catch {}
  ta.remove();
  return ok;
}
