# 项目:本地优先的日历 + 番茄钟桌面应用

## 是什么

一个离线可用的桌面应用,包含两个交互上相互独立的功能:

1. **周日历** — 自由拖拽创建/移动时间块(参考 Notion Calendar)
2. **番茄钟** — 工作时长与休息时长可自定义

两者共享底层的时间记录,因此可以并排展示「计划 vs 实际」。但用户可以只用其中任何一个,
彼此不存在使用上的依赖。

## 硬约束

- SQLite 仅作为**可重建的索引**,不是真相来源(目前尚未引入依赖)。
- 不要引入后端服务、账号系统、云同步。**数据永远不出这台机器**;唯一允许的出站请求是
  用户自己点托盘里「Check for Updates…」触发的那一次,它不带任何本机信息(见「自动更新」)。

## 当前进度

不变量 #1、#2、#3 都已实现并被测试锁定。**#4 尚无任何代码**(`Cargo.toml` 里没有
`notify`),那一条目前仍是唯一的规格来源,不要当成已完成的描述来读。

周日历已可用:`src-tauri/src/calendar/` 读写 `calendar/YYYY-MM.ics`,前端
`WeekCalendar.vue` 做拖拽创建/移动/改时长。番茄钟与托盘、通知、全局快捷键、开机自启
都已接好。

「计划 vs 实际」已经打通:`src-tauri/src/sessions.rs` 把每一个结束的番茄钟写进
`sessions/YYYY-MM-DD.jsonl`,`Timer` 记住段落的墙钟起点并产出 `Ended`(completed /
aborted / invalidated),`integrations::pump` 落盘后发 `sessions://recorded`。周视图
每一列左半边是 `.ics` 的计划块、右半边是 session,顶部一条汇总(计划总时长、实际
专注总时长、完成率)。session 是只读的,右半边不接任何手势。

CLI(`calpo`)三条命令都已实现:`calpo today`、`calpo log --week [N]`、
`calpo start [LABEL] [--for 25m]`。`--vault <PATH>` 与 `CALENPOMO_VAULT` 只对单次调用
生效,永远不回写 `settings.toml`。

`calpo start` 走两条路,选哪条不是用户要操心的事:GUI 在跑时,经本地 socket
(`src-tauri/src/ipc.rs`)请 GUI 代跑,GUI 侧入口是 `commands::handle_ipc`;没有 GUI 时
CLI 自己跑一个 `Timer::ephemeral`,前台画倒计时,Ctrl-C 记为 aborted。session 的
`label` 字段至此才第一次有人写。**已知未闭合的缝**:CLI 正在前台跑时用户去开 GUI,
两边会各跑各的计时;补这个缺口要靠不变量 #4(文件监听),那一条目前仍无代码。

CI 与发布已接上:`.github/workflows/ci.yml` 在三个 OS 上跑两套 feature 的
clippy/test,`release.yml` 由 `v*` tag 触发,产出 dmg / AppImage / NSIS 加一个单独编的
`calpo`,挂成**草稿** release,并拼出 `latest.json`。托盘里的「Check for Updates…」
已经接上 `tauri-plugin-updater`。`install.sh`(第 3 项)与 README(第 5 项)还没有。

**自动更新尚未端到端验证过**:那需要两个已发布的 release(装着旧的去收新的),
而现在一个 tag 都还没打。已验证的只到「产物签名与 `latest.json` 正确生成」。

## 核心架构不变量(不得违反)

### 1. 文件是唯一真相,`.index/` 是可弃缓存

所有 vault 内路径必须经由 `Vault` 的方法取得(`vault/layout.rs` 的 `calendar_file` /
`sessions_file` / `config_path` / `index_dir` / `lock_path`),禁止在别处拼接路径字符串。
写入顺序永远是「先落 `calendar/`、`sessions/` 下的文件,后更新 `.index/`」。

写锁文件 `.index/write.lock` 也归这一条管:它没有内容,删掉不丢任何东西,取锁时按需
重建。放 `.index/` 而不是配置目录,是因为锁保护的是「这些文件」而不是「这台机器」。

**验收标准:删除整个 `.index/` 目录并重启,所有数据必须完好无损地重建。**
新增任何索引后,`vault::tests::deleting_the_index_directory_is_repaired_without_touching_user_data`
必须仍然通过。

### 2. 计时归 Rust,前端只显示

`src-tauri/src/timer.rs` 同时持有 `Instant`(单调,防改系统时间/NTP 校正)
和 `SystemTime`(墙钟),每次查询比对两者增量。`Instant` 在休眠期间是否继续走各平台不一致
(Linux 的 `CLOCK_MONOTONIC` 不计休眠),所以**必须靠墙钟差值检测休眠**:墙钟跳跃远大于
单调增量即判定该番茄钟作废并提示用户,不要若无其事地继续倒计时。

前端禁止用 `setInterval` 累加计数,只能 `invoke('timer_state')` 每秒拉取计算结果;
composable 放 `src/composables/useTimer.ts`。计时状态需持久化,冷启动能恢复现场。
CLI 的前台倒计时同样只是 `Timer` 的显示器:每 200ms 调一次 `observe()` 取结果,
自己不累加任何东西。

`StartOptions` 的三个字段(label / 一次性时长 / 强制 phase)**只属于当前这一段**,
生命周期与 `started_at` 完全一致,统一由 `clear_segment()` 抹掉。漏抹是静默错误 ——
下一段会莫名带着上一段的标题、跑上一段的长度。一次性时长绝不许改 `set_durations`,
否则 `calpo start x --50m` 会把用户 app 里的工作时长永久改掉。这三个字段也进
`timer.json`:重启后恢复出来的必须是**同一段**,而不是一段长度被换掉的同名番茄钟。

### 3. 用户数据写入必须经过 `fs_atomic`

所有写入必须调用 `fs_atomic::atomic_write` 或 `fs_atomic::append_jsonl`,所有 JSONL 读取
必须调用 `fs_atomic::read_jsonl`。禁止在 `fs_atomic.rs` 之外出现 `fs::write`、
`File::create`、`OpenOptions`,也禁止自己按 `\n` 切分。
(例外:只读的 `fs::read_to_string`,如 `settings.rs`、`vault/mod.rs` 读配置,
`calendar/store.rs` 读 `.ics`;只读的 `fs::read_dir`,如 `calendar/store.rs` 列月份文件;
以及测试代码。)

跨进程互斥同样归这个模块:`fs_atomic::lock_exclusive` 是唯一持有锁文件句柄的地方,
所以「`OpenOptions` 不出 `fs_atomic.rs`」这条不需要开新例外。调用方拿的是
`Vault::lock()`(等 `LOCK_WAIT`)或 `Vault::try_lock()`(试一次就走)。
**锁按「逻辑操作」持有,不是按单次 `atomic_write`** —— 跨月移动事件是两次写,
读者绝不能撞进中间那个「两个分片里都有」的窗口。

### 4. 文件监听不得自激 —— 尚未实现

实现时用 `notify` crate,事件 debounce 约 300ms。注意:

- 很多编辑器保存文件的方式是「写临时文件 + rename」,收到的是 delete + create
  而不是 modify,要能正确识别为修改。
- 本应用自身写入触发的事件必须被抑制,否则会无限循环。抑制逻辑要覆盖
  `fs_atomic::temp_path` 生成的 `.<name>.tmp-*` 文件。

## 数据格式

**日历** — 标准 iCalendar(`.ics`),用 `icalendar` crate(`recurrence` feature,
底层即 `rrule`)解析与序列化。不要手写解析器,不要自己实现 RFC 5545。
这样用户可以直接把文件导入 Google Calendar 或 Apple 日历。

**番茄钟记录** — JSONL,一行一条(`sessions::Session`,盘上是 snake_case;发给前端的
`SessionView` 是 camelCase,两者故意分开):

```json
{"id":"...","kind":"work","planned_sec":1500,"actual_sec":1500,"started_at":"2026-07-23T14:00:00+09:00","ended_at":"2026-07-23T14:25:00+09:00","outcome":"completed","label":"写论文"}
```

`outcome` 取值:`completed` | `aborted` | `invalidated`(休眠导致)。
时间戳一律带时区偏移量的 RFC 3339 格式。`label` 只有 `calpo start "写论文"` 会写
(app 界面上没有能打字的地方),其余行一律是 `null` —— 是写出来而不是省略,这样手翻文件
时每一行形状一样。

## 实现期决策(代码里已定,改动前先读)

**JSONL 语义** — 换行符是提交标记,不是「能否解析」:`read_jsonl` 无条件丢弃最后一个分片,
即使末行是合法 JSON 但缺 `\n` 也算未提交(测试 `a_truncated_record_that_parses_is_still_discarded`)。
写路径**永不销毁字节**:遇到半行时 `append_jsonl` 补一个 `\n` 隔开,残片原样留在盘上。
坏行分三类:`records` / `corrupt`(带 1-based 行号)/ `truncated_tail`;空白行静默跳过,
不计入 `corrupt`;容忍 CRLF。`JsonlRead` 手写 `Default` 是为了不给 `T` 强加 `Default` 约束。

**落盘** — macOS 上 `sync_all` 只把数据交给盘的写缓存,因此额外调 `F_FULLFSYNC`
(target-specific 的 `libc` 依赖),返回值**故意忽略**——网络挂载会拒绝。
临时文件名 `.{name}.tmp-{pid}-{nanos}-{seq}`,带点前缀且**故意不以 `.ics`/`.jsonl` 结尾**,
这样按扩展名扫目录的代码永远不会把崩溃残留当成 vault 数据。
`stage` 返回即关闭句柄,因为 Windows 不允许 rename 打开中的文件;`sync_parent_dir` 在非 unix 上是空实现。

**日历分片与重复事件** — 事件存在 DTSTART **本地时间**所属月份的 `.ics` 里,
所以 `YearMonth::of` 要传时区(`Calendars` 因此对 `TimeZone` 泛型,测试传固定偏移,
生产传 `Local`)。跨月移动**先写目标文件再重写源文件**:崩在中间是「重复」而不是「丢失」。
消解重复用 RFC 5545 自己的办法 `SEQUENCE` → `LAST-MODIFIED` → `DTSTAMP`(`ics::revision`),
**不能**用「哪份待在自己该在的分片里」——残留那份带的是旧 DTSTART,一样名正言顺。
`update` 就地改 VEVENT 而非重建,否则手写的 DESCRIPTION / VALARM / X- 属性会被抹掉。
写只产出 UTC(`...Z`)形式,读四种形式全收。RRULE 只展开不编辑
(`AppError::RecurringNotEditable`),要改规则请直接编辑 `.ics`。
`range` 会扫所有 ≤ 窗口末月的分片,因为重复事件的 DTSTART 可能在很早的月份;
文件小,先不给 `.index/` 加东西。

**Vault 与错误** — `Vault::open` 只补建骨架,**绝不创建根目录**:外置盘没挂载时若自动新建,
用户看到的就是「数据没了」。配置解析失败是硬错误,`Vault::open` 末尾那次 `read_config()`
纯粹为了失败时报错(看着像废代码,是故意的),`Settings::load` 同样立场。
`AppError` 是单一扁平枚举,不分模块错误类型;手写 `Serialize` 把错误压成字符串给前端;
`IoResultExt::at` 强制每个 io 错误带上路径。
`set_vault` 必须 `Vault::open` 成功后才 `Settings::save`,否则下次启动会卡在一个打不开的路径上。
`VaultStatus` 是三态 tagged enum,`missing` 是正常状态而非错误,前端手工镜像在 `src/types/vault.ts`。

**写锁与 CLI** — 锁用 `std::fs::File::{lock, try_lock, unlock}`(Rust 1.89 起进标准库,
所以 `rust-version = "1.89"`),**不引 `fs4`/`fs2`**。锁是 advisory 的:它只约束本项目
自己的进程,对文本编辑器无效 —— 这是有意的,这些文件本来就该能手改。
GUI 的 ticker 用 `try_lock`(等 0 秒):`drain_sessions` 每秒跑一次,为了等 CLI 而卡住
就等于停表,抢不到就把记录留在队列里下一秒再来。交互式写入用 `lock()` 等 5 秒。
注意 unix 上 flock 归「打开的文件描述」所有,同进程再取一次同样会互斥,所以这两个
方法**不可嵌套**;GUI 侧靠自己的 vault Mutex 串行化。

CLI 与 GUI 共用一个 crate,靠默认开启的 `gui` feature 分开:`gui` 关掉后
tauri / tauri-build / 四个插件全部不进依赖树,`calpo` 因此不需要 webkit2gtk 之类的系统
依赖。新代码放 `vault` / `calendar` / `sessions` / `timer` / `fs_atomic` / `cli` 时,
**不许让它们长出对 tauri 的依赖**。`cli/report.rs` 的汇总口径是 `src/lib/summary.ts`
的镜像(计划块按窗口裁剪、session 按起点整取、all-day 不计、break 与 invalidated 不算
focus),改一边必须改另一边。CLI 输出刻意全 ASCII 且把变宽的用户文本放在每行最后:
`–` `·` `—` 都是 East Asian Ambiguous,CJK 终端下会画成双宽而把表格拉歪。
`config_dir()` 手工复刻 Tauri 的 `app_config_dir()`(即 `dirs::config_dir()/${identifier}`),
`APP_IDENTIFIER` 必须和 `tauri.conf.json` 的 `identifier` 保持一致。
一个包两个 bin,所以 `Cargo.toml` 里有 `default-run = "calenpomo"` —— `npm run tauri dev`
跑的正是裸 `cargo run`,没有这一行会直接报「could not determine which binary to run」。

**本地 IPC** — `src-tauri/src/ipc.rs`,unix 是 `config_dir/cli.sock`,Windows 是命名管道。
用 `interprocess` crate(std 没有命名管道);它只是 socket,不开端口、不解析主机名,
和「永不联网」不冲突。**写锁解决不了这一半**:GUI 在跑时计时器在它内存里、每 10 秒重写
`timer.json`,第二个进程自己起的倒计时会被直接覆盖掉,所以唯一的办法是请 GUI 代跑。

- 一次连接一条请求一条回复,各一行 JSON,然后关闭。两端同属一个 crate,所以字段用
  snake_case,不必迁就前端的 camelCase。
- **连不上和被拒绝是两回事**:`send()` 只把 `NotFound`/`ConnectionRefused` 翻成
  `Ok(None)`(「没开 app」,可以自己跑),其余都是错误。GUI 那边即使读不懂请求也必须回一句
  `Refused` —— 沉默会被新版 CLI 读成「没开 app」,于是并排跑起第二个计时器。
- 崩溃残留的 socket 文件靠「先 bind,`AddrInUse` 就试着 connect 一下」区分:有人应答说明
  真有第二个 app 在跑,这一个就不抢;没人应答才是残骸,覆盖掉。
- Windows 命名管道是全机器一个命名空间,所以名字里拼了 config_dir 的哈希 ——
  否则两个账号同时登录会抢同一个管道。
- socket 的保护就是 config 目录自身的权限(`interprocess` 在 macOS 上设不了 socket mode)。
- GUI 侧 `serve_cli` 失败只打一行 stderr,不进 `IntegrationStatus`:那个面板讲的是
  「这台机器允许 app 做什么」,不是「另一个程序能不能找到它」。

`Ctrl-C` 在 CLI 自跑时用 `ctrlc` crate 只设一个 AtomicBool,记录动作发生在循环外、
和别处一样握着 vault 写锁。**倒计时期间不持锁** —— 另一个终端里的 `calpo today`
不该为了一个 25 分钟的番茄钟等在那里。

**自动更新** — `tauri-plugin-updater`,代码在 `integrations/updates.rs`。入口只有托盘菜单
那一项:没有启动时检查、没有定时器,也没有能把自动检查打开的开关 —— 「默认关闭」在这里
的意思是那段代码根本不存在。检查跑在 worker 上(`tauri::async_runtime::spawn`),所以
dialog 的 `blocking_show` 是合法的:它只是不许在主线程调。下载进度写回那个菜单项,每变
一个百分点才写一次(每次 setter 都是一趟主线程往返)。不进 `IntegrationStatus` —— 那个
面板讲的是「这台机器允许 app 做什么」,而这件事问的是服务器。

公钥在 `tauri.conf.json` 的 `plugins.updater.pubkey`,私钥在仓库 secret
`TAURI_SIGNING_PRIVATE_KEY`,本机副本在 `~/.tauri/calenpomo.key`(无口令)。**私钥丢了
就再也发不出能被已安装版本接受的更新**。代价是本地 `npm run tauri build` 不导出私钥会直接
失败(`A public key has been found, but no private key`),临时打包加 `--no-sign`。

macOS 的 `--bundles` 必须含 `app`:bundler 只在「构建了 updater 支持的目标」时才产出更新
产物,那个名单是 app / appimage / msi / nsis,**`dmg` 不在其中**。

`latest.json` **不交给 tauri-action 生成**,两个理由:它是「读现有的 → 加自己这个平台 →
写回去」,三个并行 job 里最后完成的那个会把另外两个平台抹掉;而且草稿 release 的 asset
URL 不是发布之后的那个。所以 `release.yml` 里由 `latest-json` 一个 job 在三个构建之后自己
拼,URL 按 tag 拼死,凑不齐四个键就让这一档失败(少一个平台却照发,那个平台的用户会被
告知「没有更新」而不是「出事了」)。**是四个键不是三个**:插件只查 `{os}-{arch}` 与
`{os}-{arch}-{installer}`,**没有 `darwin-universal` 这个回退**,所以那一个通用包要同时
挂在 `darwin-aarch64` 和 `darwin-x86_64` 下。

**安装脚本** — `install.sh` 是 POSIX sh(所以 `| sh` 是诚实的,不写 bashism),`install.ps1`
是它的 Windows 对应物。两个都把全部逻辑放进函数、**最后一行才调用**:下载被截断时,得到的
是一堆定义好却没跑的函数,而不是执行了一半的安装。

产物按**名字后缀匹配**,不是拼出来的:版本号一变,脚本不用跟着改。取资产列表时就把
`.sha256` 和 `.sig` 滤掉 —— `calpo-linux-x86_64.sha256` 匹配得上
`calpo-linux-*` 的每一个模式,不滤会挑到校验和本身。每个产物都对 `.sha256` 校验,**没有
校验和是拒绝安装而不是跳过**。

macOS 装 `.app.tar.gz` 而不是 dmg(不用挂载)。`/Applications` 对管理员组是可写的,所以
通常不需要 sudo,不可写才退到 `~/Applications`。curl **不会**打 quarantine 标记(只有浏览器
用的那套 API 会),所以 curl 装进去的未签名 app 反而不会被 Gatekeeper 拦下 —— 脚本里那行
`xattr -dr` 是给「文件从别的路子来的」兜底。

Linux 的图标从 AppImage 自己里抽,但要用**已经装好的那一份**:curl 写下来的文件没有执行位,
不能执行的 AppImage 也就不能解包(这条是桩测抓出来的,不是想出来的)。抽不到就不写
`Icon=` 那一行,不是失败。

`calpo` 两个平台统一装 `~/.local/bin`,不要 sudo;不在 PATH 上就把该加的那行打印出来。

**发布** — 两个 workflow。`ci.yml` 的矩阵是 ubuntu-22.04 / macos-latest /
windows-latest,**Linux 用 22.04 而不是 latest**:AppImage 里带着链接时的 glibc,在
24.04 上打的包到 22.04 和 Debian 12 就起不来,而 CI 只有和发布同一套环境才作数。
`cargo fmt` 只在 Linux 跑一次(格式不会因平台而异),CLI 那一半排在 app 前面 ——
它不需要系统库,而且它覆盖的是两边共享的代码,先看到它坏更有用。

`release.yml` 里 `prepare` 一个 job 先建好草稿 release、把 `releaseId` 发给三个并行的
构建 job:让 tauri-action 各自去 create 会撞出重复 release。草稿是因为 macOS 没签名,
发布前那段 Gatekeeper 的话得由人过一眼。**bundler 只打包 app**,`calpo` 是同一个 crate
里的第二个 bin,要自己 `--no-default-features` 编了再 `gh release upload` 挂上去;macOS
上编两个 target 再 `lipo` 成一个通用二进制。`--bundles` 的取值按平台过滤,`tauri.conf.json`
的 `"all"` 保持不动,收窄只发生在 workflow 里,这样本地 `npm run tauri build` 行为不变。
macOS 只传 `dmg`:`.app` 顺路就建好了,而 tauri-action 的 `artifactPaths` **无条件包含
`.app` 目录**,它会自己打成 `.tar.gz` 再上传 —— 那正是 `install.sh` 要的形状。每个产物
配一个 `.sha256`(`sha256sum` 在 macOS 上没有,`shasum` 在 Windows 上没有,取其一)。

校验和由最后那个 `finish` job 对**已经上传的资产**算,不是对本地构建产物算:tauri-action
会给它上传的一部分东西改名(macOS 那个 tarball 会带上架构),而按本地文件名存下来的校验和
对不上任何一个能下载到的名字。`install.sh` 找的正是 `<资产名>.sha256`。这个 job 不 checkout,
所以要给它 `GH_REPO` —— `gh release` 会去问 git 当前仓库是哪个,而 `gh api` 不会(仓库名在
URL 里),于是第一次跑时它一路跑到最后一条命令才说「not a git repository」,顺带把 release
notes 静默取成了空串。

版本号有四份(tag / `Cargo.toml` / `package.json` / `tauri.conf.json`),
`.github/scripts/check-versions.sh` 是发布的第一道闸:对不上就不构建。这四份不一致时
哪里都不报错,直到有人说 v0.2.0 装出来是 0.1.0。

## 代码约定

- Rust:`cargo fmt` 与 `cargo clippy` 必须干净(命令见下)。
- 错误处理用 `thiserror`,不要 `unwrap()`。**已知的有意例外**:`commands.rs` 中 Mutex 中毒时
  `unwrap_or_else(|e| e.into_inner())` 恢复而非传播——别处的 panic 不说明这份数据有问题。
- Vue:组合式 API,`<script setup>`。业务逻辑抽成 composable,不要堆在组件里。
- 前后端共享的类型定义**手工维护**在 `src/types/`,与 Rust 结构体一一对应。不要上代码生成。
- 提交信息用 Conventional Commits。

## 常用命令

仓库根目录**没有** `Cargo.toml`,裸跑 `cargo test` 会失败,必须带 `--manifest-path`:

```bash
cargo test --manifest-path src-tauri/Cargo.toml                              # Rust 测试
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check                    # 检查格式
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm run tauri dev        # 开发环境;beforeDevCommand 会自己起 vite,不要另开
npm run tauri build -- --no-sign   # 本地打包;不加这个会因为找不到 updater 私钥而失败
npm run typecheck        # 前端没有 ESLint/Prettier,lint 就是这个
npm test                 # vitest,只测 src/lib/ 下的纯函数
```

CLI 那一半要单独再跑一遍 —— `gui` 关掉后是另一套编译产物,只跑默认 feature 是测不到的:

```bash
cargo build  --manifest-path src-tauri/Cargo.toml --bin calpo --no-default-features
cargo test   --manifest-path src-tauri/Cargo.toml --no-default-features
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --no-default-features -- -D warnings
```

打 tag 之前先自己跑一遍版本闸(CI 里跑的是同一个脚本):

```bash
.github/scripts/check-versions.sh v0.2.0
```

## 明确不做的事(v1)

不要主动实现以下任何一项,即使看起来顺手:

- 账号、登录、云同步、任何**自动发生的**网络请求(用户手点的更新检查是唯一例外)
- 日视图、月视图、议程视图 —— **只做周视图**
- 提醒/闹钟系统(番茄钟结束通知除外)
- 标签系统、看板、笔记、任务依赖
- 主题切换、设置面板(除非任务里明确要求)
- CRDT 或任何冲突合并机制
- 在 UI 里创建/编辑重复事件(展开渲染已做,`RECURRENCE-ID` 覆盖是 v2 的事)

## 待确认(别自己拍板,先问我)

- `SCHEMA_VERSION` 常量与 `DEFAULT_CONFIG` 里硬写的 `schema_version = 1` 是两份,版本一升就会漂。
- `tauri.conf.json` 的 `"csp": null` 是脚手架默认值还是有意设的。
- `opening_a_fresh_directory_builds_the_whole_skeleton` 断言 `report.created` 的精确顺序,是否算契约。
- 前端不加 ESLint/Prettier 是刻意还是未做(vitest 已经加了,lint 仍然没有)。
- `.index/cache.db` 这个文件名代码里还不存在,是否还作数。
- 全天(DATE 值)事件目前渲染在日期头下面一条只读窄条里,这是实现时自己定的,要不要保留。
- 周视图固定周一起始,没有做成可配置。

## 工作方式

- 动手写代码前先说明你的计划,等我确认。
- 一次只做一件事。不要顺手重构无关代码。
- 涉及上述四条不变量的地方,写测试。
- 不确定就问,不要猜。
