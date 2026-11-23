//! EmulatorController gRPC 客户端类型。
//!
//! proto vendor 在 `src/android/proto/`（Apache-2.0，取自 Android Emulator
//! 37.3.10 的 `emulator/lib/emulator_controller.proto`），build.rs 用
//! protoc-bin-vendored 编译，不要求构建机装 protoc。

#[allow(clippy::all)]
pub mod control {
    // tonic 0.14：include_proto 宏仍在 tonic 本体；prost 集成代码由 tonic-prost-build 生成。
    // 生成代码里的 doc 注释格式不归我们管，模块级 allow 把 clippy 压平。
    tonic::include_proto!("android.emulation.control");
}
