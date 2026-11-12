# TRLE 解码器计划

当前状态：**已按 RFC 6143 第 7.7.5 节实现**（`src/codec/trle.rs`）——16x16 tile、
每 tile 一个 subencoding 字节、调色板与复用模式按 rectangle 保存，畸形输入一律返回
错误且不吐像素。8 个单元测试覆盖全部分支与畸形分支。

还没做的两件事见文末「验证状态」。

## 决策门：已经定下来了

**结论：走路线 A（RFC 6143 第 7.7.5 节的格式）。路线 B 作废。**

证据（2026-11-12 查得，都记了出处）：

1. **UltraVNC 现在根本没有 TRLE。** 本机装的是 1.8.x（`D:\Program Files\uvnc\UltraVNC`）：
   - `rfb/rfbproto.h` 里没有任何 type 15 的 encoding 定义，值为 15 的只有
     `rfbFileTransferSessionStart`；
   - `winvnc/winvnc/` 的编码器清单是 Tight / Zlib / ZlibHex / XZ / Ultra / Ultra2 /
     Zrle / Corre / Hext / RRE / Cursor，**没有 TRLE 编码器**；
   - `winvnc.exe`、`vncviewer.exe` 里搜到的 6 处 `trle` 全是 `lstrlenA` / `strlen`
     的子串，假阳性。
   所以「本机装 UltraVNC 抓真流」这个前提不成立，路线 B 没有对端可抓。
2. **libvncserver 有完整实现，且格式与 RFC 一致。** `src/libvncclient/trle.c`
   （299 行）：
   - `include/rfb/rfbproto.h`：`#define rfbEncodingTRLE 15`，与我们 `config.rs` 的
     `Trle = 15` 一致；
   - `rfbclient.c:1323` 客户端**主动通告** type 15，`:2419` 分派到 `HandleTRLE`，
     说明现实里确实有服务器发这种编码，不是纯历史包袱；
   - 帧结构：`for (y = ry; y < ry + rh; y += 16)`、`w = h = 16` → **16x16 tile**；
     每个 tile 开头 `ReadFromRFBServer(&type, 1)` 直接读 1 字节 subencoding，
     **没有 4 字节长度、没有 inflate**；
   - subencoding：0 原始 CPIXEL 数组、1 单色填充、2..16 调色板索引、127 复用上一
     tile 状态、128 RLE、129 复用调色板 + RLE、130..255 调色板 RLE
     （`type - 128` 项调色板）；`CARDBPP palette[128]`；
   - 一个必须照抄的细节：`case 0` 结尾写的是 `type = last_type;`（原始 tile **不更新**
     `last_type`），127 / 129 的「复用」语义依赖这个怪癖。

顺带记一条与本仓库无关但有用的事实：UltraVNC 的 ini 里 `passwd=04569284227440C90F`
这种 18 位十六进制是 `WritePrivateProfileStruct` 写的 8 字节 DES 结果加校验字节，
DES key 是 VNC 固定的 `{23,82,107,6,35,78,88,7}`；`createpassword.exe <密码>` 只接受
**最多 8 字符**（内部 `char passwd[9]`，9 字符会触发 `strcpy_s` 终止），且它写的就是
`C:\ProgramData\UltraVNC\ultravnc.ini`。

## 路线 A：按 RFC 6143 第 7.7.5 节实现

1. `src/config.rs` 的 `Trle = 15` 保持不动，`from_wire` 已经映射好。
2. 重写 `src/codec/trle.rs` 的 `Decoder::decode`：
   - 不读任何头部；tile 按 16x16 从左到右、从上到下走，右边界和下边界按余数缩小；
   - 每 tile 一个 subencoding 字节，按上面那张表分支；
   - `Decoder` 结构体里要存 `last_type`、调色板、上一个 RLE 像素，让 127 / 129 跨
     tile、跨 rectangle 生效（现在每 tile 无条件 `palette.clear()` 是错的根源之一）；
   - CPIXEL 打包可以直接用 `src/codec/zrle` 里的规则，但 tile 循环不能共用。
3. 边界：保留 `crate::limits::dimensions`；调色板限制在 128 项，索引越界必须报错；
   tile 声明的像素数超过 rectangle 剩余量时报错而不是无限读。
4. 测试，写在 `src/codec/trle.rs`：
   - 测试内部自己写一个编码器，每种 subencoding 出一个 rectangle，断言像素逐字节
     正确，包含 127 / 129 两个复用场景和 `case 0` 不更新 `last_type` 这条；
   - 畸形输入（超长 run length、索引越界、tile 像素数超矩形）断言返回错误而非 panic；
   - 保留 run length 与调色板索引的越界测试。
5. 验收标准（可执行，不靠自证）：把 libvncserver 的 `HandleTRLE` 当**预言机**——用
   我们的测试编码器产出字节流，喂给编译进来的 libvncserver 客户端解码路径，比对两
   边像素一致。这比「找一台真服务器」现实，因为本机已经没有能发 TRLE 的服务器了。
6. 没有「恢复通告」这一步：`VncConnector::build` 要求编码列表非空，库从不默认通告
   任何编码，只有调用方显式 `add_encoding(VncEncoding::Trle)` 时 type 15 才会上线。

工作量：3 到 5 小时（比原估的多，因为预言机那套交叉验证要搭起来）。

## 路线 B：作废，保留作背景

原本设想「UltraVNC 的 TRLE 是 zlib 变体，本机装 server 抓包确认」。前提已被源码否
掉：当前 UltraVNC 既没有 type 15 的定义也没有编码器。若以后要重启这条线，需要先找
到仍发 TRLE 的服务器实现，而不是靠 UltraVNC。

## 明确不做

- 不做静默回退：任何畸形输入（未用的 subencoding、越界索引、超长 run、截断的
  调色板或像素）都必须返回错误，且一个像素都不吐出来。测试里对每条都断言了
  `pixels.is_empty()`。
- 交叉验证已完成，但仍然没有真机对端；对外描述保留「未经真机验证」这句，不要写成
  已在真实服务器上验证过。
- 不要动现有的 indexed 像素位打包那段：里面 `shift` 推断出来是 `i32`，行边界的重新
  同步逻辑我单独写探针验证过，它不是这次出错的原因。

## 验证状态

- **已实现**：`src/codec/trle.rs` 全部分支（0 / 1 / 2..16 / 127 / 128 / 129 /
  130..255），8 个单元测试通过；`cargo fmt`、三条 clippy、`cargo test` 全绿
  （58 unit / 6 integration / 2 doc）。
- **已交叉验证**（2026-11-13，WSL Ubuntu + gcc）：把 libvncserver 的 `trle.c` 原样
  编成预言机（`BPP 32` / `REALBPP 32`，CPIXEL 4 字节），另写一个独立的 Python TRLE
  编码器产出 147 个用例（139 正例 + 8 反例），覆盖全部 subencoding、16x16 边界裁剪、
  跨 tile 复用、`0xff` 续长 run、以及到 64x64 的多 tile 矩形。三方逐像素对拍：
  - 本仓库解码器 vs 编码器真值：**147/147**
  - 本仓库解码器 vs libvncserver：**146/147**
  - libvncserver vs 编码器真值：146/147

  唯一分歧是 `run-longer-than-tile`，即下面第 1 条有意加固，不是缺陷。8 个反例本
  仓库全部正确拒绝。
- **未上机**：本机没有任何会发 type 15 的服务器（UltraVNC 已确认不带 TRLE，macOS
  与 TigerVNC/RealVNC 也不发），真流验证一条都没做过。

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
