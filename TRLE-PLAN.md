# TRLE 解码器计划

当前状态：**已按 RFC 6143 第 7.7.5 节实现**（`src/codec/trle.rs`）——16x16 tile、
每 tile 一个 subencoding 字节、调色板与复用模式按 rectangle 保存。畸形输入返回错误；
单 tile 畸形测试确认不会吐出像素。9 个单元测试覆盖全部分支与畸形分支，另有 1 个
TCP loopback 集成测试覆盖客户端链路。

还没做的一件事见文末「验证状态」：在真实 MiniVNC 服务端上运行。

## 决策门：已经定下来了

**结论：走路线 A（RFC 6143 第 7.7.5 节的格式）。路线 B 作废。**

证据（2026-11-13 查得，都记了出处）：

1. **UltraVNC 1.8.x 没有 TRLE 编码器。** 本机安装目录是
   `D:\Program Files\uvnc\UltraVNC`：
   - `rfb/rfbproto.h` 里没有 type 15 的 encoding 定义，值为 15 的只有
     `rfbFileTransferSessionStart`；
   - `winvnc/winvnc/` 的编码器清单是 Tight / Zlib / ZlibHex / XZ / Ultra / Ultra2 /
     Zrle / Corre / Hext / RRE / Cursor，**没有 TRLE 编码器**；
   - `winvnc.exe`、`vncviewer.exe` 里搜到的 6 处 `trle` 全是 `lstrlenA` / `strlen`
     的子串，假阳性。
   所以「本机装 UltraVNC 抓真流」这个前提不成立。
2. **libvncserver 的 TRLE 支持是客户端解码器，不是服务端编码器。**
   `src/libvncclient/trle.c`（299 行）提供 `HandleTRLE`：
   - `include/rfb/rfbproto.h` 定义 `rfbEncodingTRLE 15`，客户端主动通告并分派到
     `HandleTRLE`；
   - 帧结构：`for (y = ry; y < ry + rh; y += 16)`、`w = h = 16` → **16x16 tile**；
     每个 tile 开头直接读 1 字节 subencoding，**没有 4 字节长度、没有 inflate**；
   - subencoding：0 原始 CPIXEL 数组、1 单色填充、2..16 调色板索引、127 复用上一
     tile 状态、128 RLE、129 复用调色板 + RLE、130..255 调色板 RLE；
   - libvncserver 的 README 支持表把 TRLE 标为 LibVNCClient 支持，LibVNCServer 不支持；
     NEWS 也明确写的是 “Added a TRLE decoder”。
3. **MiniVNC 是已确认的服务端候选。** [mac-minivnc](https://github.com/marciot/mac-minivnc)
   是面向 Macintosh Plus、Apple Lisa 等 68k/System 7 机器的 VNC 服务端，当前公开版本
   为 `v1.5-beta-april-06-2025`：
   - `VNCTypes.h` 定义 `mTRLEEncoding = 15`；
   - `VNCEncoder.cpp` 在客户端通告 type 15 且配置允许时选择 TRLE，并调用
     `VNCEncodeTRLE::encodeTile`；
   - `VNCServer.cpp` 把 `VNCEncoder::getEncoding()` 写入 framebuffer-update rectangle；
   - `VNCEncodeTRLE.cpp` 实现实际 tile 编码。
   这是源码级确认，不等于本仓库已经完成真实 MiniVNC 互操作；当前环境没有 68k/System 7
   运行环境。
顺带记一条与本仓库无关但有用的事实：UltraVNC 的 ini 里 `passwd=04569284227440C90F`
这种 18 位十六进制是 `WritePrivateProfileStruct` 写的 8 字节 DES 结果加校验字节，
DES key 是 VNC 固定的 `{23,82,107,6,35,78,88,7}`；`createpassword.exe <密码>` 只接受
**最多 8 字符**（内部 `char passwd[9]`，9 字符会触发 `strcpy_s` 终止），且它写的就是
`C:\ProgramData\UltraVNC\ultravnc.ini`。

## 路线 A：按 RFC 6143 第 7.7.5 节实现（已完成）

1. `src/config.rs` 的 `Trle = 15` 保持不动，`from_wire` 已经映射好。
2. `src/codec/trle.rs` 的 `Decoder::decode` 已按 TRLE wire format 实现：
   - 不读任何头部；tile 按 16x16 从左到右、从上到下走，右边界和下边界按余数缩小；
   - 每 tile 一个 subencoding 字节，覆盖 0 / 1 / 2..16 / 127 / 128 / 129 / 130..255；
   - palette、solid colour 和 reuse mode 在**同一个 rectangle 的 tile 之间**保持；每次
     `decode` 开始时重置，不跨 rectangle 保持；
   - CPIXEL 打包规则与 `src/codec/zrle` 一致，但 tile 循环独立实现。
3. 边界与错误处理：保留 `crate::limits::dimensions`；索引越界、截断输入、未使用的
   subencoding 和超出 tile 剩余像素的 run 都返回错误，不静默回退。
4. 测试已完成：
   - `src/codec/trle.rs` 有 9 个单元测试，覆盖全部 subencoding、tile 边界、复用状态和
     畸形输入；额外覆盖 raw / plain RLE / palette RLE 不更新 reuse state 的行为；
   - `tests/trle.rs` 通过真实 TCP loopback 验证 `SetEncodings → type 15 framebuffer
     rectangle → RawImage` 的客户端链路；
   - libvncserver 的 `HandleTRLE` 作为独立预言机，147 个用例完成逐像素交叉验证。
5. 没有「恢复通告」这一步：`VncConnector::build` 要求编码列表非空，库从不默认通告
   任何编码，只有调用方显式 `add_encoding(VncEncoding::Trle)` 时 type 15 才会上线。

## 路线 B：UltraVNC 抓包（作废）

原本设想「UltraVNC 的 TRLE 是 zlib 变体，本机装 server 抓包确认」。当前 UltraVNC
既没有 type 15 的定义也没有编码器，所以这条路线作废。已确认的真实服务端候选是
[MiniVNC](https://github.com/marciot/mac-minivnc)，但它面向 68k/System 7 Macintosh，
当前环境没有可运行的目标系统。

## 明确不做

- 不做静默回退：任何畸形输入（未用的 subencoding、越界索引、超长 run、截断的
  调色板或像素）都必须返回错误。单 tile 畸形测试断言不会吐出像素；多 tile rectangle
  若前面的 tile 已经发出，后续 tile 出错时不回滚此前事件。
- 对外描述保留「尚未在真实 MiniVNC 上验证」这句，不把 libvncserver 预言机或 TCP
  loopback 测试写成真实服务器互操作。
- 不要动现有的 indexed 像素位打包那段：里面 `shift` 推断出来是 `i32`，行边界的重新
  同步逻辑我单独写探针验证过，它不是这次出错的原因。

## 验证状态

- **已实现**：`src/codec/trle.rs` 全部分支（0 / 1 / 2..16 / 127 / 128 / 129 /
  130..255），9 个单元测试通过；另有 1 个 TCP loopback integration test；`cargo fmt`、
  clippy、`cargo test` 全绿（预计 59 unit / 7 integration / 2 doc）。
- **已交叉验证**（2026-11-13，WSL Ubuntu + gcc）：把 libvncserver 的 `trle.c` 原样
  编成预言机（`BPP 32` / `REALBPP 32`，CPIXEL 4 字节），另写一个独立的 Python TRLE
  编码器产出 147 个用例（139 正例 + 8 反例），覆盖全部 subencoding、16x16 边界裁剪、
  跨 tile 复用、`0xff` 续长 run、以及到 64x64 的多 tile 矩形。三方逐像素对拍：
  - 本仓库解码器 vs 编码器真值：**147/147**
  - 本仓库解码器 vs libvncserver：**146/147**
  - libvncserver vs 编码器真值：146/147

  唯一分歧是 `run-longer-than-tile`，即上面的有意加固，不是缺陷。8 个反例本仓库全部
  正确拒绝。
- **已做客户端链路验证**：`tests/trle.rs` 在 loopback TCP 上完成握手、通告 type 15、
  发送 TRLE rectangle 并收到 `RawImage`。
- **尚未真实上机**：MiniVNC 源码已确认会把 15 写入 framebuffer-update rectangle，但
  尚未在 68k/System 7 环境启动它；没有把这项源码确认冒充成真实流验证。

### 复现方法

脚本都在 `C:\Users\michael\AppData\Local\Temp\trle_probe`（仓库外）：

1. `python gen_corpus.py cases.json cases.tsv` —— 独立编码器产出字节流与真值像素。
2. `gcc -O2 -o oracle oracle.c`（在 WSL 里，`oracle.c` 直接 `#include "ref_trle.c"`，
   后者是从 libvncserver 取来的未改动源码）。
3. `bash run_oracle.sh` —— 预言机跑完 147 个用例，输出 `oracle_out.tsv`。
4. 仓库内临时挂一个 `#[cfg(test)] mod trle_crosscheck`，读 `TRLE_CORPUS` 指向的
   TSV、按 tile 坐标组装帧缓冲、把结果写到 `TRLE_OUT`；跑完即撤掉，不进提交。
5. `python compare.py` —— 三方对拍并列出分歧。

两个踩过的坑记在这里，免得下次重犯：调色板 RLE 的 subencoding 是 `128 + k`（不是
`130 + k`，参考实现按 `type - 128` 取调色板长度）；多 tile 矩形必须按 tile 坐标写
进帧缓冲再比对，顺序拼接各 tile 的像素会得到不同的排列。

## 与参考实现的有意差异

1. run length 受 tile 剩余像素限制，超出即报错；libvncserver 是停止填充并打一条
   `possible TRLE corruption` 警告，然后继续读下一个 tile。
2. 129 在没有调色板时返回错误；参考实现会拿未初始化的 `palette[]` 继续用。
3. 其余（tile 尺寸、subencoding 表、127 的 `& 0x7f` 换算、case 0 与 128/129 都不
   更新 `last_type`）与参考实现一致。

## 现状备忘

- 服务器没被要求就不可能偷偷塞一个 TRLE rectangle 进来：
  `src/client/connection.rs` 会拒掉任何不在通告列表里的编码，Raw 除外。
- fuzz 目标仍然会请求 `VncEncoding::Trle`（`fuzz/fuzz_targets/protocol.rs`），现在
  打到的是真解码路径。
- README 的 Encodings 一节已经改成「按 §7.7.5 重写、有单元测试、未经真机验证」。
- 一次性探针留在 `C:\Users\michael\AppData\Local\Temp\trle_probe`（仓库外，只通告
  type 15、不发任何键鼠事件）；抓包文件已删，里面只有一段失败的认证握手、没有画面。
- 本机 UltraVNC 未做任何改动：ini 未写、winvnc 未重启、密码未设。
