// 全局界面事件流（/api/ui/events）：后端 fire_event 推来的界面事件统一入口。
// 连上/重连即重拉一次设置与插件清单（2026-10-05）：服务重启（换二进制/崩溃恢复）
// 后事件流会断线自动重连，但断连期间推的事件全丢了——页面会一直显示重启前的旧态，
// 用户以为「又没刷新」。onopen 在首次连接也会触发，多一次 GET 无害。
// 断线重连是 EventSource 自带的；调用方只给 onOpen/onEvent 两个回调。
export function connectUiEvents({ onOpen, onEvent }) {
  try {
    const src = new EventSource('/api/ui/events');
    src.onopen = () => {
      if (onOpen) onOpen();
    };
    src.onmessage = async (ev) => {
      let m;
      try {
        m = JSON.parse(ev.data);
      } catch {
        return;
      }
      if (onEvent) await onEvent(m);
    };
    return src;
  } catch {
    return null;
  }
}
