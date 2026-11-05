//! AppContainer 沙箱模式原语（施工单 019）—— 受限 token 沙箱的升级路径
//!
//! 机制：LowBox token（由 CreateProcess 的 SECURITY_CAPABILITIES 启动属性自动创建，
//! 见 proc.rs 的 appcontainer 分支）。
//! - 默认**无网络出向**：不授予 INTERNET_CLIENT 能力 SID（S-1-15-3-3）即断网；
//!   `allow_network = true` 时 profile 创建带该能力；
//! - 文件/注册表默认全拒：工作区目录由 icacls 授 AppContainer SID 的 ACE（幂等）；
//! - Job 保险丝照用（超时整树击杀），与此前语义一致。
//!
//! 开关：config.toml `[sandbox] mode = "appcontainer"`（默认 "restricted" 向后兼容）；
//! 实验特性，异常时切回 "restricted" 即可。

use std::{path::Path, ptr};

use windows_sys::Win32::Foundation::{HANDLE, PSID};
use windows_sys::Win32::Security::SID_AND_ATTRIBUTES;

/// 沙箱 AppContainer profile 名（幂等；能力变更走 recreate）。
/// 坑（真机踩过）：name 含「.」会 E_INVALIDARG（0x80070057），只能字母数字-_。
pub const PROFILE_NAME: &str = "agent_local_sandbox";
/// INTERNET_CLIENT 能力 SID（出网能力）。
pub const INTERNET_CLIENT_SID: &str = "S-1-15-3-3";

#[link(name = "userenv")]
// SAFETY: 这些符号由 Windows 系统 DLL userenv.dll 提供；签名与 Windows SDK/windows-sys 的
// CreateAppContainerProfile/DeleteAppContainerProfile 一致（输出/输入均按 Win32 契约传
// NUL 结尾 UTF-16 与有效指针），调用方保证参数有效。
unsafe extern "system" {
    fn CreateAppContainerProfile(
        app_container_name: *const u16,
        display_name: *const u16,
        description: *const u16,
        capabilities: *mut SID_AND_ATTRIBUTES,
        capability_count: u32,
        out_sid: *mut PSID,
    ) -> i32;
    // 注：不用 DeriveAppContainerSidFromAppContainerName——它是纯数学运算，profile
    // 不存在也返回成功（真机踩过），所以我们一律走 recreate_profile。
    fn DeleteAppContainerProfile(app_container_name: *const u16) -> i32;
}
#[link(name = "advapi32")]
// SAFETY: ConvertStringSidToSidW 由系统 advapi32.dll 提供，签名与 windows-sys 一致：
// 输入 NUL 结尾 UTF-16 SID 字符串，输出经 LocalAlloc 分配、须 LocalFree 释放的 PSID。
unsafe extern "system" {
    fn ConvertStringSidToSidW(string_sid: *const u16, out_sid: *mut PSID) -> i32;
}
#[link(name = "kernel32")]
// SAFETY: LocalFree 由系统 kernel32.dll 提供，签名与 windows-sys 一致（HGLOBAL/HANDLE 入参，
// 成功返回 NULL）；本文件只用于释放 CreateAppContainerProfile 返回的 SID。
unsafe extern "system" {
    fn LocalFree(hmem: HANDLE) -> HANDLE;
}

/// HRESULT 成功判定。
fn ok(hr: i32) -> bool {
    hr >= 0
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 确保 AppContainer profile 存在并返回派生 SID。
/// **坑（真机踩过）**：DeriveAppContainerSidFromAppContainerName 是纯数学运算，
/// profile 不存在也返回成功——CreateProcess 会报错误 2。所以这里总是重建 profile
/// （幂等删除+创建；sandbox profile 无持久数据，代价可忽略）。
// SAFETY: 调用方前提：同一进程内不得并发重建同名 profile（否则删除/创建会交错）；
// 其余指针都在函数内部按帧存活，无需调用方再保证。
pub unsafe fn ensure_sid(allow_network: bool) -> Result<SidGuard, String> {
    // SAFETY: 契约原样透传——调用方已保证不并发重建同一 profile，参数为普通 bool。
    unsafe { recreate_profile(allow_network) }
}

/// 删除并重建 profile（能力按 allow_network），返回派生 SID。
// SAFETY: 调用方前提同 ensure_sid：不并发重建同名 profile。函数内部所有指针都指向本帧
// 存活的 Vec<u16>/SID，列表长度如实传入，且不保留任何借出指针到返回之后。
pub unsafe fn recreate_profile(allow_network: bool) -> Result<SidGuard, String> {
    let name_w = wide(PROFILE_NAME);
    // SAFETY: name_w 是本帧 NUL 结尾 UTF-16 缓冲，调用期间存活；DeleteAppContainerProfile
    // 只读该字符串，profile 不存在时返回失败，本处按幂等语义忽略。
    unsafe {
        let _ = DeleteAppContainerProfile(name_w.as_ptr());
    }

    // 坑（真机踩过）：capabilities 空数组（count=0 + 非 NULL 指针）会 E_INVALIDARG——
    // 无网络能力时必须传 NULL 数组；有网络能力时传 INTERNET_CLIENT。
    let mut cap_sids: Vec<PSID> = Vec::new();
    if allow_network {
        let s = wide(INTERNET_CLIENT_SID);
        let mut sid: PSID = ptr::null_mut();
        // SAFETY: s 是本帧 NUL 结尾 UTF-16 SID 字符串，&mut sid 是本帧有效 PSID 输出变量；
        // 失败返回 0 时不使用 sid。成功得到的 SID 仅作为 capabilities 输入指针，调用期间
        // 不释放（系统在 CreateAppContainerProfile 内复制）。
        if unsafe { ConvertStringSidToSidW(s.as_ptr(), &mut sid) } != 0 {
            cap_sids.push(sid);
        }
    }
    let mut caps: Vec<SID_AND_ATTRIBUTES> = cap_sids
        .iter()
        .map(|sid| SID_AND_ATTRIBUTES { Sid: *sid, Attributes: 0 })
        .collect();

    // 显示名/描述会出现在 Windows 的 AppContainer 列表里；PROFILE_NAME 是删除/重建用的
    // 标识符，改它等于丢掉已建 profile，故只改这两个给人看的字段。
    let display = wide("Pawch Agent Sandbox");
    let desc = wide("Pawch Agent 项目沙箱（LowBox，默认断网）");
    let mut sid: PSID = ptr::null_mut();
    let hr = unsafe {
        // SAFETY: name_w/display/desc 都是本帧 NUL 结尾 UTF-16 缓冲，调用期间存活；caps 为空
        // 时传 NULL（避免空数组的 E_INVALIDARG），否则指向 cap_sids 派生的 SID_AND_ATTRIBUTES
        // 数组且 count 用 len 如实传入；&mut sid 是本帧有效输出变量，成功后由 SidGuard 释放。
        CreateAppContainerProfile(
            name_w.as_ptr(),
            display.as_ptr(),
            desc.as_ptr(),
            if caps.is_empty() { ptr::null_mut() } else { caps.as_mut_ptr() },
            caps.len() as u32,
            &mut sid,
        )
    };
    if !ok(hr) {
        return Err(format!(
            "CreateAppContainerProfile 失败: hr=0x{hr:08X}（首次创建可能需要管理员权限）"
        ));
    }
    Ok(SidGuard { sid })
}

/// SID 句柄（Drop 时 LocalFree）。
pub struct SidGuard {
    pub sid: PSID,
}
impl Drop for SidGuard {
    fn drop(&mut self) {
        if !self.sid.is_null() {
            // SAFETY: self.sid 非空时是 CreateAppContainerProfile 分配、尚未释放的 SID；
            // Drop 只执行一次，LocalFree 接受该系列分配器返回的指针。
            unsafe {
                LocalFree(self.sid as HANDLE);
            }
        }
    }
}

/// SID 转字符串（icacls / 日志用）——手写转换，零额外 FFI。
/// SID 内存布局：[Revision u8][SubAuthorityCount u8][IdentifierAuthority 6B 大端][SubAuthority u32 × N 小端]
// SAFETY: 调用方前提：sid 要么为空（立即返回 None），要么指向系统写入的合法 SID 且在本调用
// 期间有效。函数先校验 revision==1、子权威数 ≤15，再按 SID 头声明的长度读取。
pub unsafe fn sid_to_string(sid: PSID) -> Option<String> {
    if sid.is_null() {
        return None;
    }
    // 指针解引用逐处显式 unsafe（edition 2024 要求，即使在 unsafe fn 体内）
    // SAFETY: 合法 SID 布局为 [Revision][Count][6B Authority][Count×u32]；此处先校验
    // revision==1、count ≤15 后才按该长度读取，子权威用 read_unaligned 避免对齐假设。
    unsafe {
        let p = sid as *const u8;
        if *p != 1 {
            return None; // 仅支持 SID_REVISION 1
        }
        let count = *p.add(1) as usize;
        if count > 15 {
            return None;
        }
        let mut auth: u64 = 0;
        for i in 0..6 {
            auth = (auth << 8) | *p.add(2 + i) as u64;
        }
        let mut s = format!("S-1-{}", auth);
        for i in 0..count {
            let sub = (p.add(8 + i * 4) as *const u32).read_unaligned();
            s.push_str(&format!("-{}", sub));
        }
        Some(s)
    }
}

/// 给目录授 AppContainer SID 的完全控制 ACE（icacls，幂等；递归继承 OI+CI）。
/// 失败仅返回 Err（调用方记日志，不阻断——探针会暴露权限问题）。
pub fn grant_dir_access(dir: &Path, sid_str: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("icacls")
        .arg(dir)
        .args(["/grant", &format!("*{sid_str}:(OI)(CI)F")])
        .creation_flags(0x0800_0000) // 不闪控制台窗口
        .output()
        .map_err(|e| format!("icacls 执行失败: {e}"))?;
    if !out.status.success() {
        return Err(format!("icacls 授权失败: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(())
}
