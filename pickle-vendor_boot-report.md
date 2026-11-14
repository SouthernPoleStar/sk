# 一加 Ace（pickle）原厂 vendor_boot 解剖

对象：`vendor_boot_20261111_203114.img`（来自 QQ 闪传的 7z 包，MD5 校验通过）
结论先说：**你说得对，它就是个引导镜像（"内核格式"指的就是这一类），不是文件系统。** 7z 能打开的是外层归档；里面的 `.img` 是 Android 引导镜像家族里的 **VNDRBOOT v4**，真正的根文件系统藏在它中间那段 LZ4 压缩的 ramdisk 里。

---

## 1. 为什么之前卡住了

| 层 | 实际情况 |
|---|---|
| 外层 | 7z 包 → 解出 64 MiB 的 `.img` |
| 镜像 | `VNDRBOOT` magic + header v4，page 4096 |
| ramdisk | 从偏移 4096 开始，magic `02 21 4c 18` = **LZ4 legacy frame**（老式帧格式） |
| 坑 | Python 的 `lz4` 库只认现代帧（`04 22 4d 18`），对 legacy 帧直接报 `ERROR_frameType_unknown`；所以按 block 反复试解一直失败 |
| 解法 | `lz4 -d`（命令行工具支持 legacy）→ 解出 108,256,512 字节的 newc cpio |

我用 `lz4` CLI 一次性解开了，`cpio -idm` 展开后是完整的 ColorOS 恢复模式根目录。

---

## 2. 镜像结构（逐段实测）

| 段 | 偏移 | 大小 | 内容 |
|---|---|---|---|
| header | 0 | 2,128 B | v4 头，`kernel_addr=0x40000000`、`ramdisk_addr=0x66f00000`、`tags/dtb_addr=0x47c80000` |
| cmdline | 28 | 2048 B | `bootopt=64S3,32N2,64N2 buildvariant=user` |
| ramdisk | 4,096 | 38,456,840 B | LZ4 legacy → 108,256,512 B newc cpio |
| dtbo 段 | 38,461,440 | 308,024 B | DTBO 表（magic `d7 b7 ab 1e`，2 个 entry），真正的 FDT 在其后 64 字节 |
| vbmeta | 38,776,832 | 384 B | `AVB0`，算法 0（**未签名**） |
| 填充 | — | — | 全零到分区尾 |
| AVB footer | 67,108,800 | 64 B | `AVBf`，`original_image_size=0x24FB000` |
| 合计 | — | 67,108,864 B | 正好 64 MiB，是分区 dump 而非裸镜像 |

> 头里的 ramdisk 表是 1 项且全零（`header_size=2128`，`bootconfig_size=0`），说明这是**单段 ramdisk**，没有 fragment 拆分，重打包时只需替换这一整段。

---

## 3. 最重要的发现：recovery 在 vendor_boot 里

解开的 ramdisk 里能直接看到：

- `init.recovery.mt6895.rc`（第一行 `import /init.recovery.project.rc`）
- `system/etc/init/hw/init.rc` 第 1 行就是 `import /init.recovery.${ro.board.platform}.rc`
- `system/bin/recovery`、`system/bin/phoenix_recovery`（OPLUS 的 Phoenix 恢复）
- `res/OplusOS/`（ColorOS 恢复模式的字体、图标、语言、主题）
- `system/etc/recovery.fstab`、`first_stage_ramdisk/fstab.mt6895`

**这台机器（MTK GKI 布局）把恢复模式的根文件系统放进了 vendor_boot，不是独立的 recovery 分区。**

这对 Skyward 是硬约束：它的 `skyward-bootimg` 目前只解析/打包 boot、init_boot、recovery 的 v3/v4 头（kernel + ramdisk 两段），`skyward-install` 对 `VNDRBOOT` 只做识别（用于刷写原厂包），**没有打包 vendor_boot 的能力**。要出可刷的镜像，必须新增 vendor_boot 后端。

---

## 4. 里面有什么（可直接进配置）

### 4.1 系统标识

| 项 | 值 |
|---|---|
| 平台 | `ro.board.platform=mt6895`（天玑 8100 系列） |
| 系统 | ColorOS 12.1 / Android 12（SDK 31） |
| 指纹 | `oplus/ossi/ossi:12/SP1A.210812.016/...`（ossi 统一包，多项目共用） |
| 密度 / 分辨率 | `ro.sf.lcd_density=480`，1080×2412 |
| TEE | `ro.hardware.gatekeeper=trustonic`、`ro.hardware.kmsetkey=trustonic` |
| 分区 | A/B + 动态分区（`ro.boot.dynamic_partitions=true`） |

### 4.2 分区表（`recovery.fstab`）

- 动态分区：`system`、`vendor`、`product`、`system_ext`、`odm`，以及 OPLUS 自有的 `my_product`、`my_heytap`、`my_stock`、`my_bigball`、`my_preload`、`my_manifest`、`my_company`、`my_carrier`、`my_region`、`my_engineering`
- 数据：`userdata` 为 **f2fs**，`fileencryption=aes-256-xts:aes-256-cts:v2+inlinecrypt_optimized`，`keydirectory=/metadata/vold/metadata_encryption`，`sysfs_path=/sys/devices/platform/soc/112b0000.ufshci`
- 缓存：**没有 cache 分区**，`/cache` 指向 `oplusreserve2`
- MTK 专有：`nvram`、`nvdata`、`nvcfg`、`protect1/2`、`seccfg`、`proinfo`、`md1img`、`md1dsp`、`md1arm7`、`md3img`、`spmfw`、`scp1/2`、`sspm1/2`、`dpm1/2`、`mcupm1/2`、`gz1/2`、`ccu`、`vcp`、`apusys1/2`、`mvpu_algo1/2`、`pi_img`、`boot_para`、`odmdtbo`

### 4.3 内核模块

- ramdisk 里 **275 个 `.ko`**，带 `modules.dep / alias / softdep / load / load.recovery` 全套
- 原厂 recovery 的加载顺序：`modules.load.recovery`，**246 条**
- 触摸：**FocalTech FT3683G**，三段加载 `oplus_bsp_tp_comon` → `oplus_bsp_tp_ft3683g` → `oplus_ft3683g`
- 触摸固件工程：`omega-c2`，面板 alias **AC157**，BOE / 天马两份固件在 `/vendor/firmware/tp/omega-c2/`
- 面板：`oplus21641_*`（BOE NT37701 / NT37701A、天马 ILI7838A）与 `oplus21861_*` 两组，外加 `mtk_panel_ext` + `mediatek-drm`
- 振动：`aw8697` + `haptic`（波形 bin 编在驱动里）
- 无线：ramdisk 只有 `cfg80211.ko` / `mac80211.ko`，**MTK 的 Wi-Fi 驱动不在 vendor_boot**，在 `vendor_dlkm`（connac2 方案，通常 `conninfra` + `wlan_drv_gen4m`）
- TEE：`mcDrvModule.ko`（Trustonic MobiCore）+ `vendor/bin/mcDriverDaemon`，恢复模式里就能用

### 4.4 恢复模式自带的服务

`system/bin/` 有 `hwservicemanager`、`servicemanager`、`adbd`、`recovery`、`phoenix_recovery`、`health@2.1-service`；
`vendor/bin/` 有 `mcDriverDaemon`、`vndservicemanager`、`vendor/bin/hw/android.hardware.boot@1.2-service`。

**缺 keymint / gatekeeper 的 HAL 程序**——它们在 vendor 分区，不在 ramdisk 里。

---

## 5. Skyward 要动的地方（按先后）

| 优先级 | 位置 | 要做的事 |
|---|---|---|
| P0 | `skyward-bootimg` | 新增 vendor_boot v4 读写：2128 字节头、page 对齐、单段 ramdisk、dtbo 段原样保留、bootconfig 段 |
| P0 | `skyward-pack` / `skyward-install` | 让"recovery 镜像"接受 VNDRBOOT，并把它落到 `vendor_boot` 分区；刷写后用 `skyward-vab` 重签（AVB 尾部在分区最后 64 字节） |
| P1 | `skyward-crypto` | 不用改解析逻辑：keymint 已确认是 AIDL、gatekeeper 是 HIDL 1.0，Skyward 两种都支持；只是不再有 `qseecomd`，改成 mobicore + Trustonic 的两个 HAL；metadata 加密密钥目录 `/metadata/vold/metadata_encryption` |
| P1 | `skyward-modules` | 已经支持 `vendor_boot → /lib/modules` 的映射，可直接用；注意 MTK 模块文件名带连字符（`mediatek-drm.ko`、`mt6375-battery.ko`），`load` 里要按文件名写 |
| P2 | `skyward-block` | 解析 fstab 时认 `sysfs_path=/sys/devices/platform/soc/112b0000.ufshci`，以及 `oplusreserve2 → /cache` 这种非标准映射 |
| P2 | `skyward-info` | 电量走 MT6375 + OPLUS 充电栈（`oplus_chg_v2`），节点与高通完全不同 |
| P3 | 触摸 | **不需要**像小米那样起用户态触摸服务：FT3683G 由内核 input 直接给 `/dev/input/eventX`，`[touch].writes` 留空即可 |

---

## 6. 已经产出的东西

- `skyward/devices/pickle.toml`：用这份镜像的实测数据填好的设备配置（`cargo run -p skyward-config` 已实测可解析）
- `pickle-stock/`：从原厂镜像里抽出的原始参考文件
  - `recovery.fstab`、`fstab.mt6895`（分区与加密参数）
  - `modules.load.recovery`（原厂 246 条加载序）、`modules.dep`、`modules.txt`（全部 275 个模块名）
  - `init.recovery.mt6895.rc`、`init.rc`（恢复模式怎么起来）
  - `vnd_touch_project_config.xml`（触摸工程 / 面板 / 固件对应关系）
  - `vendor/`：keymint、gatekeeper、wifi、supplicant、boot、trustonic 的 `.rc`，keymint 的 VINTF 声明，`vendor-build.prop`，`wpa_supplicant.conf`
  - `vendor_dlkm-modules.txt`：vendor_dlkm 的 298 个模块名

---

## 7. 补：vendor / vendor_dlkm 分区核对结果

拿到 `vendor.img`（1.7 GB，ext4）和 `vendor_dlkm_b.img`（58 MB，ext4）后，之前悬着的两块全部落地了。

### 7.1 解密的三个服务（全部确认）

| 服务 | 实际路径 | 接口 | 运行身份 |
|---|---|---|---|
| Trustonic 守护进程 | `/vendor/bin/mcDriverDaemon` | 无（读 `/dev/mobicore`） | `system:system`，priority -20 |
| keymint | `/vendor/bin/hw/android.hardware.security.keymint-service.trustonic` | **AIDL** `IKeyMintDevice/default`（另有 `IRemotelyProvisionedComponent/default`） | `system:system` |
| gatekeeper | `/vendor/bin/hw/android.hardware.gatekeeper@1.0-service` | **HIDL** `android.hardware.gatekeeper@1.0::IGatekeeper/default` | `system:system` |

关键点：

- keymint 是 **AIDL（ndk backend）**，不是 HIDL —— 正好对上 Skyward `skyward-crypto/src/keymint.rs` 里写死的 AIDL 名字，**不用改解密代码**
- gatekeeper 是 HIDL 1.0，Skyward 的 `ready_hardware_service` 也支持
- mobicore 的完整命令（原厂 `trustonic.rc`）：`--sfs-reformat --P1 /mnt/vendor/persist/mcRegistry` + **31 个 `-r *.drbin`**
  - `--sfs-reformat` 在恢复模式里不建议照抄：它是正常开机重建 TEE 安全文件系统用的，可能清掉 keystore/gatekeeper 状态
  - `/dev/mobicore`、`/dev/mobicore-user` 由 `mcDrvModule.ko` 建，这个模块在原厂 ramdisk 里就有
- `boot@1.2-service`（HIDL `IBootControl`）原厂 ramdisk 自带，直接可用

### 7.2 WLAN（vendor_dlkm）

| 项 | 值 |
|---|---|
| 驱动 | `conninfra` → `wlan_drv_gen4m_6895`（依赖链自动带 `wmt_chrdev_wifi_connac2`、`connadp`、`connfem`…） |
| 上电 | 写 `1` 到 `/dev/wmtWifi`（`wmt_chrdev_wifi_connac2` 建的字符设备；`libwifi-hal.so` 也是这么做的） |
| 属性 | `ro.vendor.wlan.gen=gen4m_6895`、`ro.vendor.wlan.chrdev=wmt_chrdev_wifi_connac2` |
| 固件 | `/vendor/firmware/WIFI_RAM_CODE_soc7_0_*.bin` |
| supplicant | `/vendor/bin/hw/wpa_supplicant`，原厂参数 `-O/data/vendor/wifi/wpa/sockets -g@android:wpa_wlan0`，HIDL `@1.0::ISupplicant/default` |
| 联网探测 | `https://connectivitycheck.gstatic.com/generate_204`（另有 `www.google.cn`） |
| 模块总数 | vendor_dlkm 里 298 个 `.ko`，带自己的 `modules.dep` |

### 7.3 其他

- vendor 的 `ro.product.vendor.device` = **`ossi`**（OPLUS 统一包代号，不是 `pickle`），说明这份 vendor 是多个 MTK 机型共用的
- vendor 里没有 `lib/modules` 内容（模块都在 vendor_dlkm 和 vendor_boot）
- 振动器 `aw8697` + `haptic_hv` 只暴露 `activate/duration/gain/vmax/index`，**没有 f0 节点**，所以 Skyward 的 `[haptics]`（需要 `resonance` 文件）暂时填不了

---

## 8. 还缺什么

1. **真机**：`/proc/modules`（确认实际加载）、`dmesg | grep -i lcm`（确认实际面板）、`ls /sys/class/power_supply`（确认 MT6375 的电量节点）
2. **原厂完整 OTA 包地址**：`[stock].package` 要用，另外要用包里的 `vendor_boot.img` 重算 SHA-256（当前分区 dump 的哈希是 `bb5a094d…839670`）
3. **代码层**：`skyward-bootimg` 的 vendor_boot 打包后端（P0），其余配置项已经齐了
