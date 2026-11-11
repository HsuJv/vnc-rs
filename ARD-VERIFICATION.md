# ARD 认证真机验证指南

验证目标：本仓库新增的 Apple ARD 认证（RFB security type 30）能否连接真实的
macOS 屏幕共享服务，并绕过 macOS 10.7+ 普通 VNC 认证弹出的登录窗口。

协议逻辑已由模拟服务器测试覆盖（`cargo test`），本文档只做真机互操作验证。

## 1. 准备验证机（你将要换过去的那台机器）

要求：Linux，与 MacBook 在同一局域网，能访问 Mac 的 5900 端口。

```bash
# 安装 Rust（已装可跳过）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# 拉取代码（ARD 改动已在 main 分支）
git clone https://github.com/michael-eddy/vnc-rs.git
cd vnc-rs

#  sanity check：模拟服务器的单元/集成测试应全部通过
cargo test
```

`cargo test` 全绿说明代码和环境没问题，再往下做真机部分。

## 2. 设置 MacBook（被控端）

1. 打开屏幕共享：
   - macOS 13 及以后：系统设置 → 通用 → 共享 → 打开「屏幕共享」
   - 旧版 macOS：系统偏好设置 → 共享 → 勾选「屏幕共享」
2. **不要勾**「VNC 显示程序可以使用密码控制屏幕」。
   勾了服务器会提供 type 2（普通 VNC 认证）；不勾才只提供 type 30（ARD），
   这正是本次要验证的场景。
3. 确认允许访问的用户：屏幕共享旁的 ⓘ → 允许访问选「所有用户」或包含你要用的账号
   （管理员账号默认允许）。
4. 记下 Mac 的局域网 IP（共享设置页会显示 `vnc://192.168.x.x`，或
   `ifconfig | grep "inet "` 查看）。
5. 防火墙：系统设置 → 网络 → 防火墙，确认放行屏幕共享（或验证期间暂时关闭）。
6. Mac 保持开机唤醒状态（系统睡眠会断开连接；显示器睡眠不影响）。

## 3. 验证步骤

以下命令都在验证机的仓库根目录执行。首次运行会编译依赖，需要几分钟。

### 3.1 探测服务器提供的安全类型

```bash
cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip>
```

预期输出：

```text
server version: "RFB 003.889\n"
apple server:   true
offered security types: 30 (Apple ARD)
=> type 30 (Apple ARD) is available; retry with --connect <username> <password>
```

看到 `003.889` 和 type 30 才继续下一步。异常处理见第 4 节。

### 3.2 正确密码：完整 ARD 连接（核心验证）

```bash
cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip> --connect <Mac用户名> <密码>
```

预期输出：

```text
ARD authentication succeeded in ..., desktop name: "..."
resolution: ...
first frame: rect ..., ... bytes
success: ARD login bypassed the login window and got a frame in ...
```

拿到首帧 = ARD 登录成功且画面通道正常，核心目标达成。

### 3.3 错误密码：验证失败路径

```bash
cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip> --connect <Mac用户名> wrongpass
```

预期：`ARD authentication failed: <服务器返回的原因>`，进程以错误退出。
不应卡死，也不应误报成功。

### 3.4 锁屏 / 登录窗口状态

把 Mac 锁屏（Control+Command+Q）或注销到登录窗口，重复 3.2。
预期：仍然成功。不需要任何人先登录进桌面——这是 ARD 相对普通 VNC 认证的意义。

### 3.5（可选）回退路径

在 Mac 上勾上「VNC 显示程序可以使用密码控制屏幕」并设一个 VNC 密码，然后：

- 重复 3.1：应同时看到 `2 (VNC auth)` 和 `30 (Apple ARD)`；
- 重复 3.2：应仍优先选 30 并成功（输出同 3.2）。

验证完记得把该选项勾回去掉，恢复原状。

## 4. 异常对照表

| 现象 | 可能原因 | 处理 |
| --- | --- | --- |
| 3.1 连接被拒绝 / 超时 | 屏幕共享未开、防火墙拦截、IP 错误、不在同一网段 | 检查第 2 节；`ping <mac-ip>` 确认连通 |
| 版本串不是 `003.889` | 对端不是 macOS 自带屏幕共享（可能是第三方 VNC 服务器） | 确认连的是 Mac 本机 5900 |
| 只提供 `2 (VNC auth)` | 勾了「VNC 显示程序可以使用密码控制屏幕」 | 去掉该勾选后重试 3.1 |
| 提示 does not offer ARD | 同上，或服务器确实不提供 30 | 把 3.1 完整输出记录下来反馈 |
| 3.2 报 `ArdAuthFailed` 但密码确认正确 | 账号无屏幕共享权限，或协议互操作问题 | 加 `-vv` 重跑并记录输出反馈 |
| 认证成功但 10 秒等不到首帧 | 认证后的初始化/编码协商问题 | 加 `-vv` 重跑并记录输出反馈 |

## 5. 失败时反馈这些内容

1. macOS 版本（关于本机）；
2. 3.1 的完整输出；
3. 失败步骤加 `-vv` 的完整输出，例如：

   ```bash
   cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip> --connect <用户名> <密码> -vv
   ```

注意：`-vv` 的 trace 日志可能包含敏感信息，反馈前自行脱敏；密码不要写进反馈里。

## 6. 结果记录

| 步骤 | 结果（成功/失败） | 备注 |
| --- | --- | --- |
| 3.1 探测 | | |
| 3.2 正确密码 | | |
| 3.3 错误密码 | | |
| 3.4 锁屏状态 | | |
| 3.5 回退（可选） | | |