# 一加 Ace（pickle）Skyward 镜像构建报告

构建时间：2026-11-13
产物：`/workspace/skyward-pickle-vendor_boot.img`（67,108,864 字节 = 64 MiB，正好填满 vendor_boot 分区）
SHA-256：`595ed6a3f480ae4668a89e255131fb1ca4ec3c2ec53cc49c15a0e61b7aece285`

用的是你给的三个分区 dump，不需要 OTA 包：

| 输入 | 用途 |
|---|---|
| `vendor_boot_20261111_203114.img` | 原厂 recovery 根文件系统 + 设备树，整个镜像的底子 |
| `vendor.img` | 核对 keymint / gatekeeper / Trustonic 的路径与参数（只读参考） |
| `vendor_dlkm_b.img` | 核对 WLAN 模块名与依赖（只读参考） |

---

## 一、为构建改的代码（4 处）

Skyward 原来只能打包 `boot` / `init_boot` / `recovery`（头 v3/v4，kernel + 一段 ramdisk），
碰到 `VNDRBOOT` 会直接报错。这次补了三块，另加一份设备配置。

| 文件 | 改动 | 为什么必须改 |
|---|---|---|
| `crates/skyward-bootimg/src/vendorboot.rs`（新增） | vendor_boot v3/v4 的读写：2128 字节头、ramdisk 表（多片段、每个片段的 type / 板 ID）、dtbo 段、bootconfig，逐段 4096 对齐 | 这台机器的 recovery 根文件系统在这个镜像里，没有 recovery 分区可写 |
| `crates/skyward-pack/src/main.rs` | `recovery` 子命令按 magic 分流：`VNDRBOOT` 走 vendor_boot 路径，否则走原来的 boot/recovery 路径；把「往 ramdisk 里装 Skyward」抽成 `skyward_on()` 两边共用 | 同一套设备配置既能给小米那种 recovery 分区用，也能给这种 vendor_boot 用 |
| `crates/skyward-block/src/fstab.rs` | 找分区表时除了 `fstab.qcom`，还扫 `/vendor/etc`、`/odm/etc`、`/first_stage_ramdisk`、`/etc` 下的 `fstab.*` | 原代码写死高通的文件名。MTK 的表叫 `fstab.mt6895`，不改的话挂 persist 时直接 `the phone's own table does not mount persist` 退出，UI 起不来 |
| `devices/pickle.toml` | 设备配置（本报告后半部分逐段说明） | — |

改动已导出成补丁：`/workspace/skyward-pickle.patch`（772 行，含新增文件）。

`vendorboot.rs` 带了 10 个单元测试；拿原厂镜像做了往返验证：**解析再写出，与原文件的
前 38,776,832 字节逐字节相同**（含头、ramdisk、dtbo、表、填充）。整工作区 `cargo test`
除一个与本次改动无关的用例外全绿（见文末）。

---

## 二、构建环境

| 项目 | 值 |
|---|---|
| Rust | 1.100.0 stable，target `aarch64-linux-android` |
| NDK | r29（clang 21），API level 30 |
| 交叉编译方式 | 直接给 `CC_aarch64_linux_android` + `CARGO_TARGET_..._LINKER`，不需要 cargo-ndk |
| 压缩 | `lz4 -l -12 --favor-decSpeed`（legacy 帧，内核 initramfs 只认这个） |

一键脚本：`/workspace/build-pickle.sh`（`NDK=... ./build-pickle.sh [原厂 img]`）。
它做三件事：编 `skyward-pack`（电脑）、编 `skyward`（手机）、静态编 `skyward-system-init`
（手机），最后调 `skyward-pack recovery` 出镜像。

产物大小：

| 文件 | 大小 |
|---|---|
| `skyward`（aarch64，strip，只依赖 libc/libm/libdl） | 8,589,840 |
| `system-init`（静态） | 1,040,216 |
| MiSansVF.ttf | 20,093,424 |
| KirkMono-Regular.ttf + 许可证 | 270,456 + 4,483 |

---

## 三、产物核对

### 3.1 镜像结构（与原厂同构）

| 段 | 值 |
|---|---|
| magic / 版本 | `VNDRBOOT` / v4 |
| page_size | 4096 |
| ramdisk_addr | `0x66f00000`（原样） |
| dtb_addr / dtb_size | `0x47c80000` / 308,024（原样，逐字节未动） |
| cmdline | `bootopt=64S3,32N2,64N2 buildvariant=user`（原样） |
| ramdisk 表 | 1 项，type 1，offset 0，size 59,228,062 |
| bootconfig | 0 |
| vbmeta | 59,547,648 起，512 字节，`AVB0`，算法 0（不签名，与库存一致） |
| AVB footer | 分区末 64 字节，`original_image_size=59,547,648`，指向 vbmeta |
| 分区填充 | 64 MiB 整，vbmeta 到 footer 之间全零 |

AVB 的 hash descriptor 用 `sha256(salt ‖ image)` 复算过，与写进去的 digest 一致；
分区名写的是 `vendor_boot`。

### 3.2 ramdisk 内容（108 MB → 135.9 MB 未压缩）

| 检查 | 结果 |
|---|---|
| 条目数 | 929 → 941 |
| 原厂文件丢失 | **0**（逐条 comm 对比） |
| 内核模块 | 270 个 `.ko`，一个不少 |
| `system/bin/recovery` | 已换成 Skyward，sha256 与交叉编译产物一致（两个副本同一个哈希） |
| `system/bin/{7za,avbctl,resetprop,zstd,unzip}` | 5 个符号链接 → `recovery` |
| `skyward/` | `device.toml`、`MiSansVF.ttf`、`KirkMono-Regular.ttf`、`KirkMono-OFL.txt`、`system-init` |
| `prop.default` | 末尾追加 `ro.secure=0`、`ro.adb.secure=0`、`ro.debuggable=1`、`ro.ota.allow_downgrade=true` |
| 原厂 init 链路 | `init.rc`、`init.recovery.mt6895.rc`、`phoenix_recovery`、`modules.load.recovery` 全部保留 |

启动链路没有动：`init.rc` 里 `service recovery /system/bin/recovery` 由 `class_start default`
拉起，`phoenix_recovery`（110 KB 的 OPLUS 壳，动态符号里没有 `execve`）继续按原样跑；
Skyward 被以 `recovery` 这个名字启动时会忽略命令行参数（`main.rs` 里专门处理了这点），
所以就算原厂壳传了 BCB 参数也不影响。

### 3.3 空间账（64 MiB 分区）

| 项 | 字节 | 说明 |
|---|---|---|
| 头 | 4,096 | |
| ramdisk（压缩后） | 59,228,062 | 原厂 38,456,840，新增 20.8 MB 全是字体 + 二进制 |
| dtbo | 308,024 | 原样 |
| ramdisk 表 | 108 | |
| 对齐填充 | ~7,300 | |
| vbmeta | 512 | |
| footer | 64 | |
| **合计** | **59,547,648（56.8 MiB）** | 余量约 7.2 MiB |

余量的大头被 20 MB 的 MiSansVF 吃掉（lz4 只压到 16.2 MB，TTF 本来就没多少冗余）。
如果以后还要往 ramdisk 里塞东西，先动字体（子集化能省 10 MB 以上），别动模块。

---

## 四、刷入与回滚

> **先看清楚**：vendor_boot 在 GKI 机器上是启动链的一环，不只是恢复模式用。
> 刷之前把手上的 `vendor_boot_20261111_203114.img` 留好，它就是回滚包。
> 我在这里放了一份原样副本：`/workspace/pickle-stock-vendor_boot.img`
> （SHA-256 `bb5a094de2f3396b9be3b20354d637092ef57d1355413b784cd863f222839670`）。

```sh
# 1. 看当前槽位
fastboot getvar current-slot          # 假设是 a，则写 vendor_boot_a

# 2. 刷（A/B 机器要指名槽位，别让 fastboot 猜）
fastboot flash vendor_boot_a skyward-pickle-vendor_boot.img

# 3. 进恢复模式（一加一般是关机后 音量下 + 电源，或系统里 adb reboot recovery）
fastboot reboot
```

回滚：

```sh
fastboot flash vendor_boot_a vendor_boot_20261111_203114.img
fastboot reboot
```

刷之前建议在恢复模式里先 `adb pull /proc/last_kmsg` 之类留一份原厂日志，方便对比。

---

## 五、还没验证的部分（必须在真机上过一遍）

镜像的**结构**是可信的（逐字节往返、AVB 复算、文件清单全对），但**运行时**做不到离线验证。
按风险从高到低：

1. **vendor_boot 是否被正常开机共用**。这个 ramdisk 里是整个 recovery 根（含
   `init.recovery.mt6895.rc`、108 MB），看起来是专供恢复模式的；但如果 MTK 的正常启动
   也读这一段，最坏情况是系统也起不来。**这是刷机前最该确认的一点**，确认方式很土：
   刷完先进系统，能进就说明互不影响；进不去就回滚。
2. **DRM 节点**。Skyward 的 UI 走 `/dev/dri/card0`（`drm` crate + 裸 ioctl，不依赖
   libdrm.so，二进制只 NEEDED libc/libm/libdl——这点已验证）。原厂 recovery 用的是
   minui/fb，`mediatek-drm` 起来后有没有 `card0` 得真机 `ls /dev/dri` 看。
3. **SELinux**。Skyward 以 `u:r:recovery:s0` 跑，要 open `/dev/dri/card0`、
   `/dev/input/*`、`/dev/block/*`。原厂 recovery 域一般够宽，但如果被拒，
   `dmesg | grep avc` 会直接指出来。
4. **Trustonic**。`mcDriverDaemon` 的 31 个 `-r *.drbin` 里有一批在 `/odm/vendor/app/mcRegistry`
   （odm 分区），我们只挂了 persist（`/mnt/vendor/persist/mcRegistry` 是 TEE 的存储）。
   odm 带 `first_stage_mount`，正常应由原厂 init 挂好；真机上 `mount | grep odm` 确认一下。
   我已经去掉了 `--sfs-reformat`，不会重建 TEE 文件系统。
5. **触摸 / 面板**。配置按原厂 `modules.load.recovery` 的顺序加载了 5 个面板驱动
   （只有实际接的那颗会 probe 上）+ FT3683G 触摸；真机 `dmesg | grep -i lcm` 能看出
   实际是哪块屏，`ls /sys/class/input` 能看出触摸节点。
6. **振动**。`aw8697` 不暴露 f0 节点，`[haptics]` 段仍是注释状态，UI 里不会震。

真机跑起来后，把这三条命令的输出发我，我就能把剩下几个 TODO 填死：

```sh
cat /proc/modules | head -50
dmesg | grep -iE "lcm|touch|ft3683|drm"
ls /sys/class/drm /dev/dri 2>&1
```

---

## 六、附：一个与本次改动无关的测试失败

`cargo test --workspace` 里 `skyward-files` 的
`a_file_written_anew_takes_its_own_place_keeping_its_permissions` 失败，断言是
「新写的文件 inode 必须与旧的不同」。这是 overlayfs 会复用 inode 导致的，
改之前就存在，和 vendor_boot / fstab 的改动无关（`skyward-files` 一行没动）。
