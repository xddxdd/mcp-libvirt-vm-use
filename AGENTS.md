# AGENTS.md — mcp-libvirt 项目说明

Rust 实现的 MCP (Model Context Protocol) server：通过 libvirt 管理虚拟机，通过 SPICE 协议对 VM 执行屏幕截图与键鼠控制。

## 架构与目录结构

```
src/
├── main.rs          tokio 入口：构建 LibvirtTools，经 rmcp stdio serve
├── mcp.rs           rmcp glue：工具路由、参数结构体、结果辅助函数
├── libvirt.rs       libvirt 封装（virt crate FFI，不调用 virsh 子进程）
├── tools.rs         8 个 MCP tools（list_domains / screenshot / type_text /
│                    key_press / mouse_move / mouse_click / mouse_scroll / mouse_drag）
└── spice/
    ├── mod.rs       SpiceSession 异步适配层：shakenfist-spice-renderer 的
    │                run_connection 会话编排 + SurfaceMirror 帧捕获 → PNG
    └── inputs.rs    键名→扫描码表 + 组合键解析（表经 spice-html5 校验移植）

docs/                SPICE 协议权威参考（spice.proto、spice-protocol 头文件、
                     spice-html5 参考实现；手写协议客户端已被 ryll crates 取代）
PLAN.md              约束性契约：模块 API 签名、crate 选型、迁移决策
flake.nix            flake-parts devShell（rustc / cargo / pkg-config / libvirt）
```


## 构建与测试（NixOS）

`virt` crate 需要 libvirt C 头文件与库，所有 cargo 命令必须在 devShell 内执行：

```bash
nix develop -c cargo build
nix develop -c cargo test
nix develop -c cargo run
```

宿主机 rustup 的 ld shim 损坏时，裸 cargo 链接需 `RUSTFLAGS="-C link-arg=-fuse-ld=bfd"`；devShell 内的 nixpkgs rustc 无此问题。

## 开发规则

- **PLAN.md 是契约**：模块公开 API、SPICE wire 语义、依赖选型都以它为准。修改模块 API 前先更新 PLAN.md。
- **SPICE 协议数值**以 `docs/spice-protocol/enums.h` 为唯一权威（消息 ID、枚举值）。
- **Cargo.toml 由项目所有者统一管理**；依赖选型决策（官方 `rmcp` crate 做 MCP、`virt` crate 做 libvirt、手写 SPICE 客户端）记录在 PLAN.md。
- 每次操作（截图/输入）新建一个 SPICE 会话：一个 `run_connection` tokio 任务编排全部通道，事件经 mpsc 应用到 SurfaceMirror；Drop 时置 cancel 标志断开。
- SPICE 协议实现来自 shakenfist-spice-renderer 0.1.7（ryll，Apache-2.0）：auth（RSA-OAEP SHA1 ticket）、display/cursor/inputs 等通道、QUIC/GLZ/LZ/JPEG/H264 解码均由 crate 处理；压缩偏好由 renderer 选择（AUTO_GLZ），不再请求 RAW 位图。
- 已知取舍：unix-socket SPICE 端点不再支持（ryll 仅 TCP/TLS）。