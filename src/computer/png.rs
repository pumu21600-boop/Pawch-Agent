//! 极简 PNG 编码器（computer 截图用；零新增依赖）。
//!
//! 为什么不引 image/png crate：本机验证时外网不通，且截图的管线已经很确定——
//! GDI GetDIBits 出 32bpp BGRA，去掉 alpha 就是 RGB truecolor，PNG 只剩
//! 「zlib(flate2 已有) + CRC32(手写 20 行) + 四个 chunk」三件事。
//! 只编码、不解码（面板显示走浏览器 <img>，不用本模块）。

use std::io::Write;

/// PNG 签名（8 字节）。
const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// CRC32（IEEE 802.3，PNG 用的多项式 0xEDB88320 反射形式）。表惰性生成。
fn crc32_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, slot) in t.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *slot = c;
    }
    t
}

fn crc32(data: &[u8]) -> u32 {
    let t = crc32_table();
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// 写一个 chunk：len(4 BE) + type(4) + data + crc32(type+data)(4 BE)。
fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// BGRA（每像素 4 字节，GDI GetDIBits 的 32bpp 输出）→ RGB PNG 字节。
/// `stride` = 每行字节数（可能 > width*4，GDI 会对齐到 4 的倍数——32bpp 天然对齐，
/// 但保守起见按传入 stride 走；行内多出来的字节跳过）。
pub fn encode_png_from_bgra(bgra: &[u8], width: u32, height: u32, stride: usize) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("截图尺寸为 0".into());
    }
    let row_bytes = (width as usize) * 4;
    if stride < row_bytes {
        return Err(format!("stride {stride} 小于行字节数 {row_bytes}"));
    }
    // 扫描线序列：每行 = 1 字节 filter(0=None) + width*3 字节 RGB
    let mut raw = Vec::with_capacity((row_bytes / 4 * 3 + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(0u8); // filter type None
        let row = &bgra[y * stride..y * stride + row_bytes];
        for px in row.as_chunks::<4>().0 {
            raw.push(px[2]); // R ← B位 后的 G/R：BGRA 序 [B,G,R,A]
            raw.push(px[1]);
            raw.push(px[0]);
        }
    }
    // IHDR：width/height/bit_depth(8)/color_type(2=RGB)/compression(0)/filter(0)/interlace(0)
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut out = Vec::with_capacity(raw.len() / 2 + 64);
    out.extend_from_slice(&SIG);
    write_chunk(&mut out, b"IHDR", &ihdr);
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&raw).map_err(|e| format!("PNG zlib 压缩失败: {e}"))?;
    let idat = enc.finish().map_err(|e| format!("PNG zlib 收尾失败: {e}"))?;
    write_chunk(&mut out, b"IDAT", &idat);
    write_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

/// 最近邻降采样 BGRA（截图内联给模型前压尺寸用，纯指针算术无依赖）。
/// 返回 (新宽, 新高, 新 stride, 新缓冲)。超过上限才需要调；不超时原样返回 None。
pub fn downscale_bgra(
    bgra: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    max_w: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    if width <= max_w {
        return None;
    }
    let scale = max_w as f64 / width as f64;
    let new_w = max_w;
    let new_h = ((height as f64 * scale).round() as u32).max(1);
    let new_stride = (new_w as usize) * 4;
    let mut out = vec![0u8; new_stride * new_h as usize];
    for y in 0..new_h as usize {
        let sy = ((y as f64 / scale) as usize).min(height as usize - 1);
        for x in 0..new_w as usize {
            let sx = ((x as f64 / scale) as usize).min(width as usize - 1);
            let src = sy * stride + sx * 4;
            let dst = y * new_stride + x * 4;
            out[dst..dst + 4].copy_from_slice(&bgra[src..src + 4]);
        }
    }
    Some((new_w, new_h, out))
}

/// 裁一块并（可选）最近邻放大：局部截图（screenshot region）与点击回执共用。
/// `rect` = (x, y, w, h)，缓冲内像素坐标。返回 (宽, 高, 缓冲, stride)；
/// 调用方负责先夹好范围，这里只做保守截断。
///
/// 放大走最近邻是有意的：源像素放大成 scale×scale 的方块后边界清清楚楚，
/// 模型拿它量「偏了几像素」比插值糊出来的图准得多。
pub fn crop_bgra(
    bgra: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    rect: (u32, u32, u32, u32),
    scale: u32,
) -> Result<(u32, u32, Vec<u8>, usize), String> {
    let (x, y, w, h) = rect;
    let w = w.min(width.saturating_sub(x));
    let h = h.min(height.saturating_sub(y));
    if w == 0 || h == 0 {
        return Err(format!("裁剪区域为空（x={x} y={y}，图 {width}×{height}）"));
    }
    let s = scale.clamp(1, 4) as usize;
    let new_w = w * s as u32;
    let new_h = h * s as u32;
    let new_stride = (new_w as usize) * 4;
    let mut out = vec![0u8; new_stride * new_h as usize];
    for dy in 0..new_h as usize {
        let sy = y as usize + dy / s;
        for dx in 0..new_w as usize {
            let sx = x as usize + dx / s;
            let src = sy * stride + sx * 4;
            let dst = dy * new_stride + dx * 4;
            out[dst..dst + 4].copy_from_slice(&bgra[src..src + 4]);
        }
    }
    Ok((new_w, new_h, out, new_stride))
}

/// 在图上画一个十字标记（点击回执用：让模型一眼看到「我到底点在哪」）。
///
/// 2026-11-02 二次实测：回执给了 1:1 原图，模型仍把自己的点击点当成目标位置，
/// 连点三次几乎没挪窝（1013→1020→1006）。画个十字后「我点的」和「目标在哪」
/// 变成图上两个可见的东西，不用再靠心算中心偏移。
/// 中间留 gap 空档，避免十字糊住要看的小控件（开关圆点才二十来像素）。
pub fn draw_crosshair(bgra: &mut [u8], width: u32, height: u32, stride: usize, at: (u32, u32), arm: u32, gap: u32) {
    let (cx, cy) = at;
    let thick = 2u32;
    let mut put = |x: u32, y: u32| {
        if x >= width || y >= height {
            return;
        }
        let i = y as usize * stride + x as usize * 4;
        if i + 4 <= bgra.len() {
            // BGRA 纯红（B=0,G=0,R=255,A=255）——界面里极少出现，不会和目标混淆
            bgra[i..i + 4].copy_from_slice(&[0, 0, 255, 255]);
        }
    };
    for a in 0..=arm {
        for t in 0..thick {
            if a < gap {
                continue;
            }
            put(cx.saturating_sub(a), cy.saturating_add(t));
            put(cx.saturating_add(a), cy.saturating_add(t));
            put(cx.saturating_sub(t), cy.saturating_add(a));
            put(cx.saturating_add(t), cy.saturating_add(a));
            put(cx.saturating_sub(a), cy.saturating_sub(t));
            put(cx.saturating_add(a), cy.saturating_sub(t));
            put(cx.saturating_sub(t), cy.saturating_sub(a));
            put(cx.saturating_add(t), cy.saturating_sub(a));
        }
    }
}

/// 两块同尺寸 BGRA 的差异占比（0.0-1.0）。只算肉眼可见的色差——抗锯齿/动画
/// 尾帧带来的微抖动不该被算成「有反应」。
pub fn diff_ratio(a: &[u8], b: &[u8], width: u32, height: u32, stride: usize) -> f64 {
    let total = (width as usize) * (height as usize);
    if total == 0 {
        return 0.0;
    }
    let mut changed = 0usize;
    for y in 0..height as usize {
        let row = y * stride;
        for x in 0..width as usize {
            let i = row + x * 4;
            if i + 4 > a.len() || i + 4 > b.len() {
                continue;
            }
            let d = (a[i] as i32 - b[i] as i32).abs()
                + (a[i + 1] as i32 - b[i + 1] as i32).abs()
                + (a[i + 2] as i32 - b[i + 2] as i32).abs();
            if d > 30 {
                changed += 1;
            }
        }
    }
    changed as f64 / total as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_signature_and_chunks_wellformed() {
        // 2×1 纯红像素（BGRA: [0,0,255,255]）
        let bgra = [0u8, 0, 255, 255, 0, 0, 255, 255];
        let png = encode_png_from_bgra(&bgra, 2, 1, 8).unwrap();
        assert!(png.starts_with(&SIG), "签名必须在最前");
        assert!(png.windows(4).any(|w| w == b"IHDR"), "IHDR chunk 在");
        assert!(png.windows(4).any(|w| w == b"IDAT"), "IDAT chunk 在");
        assert!(png.ends_with(b"IEND\xaeB`\x82") || png.windows(4).any(|w| w == b"IEND"), "IEND 收尾");
        // IHDR 里宽高正确（BE u32）
        assert_eq!(&png[16..20], &2u32.to_be_bytes());
        assert_eq!(&png[20..24], &1u32.to_be_bytes());
        assert_eq!(png[24], 8, "bit depth 8");
        assert_eq!(png[25], 2, "color type = truecolor RGB");
    }

    #[test]
    fn crc32_known_vector() {
        // CRC32("IEND") = 0xAE426082（PNG 规范里的经典值）
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn downscale_halves_dimensions() {
        // 4×2 BGRA → 上限 2 宽 → 2×1，取最近邻（左上角像素）
        let mut bgra = vec![0u8; 16];
        for (i, px) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px.copy_from_slice(&[i as u8, 100, 200, 255]);
        }
        let (w, h, out) = downscale_bgra(&bgra, 4, 2, 16, 2).expect("该降");
        assert_eq!((w, h), (2, 1));
        assert_eq!(out.len(), 8);
        assert_eq!(&out[0..4], &[0, 100, 200, 255], "第一个像素取源 (0,0)");
        // 不超上限：None（调用方原样编码）
        assert!(downscale_bgra(&bgra, 4, 2, 16, 8).is_none());
    }

    #[test]
    fn stride_padding_skipped() {
        // stride 大于行字节（对齐填充）时编码不错位
        let mut bgra = vec![0u8; 3 * 12]; // 宽 2 高 2，stride 12（行 8 + 4 填充）
        bgra[0] = 255; // (0,0) 蓝位
        bgra[12] = 255; // (0,1) 蓝位
        let png = encode_png_from_bgra(&bgra, 2, 2, 12).unwrap();
        assert!(png.len() > 40);
    }

    #[test]
    fn crop_takes_requested_pixels_and_scales() {
        // 4×4 图，每像素 R 位写 x 坐标（BGRA 序 [B,G,R,A]）
        let mut bgra = vec![0u8; 4 * 4 * 16];
        for y in 0..4usize {
            for x in 0..4usize {
                let i = (y * 4 + x) * 4;
                bgra[i + 2] = x as u8;
                bgra[i + 1] = y as u8;
            }
        }
        let (w, h, out, stride) = crop_bgra(&bgra, 4, 4, 16, (1, 2, 2, 2), 1).unwrap();
        assert_eq!((w, h, stride), (2, 2, 8));
        assert_eq!(&out[0..4], &[0, 2, 1, 0], "左上角是源 (1,2)");
        // 放大 2×：每个源像素变 2×2 方块
        let (w, h, out, _) = crop_bgra(&bgra, 4, 4, 16, (0, 0, 2, 2), 2).unwrap();
        assert_eq!((w, h), (4, 4));
        assert_eq!(&out[0..4], &out[4..8], "同一源像素横向摊成两格");
        assert_eq!(&out[0..4], &out[16..20], "同一源像素纵向摊成两格（下一行起点）");
        // 越界给空：报错不崩
        assert!(crop_bgra(&bgra, 4, 4, 16, (4, 0, 2, 2), 1).is_err());
    }

    #[test]
    fn crosshair_marks_only_the_gap_ring() {
        // 40×40 深灰图，十字画在中心：中心留空（不糊住目标），外圈是纯红
        let mut bgra = vec![40u8; 40 * 40 * 4];
        draw_crosshair(&mut bgra, 40, 40, 160, (20, 20), 8, 3);
        let px = |x: usize, y: usize| -> [u8; 4] {
            let i = y * 160 + x * 4;
            [bgra[i], bgra[i + 1], bgra[i + 2], bgra[i + 3]]
        };
        assert_eq!(px(20, 20), [40, 40, 40, 40], "正中留空，别糊住目标");
        assert_eq!(px(20, 25), [0, 0, 255, 255], "竖臂在 gap 之外是纯红");
        assert_eq!(px(25, 20), [0, 0, 255, 255], "横臂同理");
        assert_eq!(px(35, 35), [40, 40, 40, 40], "臂长之外不动");
        // 贴边不 panic、不越界写
        draw_crosshair(&mut bgra, 40, 40, 160, (0, 0), 8, 3);
        draw_crosshair(&mut bgra, 40, 40, 160, (39, 39), 8, 3);
    }

    #[test]
    fn diff_ratio_counts_only_visible_changes() {
        let a = vec![100u8; 10 * 10 * 4];
        let mut b = a.clone();
        assert_eq!(diff_ratio(&a, &b, 10, 10, 40), 0.0, "完全一样 = 0");
        // 轻微抖动（每个通道差 5，合计 15 < 30）不算变化
        b.fill(105);
        assert_eq!(diff_ratio(&a, &b, 10, 10, 40), 0.0, "抗锯齿抖动不该算成有反应");
        // 20 个像素真变了 → 20/100
        let mut c = a.clone();
        for i in 0..20 {
            let base = i * 4;
            c[base] = 255;
        }
        assert!((diff_ratio(&a, &c, 10, 10, 40) - 0.2).abs() < 1e-9, "改了多少像素就报多少比例");
    }
}
