# 一加 Ace（pickle）刷入 Skyward：风险与操作手册

> 结论先说：**硬砖（需要 EDL / 授权文件才能救）的概率极低；软砖（起不来，但能进 fastboot，刷回即可）的概率中等；数据不会丢。**

---

## 一、为什么不太可能硬砖

| 依据 | 实测结果 |
|---|---|
| **头部完全合法** | 与原厂同为 `VNDRBOOT` v4、page 4096、2128 字节头、单片段时间表、dtbo 段原样搬运 |
| **压缩格式一致** | 原厂和成品都是 **LZ4 legacy frame**（magic `02 21 4c 18`），连首块大小（3,199,089 字节）都相同 |
| **AVB 行为一致** | 原厂：`algo=0 (NONE)`、`rollback_index=0`、`flags=0`、无签名；成品完全相同。**不存在防回滚拒绝、也不存在签名不匹配** |
| **大小一致** | 两份都是 **67,108,864 字节（正好 64 MiB）**，与 dump 出来的分区等大 |
| **没有删任何东西** | 原厂 929 个条目一个不少，270 个 `.ko` 全在，只新增 12 项 |
| **不碰 bootloader** | preloader / lk / vbmeta 分区一个字节都没动，只改 vendor_boot |

真正会硬砖的操作是刷坏 preloader、lk、分区表（GPT），或者触发 MTK 的安全启动熔断。**这次都不涉及。**

---

## 二、可能发生的情况

| 结果 | 概率 | 表现 | 怎么处理 |
|---|---|---|---|
| 正常进 Skyward | 未知，取决于 DRM/触摸 | 看到 Skyward 界面 | 成功 |
| **画面仍然是花的，但 adb 能连** | 中等 | 屏幕有内容但不正常 | 这是好消息：发我 `dmesg`，可以精确定位 |
| **黑屏 / 卡开机第一屏** | 中等 | 屏幕不亮或停在 logo，反复重启 | 进 fastboot 刷回原厂，5 分钟恢复 |
| 卡在 "Orange State / device is corrupt" | 低 | bootloader 警告页 | 按电源键继续，或刷回原厂 |
| fastboot 直接拒绝写入 | 低 | `Image is too large` / `not allowed in Lock State` | **什么都没发生**，设备毫发无损 |
| 需要 EDL 才能救 | 极低（<1%） | 连 fastboot 都进不去 | 需要 OPPO 授权文件，个人基本无解 |

> 最坏情况的触发条件是 bootloader 解析 vendor_boot 时崩溃。因为头部/表结构/对齐全部照抄原厂，这个概率很低；但不是零。**所以下面的备份步骤不能省。**

---

## 这一版（2026-11-14 更新）改了什么

上一版的问题不只是画面：`recovery` 服务被 init 反复重启，每次重启都把 USB 端口拆掉一次，最后连 adb 都没了。这一版针对这条链：

| 改动 | 作用 |
|---|---|
| `sys.usb.config=none` 失败不再终止进程 | 端口拆掉后一定能设回 `adb`，不会被自己的 `?` 打断 |
| panic 与启动失败改为 **exec 原厂 recovery** | 真崩了就把手机交给 `recovery.stock`（原厂二进制仍留在 ramdisk 里），不会再无限重启 |
| `service recovery` 加 `oneshot` | 即使遇到 SIGSEGV 这类 hook 抓不到的崩溃，也只跑一次，adb 留在原地 |
| 日志同时写 `/mnt/vendor/persist/skyward.log` | 重启后还在，下一次不管进系统还是进恢复模式都能读 |
| 显示优先 `ARGB8888`，并强制每像素 alpha=255 | 联发科的 mixer 会混合带 alpha 的图层；画布上没画过的像素 alpha 是 0，会被当成"洞" |
| plane 的 `alpha` 属性设为 0xffff | 同上，双重保险 |

如果这版 adb 回来了但画面仍然不对，那就只剩 DRM 本身的问题，需要 `dmesg` 里的 `skyward: display:` 那一行来确定。

---

## 三、刷之前必做（5 分钟）

### 1. 确认解锁状态

```sh
fastboot getvar unlocked          # 期望 yes
# 或
fastboot oem device-info          # 看 Device unlocked: true
```

未解锁就**别刷**，刷了也会被拒绝，白折腾。

### 2. 记录当前 slot

```sh
fastboot getvar current-slot      # 记下 a 或 b
fastboot getvar all | grep vendor_boot
```

期望看到 `vendor_boot_a` / `vendor_boot_b` 两个分区名，记下实际名字。

### 3. 备份设备上**当前**的 vendor_boot（关键）

你给我的原厂包是之前 dump 的，设备上现在这份未必完全相同（比如中间升过级）。**以设备上现存的为准**：

```sh
# 方法一：fastboot 直接取（部分版本支持）
fastboot fetch vendor_boot_a vb_backup.img

# 方法二：系统里用 root 读分区（推荐，最可靠）
adb shell su -c "dd if=/dev/block/by-name/vendor_boot_a of=/sdcard/vb_backup.img"
adb pull /sdcard/vb_backup.img .
```

拿到后核对大小，应该是 67,108,864 字节：

```sh
ls -l vb_backup.img
```

**这个 `vb_backup.img` 才是你的第一回滚包。** 我给的 `pickle-stock-vendor_boot.img` 是第二重保险。

---

## 四、刷入（只刷当前 slot）

```sh
# 假设 current-slot = a
fastboot flash vendor_boot_a skyward-pickle-vendor_boot.img
fastboot reboot
```

**不要刷 `vendor_boot_b`。** 保留另一个 slot 的原厂镜像，多一层退路。

如果 fastboot 报 `Image is too large`，**立刻停手**——说明实际分区比 64 MiB 小，什么都没写进去，设备没事。

---

## 五、刷完怎么判断

| 现象 | 含义 | 动作 |
|---|---|---|
| 出现 Skyward 界面，能触摸 | 成功 | 继续验证 WLAN、备份等 |
| **黑屏但电脑能识别 adb 设备** | UI 起不来（DRM 节点问题），**不是砖** | `adb shell` 能进就说明系统在跑，刷回或等修 |
| 画面正常但触摸没反应 | 触摸模块没加载，**不是砖** | 同上 |
| 卡 logo / 反复重启 | 启动失败 | 进 fastboot 刷回 |
| 完全黑屏、adb 也认不到 | 启动链断 | 进 fastboot 刷回 |

进 fastboot 的方法（系统起不来时）：**关机，按住音量下 + 电源**，出现 fastboot 界面再松手。

---

## 六、恢复流程

```sh
fastboot flash vendor_boot_a vb_backup.img      # 优先用自己备份的
# 或
fastboot flash vendor_boot_a pickle-stock-vendor_boot.img
fastboot reboot
```

如果 fastboot 进不去：长按电源 20 秒强制断电，再试按键组合。MTK 机型还可以试 `音量上 + 音量下 + 电源`（会进 BROM，但 OPPO 需要授权文件，属于最后手段）。

---

## 七、两个已知的"看起来像问题"的点

### 1. prop.default 的改动实际上不会生效

原厂 `prop.default` 第 66、425 行已经是 `ro.secure=1`，第一版只在末尾追加 `ro.secure=0`，init 会把它当"只读属性重复设置"忽略。

- **第二版已修**：打包时先把原厂的 `ro.secure` / `ro.adb.secure` / `ro.debuggable` 三行删掉，再写 0/0/1，所以 recovery 里 adb 会直接是 root，且不需要在手机上点"允许调试"。
- **正常系统不受影响**：你实测 `getprop ro.secure` 仍是 1，说明正常启动读的是 boot.img 的 ramdisk，这份 prop.default 只在恢复模式用。

### 2. 原厂那份 `com.android.build.vendor_boot.fingerprint` 属性描述符没复制

avbtool 生成的 vbmeta 里，原厂多一个 152 字节的属性描述符（记录编译指纹）。它**不参与启动校验**，只影响 `avbtool` 查询信息。bootloader 只校验 hash 描述符，已复算一致。

---

## 八、第二版针对黑屏改了什么

你提供的输出里最关键的两条：`/dev/dri/card0` 存在且有 `card0-DSI-1`，但 **`/proc/fb` 不存在**——这台机器根本没有 fbdev，原厂 recovery 也是走 DRM。所以黑屏不是"没有显示设备"，而是 Skyward 在初始化链上某一环退出了。

第二版改了五处：

| 改动 | 为什么 |
|---|---|
| 背光增加 `/sys/class/leds/lcd-backlight` 回退 | MTK 的 LCD 背光常常只作为一颗 LED 注册。原来只读 `/sys/class/backlight`，读不到就 `?` 返回，整个 UI 一行都不画——这是黑屏最可能的原因 |
| 背光、振动、recovery socket 打不开不再致命 | 原来任何一个失败都会让屏幕不亮。现在记一行 kmsg 后继续 |
| 面板没有 `PREFERRED` 标志时取最大分辨率模式 | MTK 的 DSI connector 不一定给这个标志，原来会直接报 `the panel reports no preferred mode` |
| 打开 card0 后显式 `DRM_IOCTL_SET_MASTER` | 避免被 bootloader 留下的显示客户端挤成 secondary，导致 atomic commit 被内核拒绝 |
| 保留原厂 recovery 为 `/system/bin/recovery.stock` | Skyward 的屏幕仍然打不开时，自动 exec 原厂 recovery，至少给一个能看的画面，不会黑屏 |

另外每步都往内核日志写一行：`display: ...`、`backlight: ...`、`touch and keys: ...`、`recovery socket: ...`。

## 九、如果这次还是黑屏

adb 这次应该直接连上（无需在手机上授权）。插上 USB 后跑：

```sh
adb devices
adb shell dmesg | grep -iE "skyward|display|backlight|vibrator|touch|recovery"
```

要看的顺序：

1. `recovery up, misc command ...` —— Skyward 起来了
2. `display: ...` —— 显示打开成功，后面跟着 connector / mode / plane 摘要
3. `backlight: ...` / `vibrator: ...` / `recovery socket: ...` —— 哪一项降级了
4. `recovery failed: ...` —— 真正退出的原因
5. `starting the stock recovery at /system/bin/recovery.stock` —— 兜底生效了，屏幕这时应该显示原厂恢复界面

如果什么都没打印，说明 Skyward 根本没被 init 启动，那就是 `service recovery` 那一层的问题。

---

## 十、刷成功之后请给我三条输出

```sh
cat /proc/modules | head -50
dmesg | grep -iE "lcm|touch|drm|panel"
ls /dev/dri
```

这三条能一次性把设备配置里剩下的 TODO（实际面板型号、触摸节点、DRM 是否可用）填死。

---

## 附：文件校验

| 文件 | 大小 | SHA-256 |
|---|---|---|
| `skyward-pickle-vendor_boot.img` | 67,108,864 | `5fd2a18ea0b5700a602cd802ec5ba3e74b8e505ca0f8f40038d8510ffcf2de1d` |
| `pickle-stock-vendor_boot.img` | 67,108,864 | `bb5a094de2f3396b9be3b20354d637092ef57d1355413b784cd863f222839670` |
