# Security Policy

## Supported Versions

当前处于原型阶段，仅维护最新主干（`master`）。

| Version | Supported |
|---|---|
| master | ✅ |
| < 0.1.0 | ❌ |

## Reporting a Vulnerability

请通过 **GitHub Issues** 报告（标题加 `[Security]` 前缀），并尽量包含：

1. 触发环境（Windows x64 / 通达信版本 / Rust 与 CMake 版本）；
2. 复现步骤与输出（控制台 / `bridge_dll/logs/tdx_czsc_bridge.log` /
   `tdx_czsc_service.log`）；
3. 影响评估（如涉及崩溃、服务不可用、数据泄露）。

安全相关修复会在下一个里程碑版本中优先处理。

## 安全注意事项（贡献者）

- 本仓库 `.gitignore` 已排除 `env` / `.env` 与全部日志；**请勿提交任何凭证、
  代理地址或通达信私有配置**；
- 服务与 DLL 仅面向本机运行，Named Pipe 管道名不可被任意进程伪造访问
  （默认 `\\.\pipe\tdx_czsc_analyzer`）；部署环境请勿暴露给不可信进程；
- 报告漏洞时请勿包含真实交易账户或个人信息。