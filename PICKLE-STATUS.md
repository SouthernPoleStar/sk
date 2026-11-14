# 一加 Ace（pickle）· Skyward 恢复镜像 —— 现状

> 最后更新：2026-11-14
> 状态：**未成功**。刷入后一直黑屏，且没有 adb。系统（ColorOS）本身可以正常启动。

---

## 一、目标

把 [Skyward](https://github.com/kirklin/skyward)（Rust 写的 Android 恢复模式）适配到 **一加 Ace**，并用本机提供的原厂 `vendor_boot` 镜像在本地打出可直接 `fastboot flash` 的恢复镜像，不依赖 OTA 包。

---

## 二、设备事实（实测确认）

| 项 | 值 |
|---|---|
| 代号 | `pickle`（PGKM10，国际版 10R） |
| SoC | 天玑 8100-Max（MT6895T），**联发科** |
| 系统 | ColorOS 12.1 / Android 12（SDK 31），`buildvariant=user` |
| 屏幕 | 1080×2412，密度 480 |
| recovery 位置 | **vendor_boot**（无独立 recovery 分区），A/B |
| vendor 代号 | `ro.product.vendor.device` = `ossi`（OPLUS 统一包） |
| 触摸 | FocalTech FT3683G（`oplus_bsp_tp_ft3683g` → `oplus_ft3683g`），工程 `omega-c2` |
| 振动 | `aw8697` + `haptic_hv`，无 f0 节点 |
| 解密 | Trustonic：`/vendor/bin/mcDriverDaemon` + keymint（AIDL）+ gatekeeper（HIDL `@1.0`） |
| WLAN | `conninfra` → `wlan_drv_gen4m_6895`，写 `1` 到 `/dev/wmtWifi`，supplicant HIDL `@1.0` |
| 显示 | **只有 DRM**：`/dev/dri/card0`、`card0-DSI-1` 存在，**无 `/proc/fb`** |
| 原厂 recovery | `/system/bin/recovery`，2.3 MB（Oplus 定制，`vendor/oplus/bootable/recovery`） |

---

## 三、已经做完的工作

### 3.1 代码改动（8 个文件）

| 文件 | 改动 |
|---|---|
| `crates/skyward-bootimg/src/vendorboot.rs` | **新增**。完整的 VNDRBOOT v3/v4 读写：2128 字节头、多片段 ramdisk 表（含 type/board_id）、dtbo、bootconfig、4K 对齐。11 个单测；真实原厂镜像往返前 38,776,832 字节逐字节一致 |
| `crates/skyward-bootimg/src/lib.rs` | 接入 vendorboot 模块 |
| `crates/skyward-pack/src/main.rs` | ① `recovery` 命令按 magic 分流，VNDRBOOT 走新路径；② 改 `prop.default` 时先删原厂 `ro.secure`/`ro.adb.secure`/`ro.debuggable` 再写 `0/0/1`；③ 原厂 recovery 备份为 `/system/bin/recovery.stock`；④ `service recovery` 插入 `oneshot` |
| `crates/skyward-block/src/fstab.rs` | `FILES` 从只认 `fstab.qcom` 改为扫描 `/vendor/etc`、`/odm/etc`、`/first_stage_ramdisk` 下所有 `fstab.*`（否则 MTK 挂 persist 直接退出） |
| `crates/skyward-ui/src/surface.rs` | ① DRM `SET_MASTER`；② 无 `PREFERRED` 标志时取最大分辨率模式；③ fourcc 候选改序（ARGB 优先）；④ 每像素强制 alpha=255；⑤ plane `alpha` 属性设 `0xffff` |
| `crates/skyward/src/ui.rs` | `Backlight::open` 增加 `/sys/class/leds` 回退（只收名字含 `backlight`/`lcd`）；背光/振动/control 失败只记日志不退出 |
| `crates/skyward-sys/src/lib.rs` | `keep_log` 允许重复调用（`OnceLock` 改可重设） |
| `crates/skyward/src/main.rs` | ① panic hook + 失败时 `exec /system/bin/recovery.stock`；② 移除会**停掉 adbd** 的 `usb::set("none")`；③ 日志双写 `/mnt/vendor/persist/skyward.log`；④ 挂载 persist |

### 3.2 设备配置

`devices/pickle.toml`：12 个模块、5 个 service、WLAN 段，已通过 `cargo run -p skyward-config` 解析校验。

### 3.3 构建与产物

- `build-pickle.sh`：一键构建脚本（NDK r29，target `aarch64-linux-android30`，`-C target-feature=+crt-static` 编 system-init）
- `skyward-pickle-vendor_boot.img`：67,108,864 B（正好 64 MiB），ramdisk 解压后 138 MB / 942 条目
- `pickle-stock-vendor_boot.img`：原厂回滚包
- `skyward-pickle.patch`：全部改动的 diff（约 1000 行）

---

## 四、当前问题

**刷入后一直黑屏，没有 adb。从第一版起就从未点亮过屏幕。**

| 现象 | 说明 |
|---|---|
| 屏幕全黑 | 无 logo、无花屏、无背光 |
| 无 adb | `adb devices` 空 |
| 系统能进 | 正常启动（非 recovery）能进 ColorOS → **说明 vendor_boot 被正确加载、ramdisk 被正确解压、内核正常** |

这条「系统能进」是重要证据：它排除了 ramdisk 损坏、AVB 校验失败、分区大小超限等一整类可能。

---

## 五、2026-11-14 新查到的关键事实

### 5.1 `service recovery` 的启动链路

`system/etc/init/hw/init.rc` 里：

```
service recovery /system/bin/recovery
    socket recovery stream 422 system system
    seclabel u:r:recovery:s0
```

**没有 `class` 声明** → 默认属于 `default` 类。它由 `on boot` 段的最后一行启动：

```
on boot
    ifup lo
    hostname localhost
    domainname localdomain
    exec u:r:recovery:s0 root root -- /system/bin/cp -af /res/OplusOS/. /res
    exec u:r:recovery:s0 root root -- /system/bin/cp -arf /vendor/app/mcRegistry/. /odm/vendor/app/mcRegistry/
    class_start default
```

### 5.2 adb 的启动链路（关键）

```ini
# init.rc
on fs && property:ro.debuggable=1
    setprop sys.usb.config adb

on property:sys.usb.config=adb
    start adbd
    start console
    start logd
```

也就是说：**只要 `ro.debuggable=1` 生效，adb 就会自己在 `on fs` 阶段被拉起来，与 `recovery` 二进制是否正常无关。**

现在 adb 完全没有 → 极可能是 **`ro.debuggable=1` 没有生效**（`ro.` 属性是 write-once，而 `buildvariant=user` 可能让 MTK/Oplus 的 init 强制回 `0`）。

### 5.3 原厂自带改 prop 的脚本

`sbin/release_root.sh`：

```sh
sed -i 's/ro.secure=1/ro.secure=0/g'      $default_prop
sed -i 's/ro.adb.secure=1/ro.adb.secure=0/g' $default_prop
sed -i 's/ro.debuggable=0/ro.debuggable=1/g' $default_prop
```

说明 Oplus 自己也认可这种改法，路径是 `/system_root/system/etc/prop.default`（不是我们改的 `/prop.default` 也不是 `/system/etc/prop.default`）。

### 5.4 TWRP（用户提供，已拆解分析）

| 项 | 值 | 意义 |
|---|---|---|
| ramdisk 解压后 | **153 MB** | 比我们的 138 MB 还大 → **体积不是黑屏原因** |
| 压缩后 | 62 MB | 分区 64 MiB 放得下 |
| 显示后端 | `/dev/dri/card0` + libdrm（静态编进 `libminuitwrp.so`） | 确认这机器只能走 DRM，无 fb0 |
| 显示接口 | `drmModeSetCrtc` + `drmModeAddFB`（**旧式**） | 我们用的是 atomic → **这是花屏/黑屏最可疑的差异** |

---

## 六、已确认的 Bug（尚未修复）

### 6.1 `without()` 误删无关属性 —— 高危

打包器里删 `ro.secure` / `ro.adb.secure` / `ro.debuggable` 的函数，实际额外删掉了 6 行：

```
-ro.build.version.release=12
-ro.build.version.security_patch=2026-04-01
-ro.vendor.build.security_patch=2026-04-01
（两段各删一遍）
```

这会让 `ro.build.version.release` 缺失，部分组件会按 Android 版本分支走进错误路径。**必须修。**

### 6.2 `recovery.stock` 的 SELinux label —— 高危

兜底逻辑 `exec /system/bin/recovery.stock`，但这是**新路径**，不在 `plat_file_contexts` 里 → label 是 `u:object_r:rootfs:s0`，而 `u:r:recovery:s0` 域对 `rootfs` 通常没有 `execute` 权限 → **exec 被拒绝** → `exit(1)` → 黑屏。

也就是说：**「崩了就交回原厂 recovery」这条兜底从来没生效过**，反而因 `oneshot` 变成彻底黑屏。

需要在 init.rc 里加：

```
on post-fs
    chcon u:object_r:recovery_exec:s0 /system/bin/recovery.stock
```

---

## 七、下一步计划

按优先级：

| # | 动作 | 目的 |
|---|---|---|
| 1 | 修 `without()` 的前缀误匹配 | 避免删掉 `ro.build.version.*` |
| 2 | init.rc 里给 `recovery.stock` 加 `chcon` | 让兜底真正可用 |
| 3 | **init.rc 里硬启动 adb**（`on boot` 中 `start adbd` + `setprop sys.usb.config adb`），不依赖 `ro.debuggable` | 拿到 adb，拿到日志 |
| 4 | 显示改回 TWRP 同款 `drmModeSetCrtc` + `drmModeAddFB` | 排除 atomic 兼容性 |
| 5 | 用原厂镜像做对照：`fastboot flash vendor_boot_a pickle-stock-vendor_boot.img`，确认原厂 recovery 能亮屏 | 区分「我改坏了」与「这机器本来就这样」 |

**当前卡点**：没有 adb 就拿不到 `dmesg`，只能盲改。**第 3 步是解锁一切的前提。**

---

## 八、文件清单

```
├── README.md / README.zh-CN.md     Skyward 上游文档
├── PICKLE-STATUS.md                本文档
├── build-pickle.sh                 一键构建
├── pickle-build-report.md          构建报告
├── pickle-vendor_boot-report.md    vendor_boot 结构分析
├── pickle-flash-guide.md           刷机指南（含回滚）
├── skyward-pickle.patch            全部代码改动 diff
├── skyward-pickle-vendor_boot.img  成品镜像（64 MiB）
├── pickle-stock-vendor_boot.img    原厂回滚镜像（64 MiB）
├── pickle-stock/                   从原厂 ramdisk 提取的关键文件
│   ├── init.rc
│   ├── recovery.fstab / fstab.mt6895 / modules.load.recovery
│   └── vendor 的 keymint / gatekeeper / trustonic .rc
├── crates/                         Rust 源码（含 vendorboot.rs）
└── devices/pickle.toml             设备配置
```

---

## 九、刷写与回滚

```sh
# 刷入
fastboot flash vendor_boot_a skyward-pickle-vendor_boot.img
fastboot flash vendor_boot_b skyward-pickle-vendor_boot.img
fastboot reboot recovery

# 回滚（随时可用）
fastboot flash vendor_boot_a pickle-stock-vendor_boot.img
fastboot flash vendor_boot_b pickle-stock-vendor_boot.img
```

回滚镜像与原厂分区逐字节同尺寸，刷回即可恢复出厂 recovery。

---

## 十、给接手的人

如果继续这个项目，请先做第七节的第 3 步：**让 adb 起来**。没有 `dmesg` 的情况下，任何对显示参数的修改都是猜测，而这个项目已经因此浪费了三轮刷机。
