#!/bin/sh
# 为 OnePlus Ace（pickle / PGKM10）构建 Skyward 恢复镜像。
#
# 依赖：rustup 的 stable 工具链与 aarch64-linux-android 目标、Android NDK r29、
# lz4 命令行（Ramdisk::encode 用它压缩，内核只认 legacy 帧）。
#
# 用法：
#   NDK=$HOME/android-ndk-r29 ./build-pickle.sh [原厂 vendor_boot.img]
# 不给参数时用仓库里解出来的那份 dump。
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SRC=${1:-/tmp/vendor_boot_extract/vendor_boot_20261111_203114.img}
NDK=${NDK:-$HOME/android-ndk-r29}
TOOL=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin
API=${API:-30}

export PATH=$TOOL:$PATH
export CC_aarch64_linux_android=$TOOL/aarch64-linux-android$API-clang
export CXX_aarch64_linux_android=$TOOL/aarch64-linux-android$API-clang++
export AR_aarch64_linux_android=$TOOL/llvm-ar
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$TOOL/aarch64-linux-android$API-clang

cd "$ROOT/skyward"

# 打包工具与预览工具跑在电脑上。
cargo build --release -p skyward-pack

# 恢复模式的服务，跑在手机上。
cargo build --release -p skyward --target aarch64-linux-android

# 第二系统的第一个程序在没有任何动态库之前运行，静态链接。
RUSTFLAGS="-C target-feature=+crt-static" \
  cargo build --release -p skyward-system-init --target aarch64-linux-android \
  --target-dir target/system-init

# 在原厂 vendor_boot 上换掉 recovery 服务，并按分区大小补上 AVB 尾巴。
./target/release/skyward-pack recovery \
  --stock "$SRC" \
  --binary target/aarch64-linux-android/release/skyward \
  --system-init target/system-init/aarch64-linux-android/release/system-init \
  --config devices/pickle.toml \
  --font resources/fonts/MiSansVF.ttf \
  --mono resources/fonts/KirkMono-Regular.ttf \
  --mono-license resources/fonts/KirkMono-OFL.txt \
  --out "$ROOT/skyward-pickle-vendor_boot.img"

sha256sum "$ROOT/skyward-pickle-vendor_boot.img"
