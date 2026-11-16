// 文件预览的纯函数：扩展名 → 预览形态（2026-11-14 阶段 A-2 自 App.svelte 平移）。
// App.svelte（loadPreview）与 PreviewPane.svelte 共用，改这里两边同步生效。

const RP_IMG_EXT = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'ico', 'avif']);
const RP_PDF_EXT = new Set(['pdf']);
const RP_MEDIA_EXT = new Set(['mp4', 'm4v', 'webm', 'mov', 'mp3', 'wav', 'ogg', 'm4a', 'flac']);
const RP_DOCX_EXT = new Set(['docx']);
const RP_XLSX_EXT = new Set(['xlsx']);
const RP_PPTX_EXT = new Set(['pptx']);
const RP_DOC_EXT = new Set(['ppt', 'odt', 'ods', 'odp']);

export function extOf(path) {
  const i = String(path).lastIndexOf('.');
  return i > 0 ? String(path).slice(i + 1).toLowerCase() : '';
}

// 预览形态：image / pdf / media / docx / xlsx / pptx / doc / text
export function previewKind(path) {
  const e = extOf(path);
  if (RP_IMG_EXT.has(e)) return 'image';
  if (RP_PDF_EXT.has(e)) return 'pdf';
  if (RP_MEDIA_EXT.has(e)) return 'media';
  if (RP_DOCX_EXT.has(e)) return 'docx';
  if (RP_XLSX_EXT.has(e)) return 'xlsx';
  if (RP_PPTX_EXT.has(e)) return 'pptx';
  if (RP_DOC_EXT.has(e)) return 'doc';
  return 'text';
}

// 后端 200KB 头尾压缩标记（context::OMITTED_MARK）：压缩过的内容拿去保存会把中间
// 省略掉的部分整段写没——这类文件不给编辑，宁可不让改也不许写坏文件
export function previewTruncated(text) {
  return String(text || '').includes('……[中间省略');
}
